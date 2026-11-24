//! Owner-private review drafts and their immutable source snapshots.
//!
//! `output-reviews.v2.json` holds only review metadata. Each snapshot's bytes
//! are written once, at capture, to their own owner-private file in
//! `output-review-snapshots/`, named by the SHA-256 of those bytes, so a note,
//! anchor or thread edit rewrites only the small metadata file. A snapshot
//! file is written before the metadata that names it and removed after the
//! metadata that dropped it: a crash between the two leaves an unnamed file,
//! which the next open removes.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::{
    output_preview::{OutputPreviewContent, OutputPreviewSnapshot, OutputTextFormat},
    output_review::{OutputAnchor, OutputFeedbackDelivery, OutputReview},
};
use interface_storage::{
    JsonLimits, PrivateDirectory, TargetSnapshot, WriterLock, atomic_write, is_atomic_temp_name,
    read_bounded, read_json, sha256_hex, snapshot_target,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// The store's file in the private data root.
pub const OUTPUT_REVIEWS_FILE_NAME: &str = "output-reviews.v2.json";
/// The version 1 store (snapshot bytes inline), carried over once when
/// [`OUTPUT_REVIEWS_FILE_NAME`] does not exist yet and never written again.
pub const OUTPUT_REVIEWS_V1_FILE_NAME: &str = "output-reviews.v1.json";
/// The snapshot files' directory, beside the store's file.
pub const OUTPUT_REVIEW_SNAPSHOTS: &str = "output-review-snapshots";
const MAX_REVIEWS: usize = 32;
/// One snapshot's bytes: the output preview's 5 MB read limit. With
/// [`MAX_REVIEWS`] this bounds every snapshot file together at 160 MiB.
const MAX_SNAPSHOT_BYTES: usize = 5 * 1024 * 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const V1_MAX_BYTES: usize = 48 * 1024 * 1024;
const LOCK_NAME: &str = ".output-reviews.lock";
const SNAPSHOTS_LOCK_NAME: &str = ".output-review-snapshots.lock";
/// The bytes of snapshot files, by file name.
type Files = BTreeMap<String, Vec<u8>>;
const UNCERTAIN: &str = "Output review storage is uncertain; restart before saving again.";
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    reviews: Vec<StoredReview>,
}
/// An [`OutputReview`] whose snapshot names its file instead of holding its bytes.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredReview {
    id: String,
    revision: u64,
    snapshot: StoredSnapshot,
    anchor: OutputAnchor,
    note: String,
    thread_key: Option<String>,
    task: String,
    updated_at: u64,
    delivery: Option<OutputFeedbackDelivery>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredSnapshot {
    root: String,
    path: String,
    byte_length: u64,
    revision: Option<String>,
    content: StoredContent,
}
/// [`OutputPreviewContent`] with its text or bytes in the snapshot file named `sha256`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum StoredContent {
    Text {
        format: OutputTextFormat,
        sha256: String,
    },
    Image {
        #[serde(rename = "mediaType")]
        media_type: String,
        sha256: String,
    },
    Pdf {
        sha256: String,
    },
    Unsupported {
        reason: String,
    },
}
impl StoredContent {
    /// The snapshot file holding this content's text or bytes.
    fn file(&self) -> Option<&str> {
        match self {
            Self::Text { sha256, .. } | Self::Image { sha256, .. } | Self::Pdf { sha256 } => {
                Some(sha256)
            }
            Self::Unsupported { .. } => None,
        }
    }
}
/// Splits a snapshot's text or bytes from the rest of it.
fn split(snapshot: OutputPreviewSnapshot) -> (StoredSnapshot, Option<Vec<u8>>) {
    let (content, bytes) = match snapshot.content {
        OutputPreviewContent::Text { text, format } => {
            let bytes = text.into_bytes();
            let sha256 = sha256_hex(&bytes);
            (StoredContent::Text { format, sha256 }, Some(bytes))
        }
        OutputPreviewContent::Image { media_type, bytes } => {
            let sha256 = sha256_hex(&bytes);
            (StoredContent::Image { media_type, sha256 }, Some(bytes))
        }
        OutputPreviewContent::Pdf { bytes } => {
            let sha256 = sha256_hex(&bytes);
            (StoredContent::Pdf { sha256 }, Some(bytes))
        }
        OutputPreviewContent::Unsupported { reason } => {
            (StoredContent::Unsupported { reason }, None)
        }
    };
    let stored = StoredSnapshot {
        root: snapshot.root,
        path: snapshot.path,
        byte_length: snapshot.byte_length,
        revision: snapshot.revision,
        content,
    };
    (stored, bytes)
}
fn is_snapshot_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid(review: &StoredReview) -> bool {
    interface_contracts::output_review::valid_id(&review.id)
        && review.revision != 0
        && review.anchor.valid()
        && review.note.len() <= 32_000
        && review.task.len() <= 512
        && review.snapshot.revision.is_some()
        && review.snapshot.content.file().is_none_or(is_snapshot_name)
}
/// The snapshot files, leased for the store's life. Each holds exactly the
/// bytes its name is the SHA-256 of, and is never rewritten.
struct SnapshotFiles {
    directory: PrivateDirectory,
    lock: WriterLock,
}
impl SnapshotFiles {
    fn open(root: &PrivateDirectory) -> Result<Self, SettingsError> {
        let directory = root.open_or_create_child(OUTPUT_REVIEW_SNAPSHOTS)?;
        let lock = WriterLock::acquire_for(root, SNAPSHOTS_LOCK_NAME, &directory)?;
        Ok(Self { directory, lock })
    }
    /// The file `name`'s bytes; `None` when it is missing, unsafe, too large
    /// or not the bytes it is named for.
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        let file = read_bounded(&self.directory, name, MAX_SNAPSHOT_BYTES).ok()??;
        (hex(file.snapshot().sha256()) == name).then(|| file.into_parts().0)
    }
    /// Durably writes `bytes` as the file `name` unless it already holds them
    /// (a file a crash left behind before its review was saved).
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), SettingsError> {
        let current = snapshot_target(&self.directory, name, MAX_SNAPSHOT_BYTES)?;
        if matches!(&current, TargetSnapshot::File(file) if hex(file.sha256()) == name) {
            return Ok(());
        }
        atomic_write(
            &self.directory,
            &self.lock,
            name,
            bytes,
            &current,
            MAX_SNAPSHOT_BYTES,
        )?;
        Ok(())
    }
    /// Removes the snapshot files and interrupted writes no review names. A
    /// failure leaves the file for the next open.
    fn prune(&self, named: &Files) {
        let Ok(entries) = std::fs::read_dir(self.directory.path()) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if (is_snapshot_name(&name) || is_atomic_temp_name(&name))
                && !named.contains_key(&name)
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                self.remove(&name);
            }
        }
    }
    fn remove(&self, name: &str) {
        if self.lock.assert_held_for(&self.directory).is_ok()
            && self.directory.assert_unchanged().is_ok()
        {
            let _ = std::fs::remove_file(self.directory.path().join(name));
        }
    }
}
fn hex(digest: &[u8; 32]) -> String {
    use std::fmt::Write;
    digest
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}
/// The version 1 document, every snapshot's bytes inline. Read once to carry
/// it over; never written.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct V1State {
    version: u8,
    #[serde(deserialize_with = "v1_rows::deserialize")]
    reviews: Vec<OutputReview>,
}
// Version 1 rows are JSON strings, so a five-MiB raster does not consume five
// million nodes in the outer strict JSON parser.
mod v1_rows {
    use super::OutputReview;
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<OutputReview>, D::Error> {
        let rows = Vec::<String>::deserialize(deserializer)?;
        if rows.len() > super::MAX_REVIEWS {
            return Err(serde::de::Error::custom("too many reviews"));
        }
        rows.iter()
            .map(|row| serde_json::from_str(row).map_err(serde::de::Error::custom))
            .collect()
    }
}
/// The version 1 store in `root`, carried over: its reviews with their
/// snapshots split out. Read-only: the file is never changed, moved or
/// quarantined. `None` when it is absent, unreadable or invalid; it is then
/// left for the build that wrote it and the store starts empty.
fn read_v1(root: &PrivateDirectory) -> Option<(Vec<StoredReview>, Files)> {
    let limits = JsonLimits::new(V1_MAX_BYTES, 20, 100_000).ok()?;
    let state = read_json::<V1State>(root, OUTPUT_REVIEWS_V1_FILE_NAME, limits)
        .ok()??
        .into_parts()
        .0;
    if state.version != 1 {
        return None;
    }
    let mut files = BTreeMap::new();
    let mut reviews = Vec::with_capacity(state.reviews.len());
    for review in state.reviews {
        let (snapshot, bytes) = split(review.snapshot);
        if let (Some(name), Some(bytes)) = (snapshot.content.file(), bytes) {
            if bytes.len() > MAX_SNAPSHOT_BYTES {
                return None;
            }
            files.insert(name.to_owned(), bytes);
        }
        let review = StoredReview {
            id: review.id,
            revision: review.revision,
            snapshot,
            anchor: review.anchor,
            note: review.note,
            thread_key: review.thread_key,
            task: review.task,
            updated_at: review.updated_at,
            delivery: review.delivery,
        };
        if !valid(&review) {
            return None;
        }
        reviews.push(review);
    }
    Some((reviews, files))
}
pub struct OutputReviewStore {
    document: LeasedDocument,
    snapshots: SnapshotFiles,
    state: State,
    /// The bytes of every snapshot file a review names.
    files: Files,
}
fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn invalid_store() -> SettingsError {
    invalid("Invalid output review store; existing data was retained.")
}
impl OutputReviewStore {
    /// Opens the store at `path` ([`OUTPUT_REVIEWS_FILE_NAME`] in the app)
    /// and its snapshot files. When `path` does not exist, the version 1
    /// store beside it ([`OUTPUT_REVIEWS_V1_FILE_NAME`]) is carried over once
    /// and left as it is. Snapshot files no review names are removed.
    /// # Errors
    /// Refuses a held writer lease, malformed state, a missing or changed
    /// snapshot file or unsafe storage.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<State>(
            path,
            LOCK_NAME,
            JsonLimits::new(MAX_BYTES, 20, 100_000)?,
            UNCERTAIN,
        )?;
        let snapshots = SnapshotFiles::open(document.directory())?;
        let (state, files, migrated) = match state {
            Some(state) => {
                if state.version != 2
                    || state.reviews.len() > MAX_REVIEWS
                    || !state.reviews.iter().all(valid)
                {
                    return Err(invalid_store());
                }
                let mut files = BTreeMap::new();
                for name in state
                    .reviews
                    .iter()
                    .filter_map(|r| r.snapshot.content.file())
                {
                    if !files.contains_key(name) {
                        let bytes = snapshots.read(name).ok_or_else(invalid_store)?;
                        files.insert(name.to_owned(), bytes);
                    }
                }
                (state, files, false)
            }
            None => match read_v1(document.directory()) {
                Some((reviews, files)) => (
                    State {
                        version: 2,
                        reviews,
                    },
                    files,
                    true,
                ),
                None => (
                    State {
                        version: 2,
                        reviews: Vec::new(),
                    },
                    BTreeMap::new(),
                    false,
                ),
            },
        };
        if state
            .reviews
            .iter()
            .any(|review| match &review.snapshot.content {
                StoredContent::Text { sha256, .. } => files
                    .get(sha256)
                    .is_none_or(|bytes| std::str::from_utf8(bytes).is_err()),
                _ => false,
            })
        {
            return Err(invalid_store());
        }
        let mut store = Self {
            document,
            snapshots,
            state: State {
                version: 2,
                reviews: Vec::new(),
            },
            files,
        };
        if migrated {
            for (name, bytes) in &store.files {
                store.snapshots.write(name, bytes)?;
            }
            // Written at once, so the version 1 file is never read again.
            store.commit(state)?;
        } else {
            store.state = state;
        }
        store.snapshots.prune(&store.files);
        Ok(store)
    }
    #[must_use]
    pub fn list(&self, root: &str) -> Vec<OutputReview> {
        self.state
            .reviews
            .iter()
            .filter(|r| r.snapshot.root == root)
            .map(|r| self.joined(r))
            .collect()
    }
    /// `review` as the wire carries it, its snapshot's bytes read back in.
    fn joined(&self, review: &StoredReview) -> OutputReview {
        let bytes = |name: &str| self.files.get(name).cloned().unwrap_or_default();
        let content = match &review.snapshot.content {
            StoredContent::Text { format, sha256 } => OutputPreviewContent::Text {
                text: String::from_utf8(bytes(sha256)).unwrap_or_default(),
                format: *format,
            },
            StoredContent::Image { media_type, sha256 } => OutputPreviewContent::Image {
                media_type: media_type.clone(),
                bytes: bytes(sha256),
            },
            StoredContent::Pdf { sha256 } => OutputPreviewContent::Pdf {
                bytes: bytes(sha256),
            },
            StoredContent::Unsupported { reason } => OutputPreviewContent::Unsupported {
                reason: reason.clone(),
            },
        };
        OutputReview {
            id: review.id.clone(),
            revision: review.revision,
            snapshot: OutputPreviewSnapshot {
                root: review.snapshot.root.clone(),
                path: review.snapshot.path.clone(),
                byte_length: review.snapshot.byte_length,
                revision: review.snapshot.revision.clone(),
                content,
            },
            anchor: review.anchor.clone(),
            note: review.note.clone(),
            thread_key: review.thread_key.clone(),
            task: review.task.clone(),
            updated_at: review.updated_at,
            delivery: review.delivery,
        }
    }
    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
    /// Captures only snapshots supplied by the authorized workspace reader.
    /// The snapshot's file is written before the review that names it.
    /// # Errors
    /// Refuses unsupported content or a full/unavailable store.
    pub fn capture(
        &mut self,
        snapshot: OutputPreviewSnapshot,
        now: u64,
    ) -> Result<OutputReview, SettingsError> {
        let Some(revision) = &snapshot.revision else {
            return Err(invalid(
                "This output has no supported immutable preview to save.",
            ));
        };
        let id = sha256_hex(
            &serde_json::to_vec(&(&snapshot.root, &snapshot.path, revision))
                .map_err(|_| invalid("Could not identify the output version."))?,
        );
        if let Some(review) = self.state.reviews.iter().find(|r| r.id == id) {
            return Ok(self.joined(review));
        }
        if self.state.reviews.len() >= MAX_REVIEWS {
            return Err(invalid(
                "32 output versions are saved. Delete an older review before saving another.",
            ));
        }
        let (snapshot, bytes) = split(snapshot);
        if let (Some(name), Some(bytes)) = (snapshot.content.file(), &bytes) {
            if bytes.len() > MAX_SNAPSHOT_BYTES {
                return Err(invalid(
                    "This output is larger than the 5 MB a saved review can keep. Open it in its own application.",
                ));
            }
            if !self.files.contains_key(name) {
                self.snapshots.write(name, bytes)?;
            }
        }
        let review = StoredReview {
            id,
            revision: 1,
            snapshot,
            anchor: OutputAnchor::File,
            note: String::new(),
            thread_key: None,
            task: String::new(),
            updated_at: now,
            delivery: None,
        };
        let mut state = self.state.clone();
        state.reviews.push(review.clone());
        // A failed commit leaves the file for the next open to remove.
        self.commit(state)?;
        if let (Some(name), Some(bytes)) = (review.snapshot.content.file(), bytes) {
            self.files.entry(name.to_owned()).or_insert(bytes);
        }
        Ok(self.joined(&review))
    }
    /// # Errors
    /// Refuses stale edits, missing reviews and unavailable storage.
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &mut self,
        root: &str,
        id: &str,
        revision: u64,
        anchor: OutputAnchor,
        note: String,
        thread_key: Option<String>,
        task: String,
        now: u64,
    ) -> Result<OutputReview, SettingsError> {
        if !anchor.valid() || note.len() > 32_000 || task.len() > 512 {
            return Err(invalid("Invalid output review."));
        }
        let mut state = self.state.clone();
        let review = state
            .reviews
            .iter_mut()
            .find(|r| r.id == id && r.snapshot.root == root)
            .ok_or_else(|| invalid("That saved output review no longer exists."))?;
        if review.revision != revision {
            return Err(invalid(
                "This review changed in another pane. Reload saved reviews before editing.",
            ));
        }
        if matches!(
            review.delivery,
            Some(OutputFeedbackDelivery::Sending | OutputFeedbackDelivery::Unknown)
        ) {
            return Err(invalid(
                "Feedback may already have been delivered. Inspect the selected thread before starting a new review.",
            ));
        }
        if review.note != note || review.anchor != anchor || review.thread_key != thread_key {
            review.delivery = None;
        }
        review.revision = revision
            .checked_add(1)
            .ok_or_else(|| invalid("Output review revision exhausted."))?;
        review.anchor = anchor;
        review.note = note;
        review.thread_key = thread_key;
        review.task = task;
        review.updated_at = now;
        let answer = review.clone();
        self.commit(state)?;
        Ok(self.joined(&answer))
    }
    /// # Errors
    /// A durable sending reservation is never replayed after an uncertain outcome.
    pub fn feedback(
        &mut self,
        root: &str,
        id: &str,
        revision: u64,
        settlement: Option<OutputFeedbackDelivery>,
    ) -> Result<OutputReview, SettingsError> {
        let mut state = self.state.clone();
        let review = state
            .reviews
            .iter_mut()
            .find(|r| r.id == id && r.snapshot.root == root && r.revision == revision)
            .ok_or_else(|| invalid("The review changed. Refresh before sending feedback."))?;
        if let Some(delivery) = settlement {
            if review.delivery != Some(OutputFeedbackDelivery::Sending)
                || delivery == OutputFeedbackDelivery::Sending
            {
                return Err(invalid("That feedback attempt cannot be settled."));
            }
            review.delivery = Some(delivery);
        } else {
            if !matches!(review.delivery, None | Some(OutputFeedbackDelivery::Unsent))
                || review.note.trim().is_empty()
                || review.thread_key.is_none()
            {
                return Err(invalid(
                    "Choose a thread and a new feedback draft. Previously sent or uncertain feedback cannot be replayed.",
                ));
            }
            review.delivery = Some(OutputFeedbackDelivery::Sending);
        }
        review.revision = revision
            .checked_add(1)
            .ok_or_else(|| invalid("Output review revision exhausted."))?;
        let answer = review.clone();
        self.commit(state)?;
        Ok(self.joined(&answer))
    }
    /// # Errors
    /// Refuses deletion after a concurrent change or storage failure.
    pub fn delete(&mut self, root: &str, id: &str, revision: u64) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        let at = state
            .reviews
            .iter()
            .position(|r| r.id == id && r.snapshot.root == root && r.revision == revision)
            .ok_or_else(|| invalid("The saved review changed or no longer exists."))?;
        let removed = state.reviews.remove(at);
        self.commit(state)?;
        // Removed after the review that named it, so a crash leaves only an
        // unnamed file for the next open.
        if let Some(name) = removed.snapshot.content.file()
            && !self
                .state
                .reviews
                .iter()
                .any(|r| r.snapshot.content.file() == Some(name))
        {
            self.files.remove(name);
            self.snapshots.remove(name);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(text: &str) -> OutputPreviewSnapshot {
        OutputPreviewSnapshot {
            root: "/project".into(),
            path: "report.md".into(),
            byte_length: text.len() as u64,
            revision: Some(sha256_hex(text.as_bytes())),
            content: OutputPreviewContent::Text {
                text: text.into(),
                format: OutputTextFormat::Markdown,
            },
        }
    }
    #[test]
    fn durable_reviews_keep_immutable_versions_and_refuse_lost_updates() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reviews.json");
        let mut store = OutputReviewStore::open(&path).unwrap();
        let old = store.capture(snapshot("old"), 1).unwrap();
        let saved = store
            .save(
                "/project",
                &old.id,
                1,
                OutputAnchor::Text {
                    quote: "old".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                },
                "Revise the ending".into(),
                Some("claude#a".into()),
                "Release notes".into(),
                2,
            )
            .unwrap();
        assert!(
            store
                .save(
                    "/project",
                    &old.id,
                    1,
                    OutputAnchor::File,
                    "lost update".into(),
                    None,
                    String::new(),
                    3
                )
                .is_err()
        );
        let new = store.capture(snapshot("new"), 4).unwrap();
        assert_ne!(old.id, new.id);
        assert!(store.list("/other").is_empty());
        drop(store);
        let mut store = OutputReviewStore::open(&path).unwrap();
        assert_eq!(store.list("/project"), vec![saved.clone(), new]);
        assert!(store.delete("/project", &old.id, 1).is_err());
        store.delete("/project", &old.id, saved.revision).unwrap();
        assert_eq!(store.list("/project").len(), 1);
    }
    #[test]
    fn feedback_reservation_survives_restart_without_duplicate_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reviews.json");
        let mut store = OutputReviewStore::open(&path).unwrap();
        let first = store.capture(snapshot("review me"), 1).unwrap();
        let draft = store
            .save(
                "/project",
                &first.id,
                first.revision,
                OutputAnchor::File,
                "Revise the ending".into(),
                Some("claude#a".into()),
                "Release notes".into(),
                2,
            )
            .unwrap();
        let reserved = store
            .feedback("/project", &draft.id, draft.revision, None)
            .unwrap();
        drop(store);
        let mut store = OutputReviewStore::open(&path).unwrap();
        assert!(
            store
                .feedback("/project", &reserved.id, reserved.revision, None)
                .is_err()
        );
        let unknown = store
            .feedback(
                "/project",
                &reserved.id,
                reserved.revision,
                Some(OutputFeedbackDelivery::Unknown),
            )
            .unwrap();
        assert!(
            store
                .feedback("/project", &unknown.id, unknown.revision, None)
                .is_err()
        );
        assert!(
            store
                .save(
                    "/project",
                    &unknown.id,
                    unknown.revision,
                    OutputAnchor::File,
                    "edited".into(),
                    Some("claude#b".into()),
                    String::new(),
                    3
                )
                .is_err()
        );
    }
    fn image(path: &str, bytes: &[u8]) -> OutputPreviewSnapshot {
        OutputPreviewSnapshot {
            root: "/project".into(),
            path: path.into(),
            byte_length: bytes.len() as u64,
            revision: Some(sha256_hex(bytes)),
            content: OutputPreviewContent::Image {
                media_type: "image/png".into(),
                bytes: bytes.to_vec(),
            },
        }
    }
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x01\x02\xff";
    fn snapshot_files(root: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(root.join(OUTPUT_REVIEW_SNAPSHOTS))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        let text = store.capture(snapshot("old"), 1).unwrap();
        let picture = store.capture(image("chart.png", PNG), 2).unwrap();
        crate::golden::assert_golden("output-reviews.captured.json", temp.path(), &path);
        store
            .save(
                "/project",
                &picture.id,
                1,
                OutputAnchor::Region {
                    x: 0.25,
                    y: 0.5,
                    width: 0.5,
                    height: 0.25,
                },
                "Label the axis".into(),
                Some("claude#a".into()),
                "Charts".into(),
                3,
            )
            .unwrap();
        crate::golden::assert_golden("output-reviews.saved.json", temp.path(), &path);
        let snapshots = temp.path().join(OUTPUT_REVIEW_SNAPSHOTS);
        assert_eq!(snapshot_files(temp.path()), {
            let mut names = vec![sha256_hex(b"old"), sha256_hex(PNG)];
            names.sort();
            names
        });
        for (name, bytes) in [(sha256_hex(b"old"), &b"old"[..]), (sha256_hex(PNG), PNG)] {
            use std::os::unix::fs::PermissionsExt;
            let file = snapshots.join(&name);
            assert_eq!(std::fs::read(&file).unwrap(), bytes);
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, 0o600);
        }
        drop(store);
        let store = OutputReviewStore::open(&path).unwrap();
        assert_eq!(store.list("/project")[0], text);
        assert_eq!(store.list("/project")[1].snapshot, picture.snapshot);
    }
    #[test]
    fn a_note_save_rewrites_only_the_small_metadata_file() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        let mut png = PNG.to_vec();
        png.extend(
            (0..5 * 1024 * 1024 - 64).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8),
        );
        let review = store.capture(image("big.png", &png), 1).unwrap();
        let file = temp
            .path()
            .join(OUTPUT_REVIEW_SNAPSHOTS)
            .join(sha256_hex(&png));
        let before = std::fs::metadata(&file).unwrap();
        let mut revision = review.revision;
        for note in ["L", "La", "Label the axis"] {
            revision = store
                .save(
                    "/project",
                    &review.id,
                    revision,
                    OutputAnchor::File,
                    note.into(),
                    None,
                    String::new(),
                    2,
                )
                .unwrap()
                .revision;
        }
        let after = std::fs::metadata(&file).unwrap();
        assert_eq!(
            (
                after.ino(),
                after.mtime(),
                after.mtime_nsec(),
                after.ctime_nsec()
            ),
            (
                before.ino(),
                before.mtime(),
                before.mtime_nsec(),
                before.ctime_nsec()
            )
        );
        let metadata = std::fs::metadata(&path).unwrap().len();
        assert!(metadata < 4096, "a note save wrote {metadata} bytes");
        assert_eq!(
            store.list("/project")[0].snapshot.content,
            image("big.png", &png).content
        );
    }
    #[test]
    fn a_version_1_store_is_carried_over_once_and_left_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let v1 = temp.path().join(OUTPUT_REVIEWS_V1_FILE_NAME);
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/output-reviews.v1.json");
        std::fs::copy(&fixture, &v1).unwrap();
        std::fs::set_permissions(&v1, std::fs::Permissions::from_mode(0o600)).unwrap();
        let original = std::fs::read(&v1).unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        let reviews = store.list("/project");
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0].snapshot, snapshot("old"));
        assert_eq!(reviews[0].note, "Revise the ending");
        assert_eq!(reviews[0].thread_key.as_deref(), Some("claude#a"));
        assert_eq!(reviews[0].revision, 2);
        assert_eq!(reviews[1].snapshot, image("chart.png", PNG));
        assert_eq!(
            reviews[1].anchor,
            OutputAnchor::Region {
                x: 0.25,
                y: 0.5,
                width: 0.5,
                height: 0.25
            }
        );
        assert_eq!(reviews[1].note, "Label the axis");
        assert!(path.exists(), "the carried-over store is written at once");
        assert_eq!(std::fs::read(&v1).unwrap(), original);
        store
            .delete("/project", &reviews[1].id, reviews[1].revision)
            .unwrap();
        assert_eq!(snapshot_files(temp.path()), vec![sha256_hex(b"old")]);
        drop(store);
        let store = OutputReviewStore::open(&path).unwrap();
        assert_eq!(store.list("/project"), vec![reviews[0].clone()]);
        assert_eq!(std::fs::read(&v1).unwrap(), original);
    }
    #[test]
    fn snapshot_files_are_shared_and_pruned_once_no_review_names_them() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        let first = store.capture(image("a.png", PNG), 1).unwrap();
        let second = store.capture(image("b.png", PNG), 2).unwrap();
        assert_eq!(snapshot_files(temp.path()), vec![sha256_hex(PNG)]);
        store.delete("/project", &first.id, first.revision).unwrap();
        assert_eq!(snapshot_files(temp.path()), vec![sha256_hex(PNG)]);
        store
            .delete("/project", &second.id, second.revision)
            .unwrap();
        assert!(snapshot_files(temp.path()).is_empty());
        let kept = store.capture(image("c.png", PNG), 3).unwrap();
        drop(store);
        // A crash between a snapshot write and its review, or between a
        // delete and its file's removal, leaves files no review names.
        let snapshots = temp.path().join(OUTPUT_REVIEW_SNAPSHOTS);
        let orphan = sha256_hex(b"orphan");
        std::fs::write(snapshots.join(&orphan), b"orphan").unwrap();
        let temporary = format!(
            ".interface-atomic-{}-{}.tmp",
            interface_storage::atomic_temp_digest_prefix(&orphan),
            "6f2f7b9c-2a51-4a8e-9a43-1b0c8f6d5e21"
        );
        std::fs::write(snapshots.join(&temporary), b"torn").unwrap();
        std::fs::write(snapshots.join("notes.txt"), b"not ours").unwrap();
        let store = OutputReviewStore::open(&path).unwrap();
        assert_eq!(
            snapshot_files(temp.path()),
            vec![sha256_hex(PNG), "notes.txt".to_owned()]
        );
        assert_eq!(store.list("/project"), vec![kept]);
    }
    #[test]
    fn a_missing_or_changed_snapshot_file_refuses_the_store_and_keeps_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        store.capture(image("a.png", PNG), 1).unwrap();
        drop(store);
        let file = temp
            .path()
            .join(OUTPUT_REVIEW_SNAPSHOTS)
            .join(sha256_hex(PNG));
        std::fs::write(&file, b"changed").unwrap();
        let metadata = std::fs::read(&path).unwrap();
        assert!(OutputReviewStore::open(&path).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"changed");
        std::fs::remove_file(&file).unwrap();
        assert!(OutputReviewStore::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), metadata);
    }
    #[test]
    fn an_oversized_snapshot_is_refused_with_a_plain_message() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OUTPUT_REVIEWS_FILE_NAME);
        let mut store = OutputReviewStore::open(&path).unwrap();
        let error = store
            .capture(image("huge.png", &vec![0; MAX_SNAPSHOT_BYTES + 1]), 1)
            .unwrap_err();
        assert!(
            error.to_string().contains("larger than the 5 MB"),
            "{error}"
        );
        assert!(snapshot_files(temp.path()).is_empty());
        for i in 0..MAX_REVIEWS {
            store.capture(snapshot(&format!("v{i}")), 2).unwrap();
        }
        let error = store.capture(snapshot("one more"), 3).unwrap_err();
        assert!(error.to_string().contains("32 output versions"), "{error}");
        assert_eq!(snapshot_files(temp.path()).len(), MAX_REVIEWS);
    }
}
