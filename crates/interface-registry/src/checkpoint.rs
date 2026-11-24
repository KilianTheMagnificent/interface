use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use chrono::DateTime;
use interface_common::paths::{BINARY_EXTENSIONS, SENSITIVE_DIRECTORIES};
use interface_storage::{
    JsonLimits, PrivateDirectory, StorageError, WriterLock, decode_json_strict,
    ensure_current_revision, read_bounded,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{RegistryError, Result};

pub const TURN_CHECKPOINT_RETENTION: usize = 40;
const CHECKPOINT_LOCK: &str = ".checkpoint-writer.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointLimits {
    pub max_files: usize,
    pub max_input_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub max_manifest_bytes: usize,
    pub max_listed_checkpoints: usize,
}

impl Default for CheckpointLimits {
    fn default() -> Self {
        Self {
            max_files: 200,
            max_input_files: 500,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 32 * 1024 * 1024,
            max_manifest_bytes: 512 * 1024,
            max_listed_checkpoints: 100,
        }
    }
}

impl CheckpointLimits {
    fn validate(self) -> Result<Self> {
        let defaults = Self::default();
        if self.max_files == 0
            || self.max_files > defaults.max_files
            || self.max_input_files == 0
            || self.max_input_files > defaults.max_input_files
            || self.max_file_bytes == 0
            || self.max_file_bytes > defaults.max_file_bytes
            || self.max_total_bytes == 0
            || self.max_total_bytes > defaults.max_total_bytes
            || self.max_manifest_bytes == 0
            || self.max_manifest_bytes > defaults.max_manifest_bytes
            || self.max_listed_checkpoints == 0
            || self.max_listed_checkpoints > defaults.max_listed_checkpoints
        {
            return Err(RegistryError::Invalid(
                "checkpoint limits must be positive and no broader than the safety defaults".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointKind {
    Review,
    Recovery,
    Turn,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointOmissionReason {
    Credential,
    Binary,
    Symlink,
    NotRegular,
    UnsafePath,
    OutputLimit,
    GitError,
    Deleted,
    Changed,
    FileLimit,
    ByteLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFile {
    pub path: String,
    pub blob: String,
    pub bytes: u64,
    pub sha256: String,
    pub mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointOmission {
    pub path: String,
    pub reason: CheckpointOmissionReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointManifest {
    pub version: u8,
    pub id: String,
    pub kind: CheckpointKind,
    pub created_at: String,
    pub repository_root: PathBuf,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub repository_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_checkpoint_id: Option<String>,
    pub files: Vec<CheckpointFile>,
    pub omitted: Vec<CheckpointOmission>,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointSummary {
    pub id: String,
    pub kind: CheckpointKind,
    pub created_at: String,
    pub repository_root: PathBuf,
    pub repository_head: Option<String>,
    pub source_checkpoint_id: Option<String>,
    pub file_count: usize,
    pub omitted_count: usize,
    pub total_bytes: u64,
}

impl From<&CheckpointManifest> for CheckpointSummary {
    fn from(manifest: &CheckpointManifest) -> Self {
        Self {
            id: manifest.id.clone(),
            kind: manifest.kind,
            created_at: manifest.created_at.clone(),
            repository_root: manifest.repository_root.clone(),
            repository_head: manifest.repository_head.clone(),
            source_checkpoint_id: manifest.source_checkpoint_id.clone(),
            file_count: manifest.files.len(),
            omitted_count: manifest.omitted.len(),
            total_bytes: manifest.total_bytes,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointInputFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointDraft {
    pub id: String,
    pub kind: CheckpointKind,
    pub created_at: String,
    pub repository_root: PathBuf,
    pub repository_head: Option<String>,
    pub source_checkpoint_id: Option<String>,
    pub files: Vec<CheckpointInputFile>,
    pub omitted: Vec<CheckpointOmission>,
}

#[derive(Debug)]
pub struct CheckpointStore {
    directory: PrivateDirectory,
    lock: WriterLock,
    limits: CheckpointLimits,
    revision: u64,
    last_pruned_total: usize,
}

impl CheckpointStore {
    /// Opens or creates an owner-private checkpoint store under one writer lock.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe storage, invalid limits, or a held lock.
    pub fn open(directory: impl AsRef<Path>, limits: CheckpointLimits) -> Result<Self> {
        let limits = limits.validate()?;
        let directory = PrivateDirectory::open_or_create(directory)?;
        let lock = WriterLock::acquire(&directory, CHECKPOINT_LOCK)?;
        Ok(Self {
            directory,
            lock,
            limits,
            revision: 1,
            last_pruned_total: 0,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Publishes a fully synced checkpoint directory after validating every
    /// path, omission, byte ceiling, and manifest field.
    ///
    /// Durability order: every blob, `manifest.json`, `journal.json` and the
    /// `blobs` directory get a plain `fsync(2)`, which hands their data and
    /// metadata to the device; then one `F_FULLFSYNC` on the staging directory
    /// drains the device's write cache, persisting all of them before the
    /// rename publishes the manifest; then the store directory is synced so
    /// the publication itself is durable before this returns.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale revision, invalid input, unsafe storage,
    /// external drift, or an unproven publication.
    pub fn create(
        &mut self,
        expected_revision: u64,
        draft: CheckpointDraft,
    ) -> Result<CheckpointManifest> {
        self.create_durably(expected_revision, draft, &mut DeviceDurability)
    }

    fn create_durably(
        &mut self,
        expected_revision: u64,
        draft: CheckpointDraft,
        durability: &mut impl CheckpointDurability,
    ) -> Result<CheckpointManifest> {
        ensure_current_revision(expected_revision, self.revision)?;
        self.lock.assert_held_for(&self.directory)?;
        validate_draft_header(&draft)?;
        if draft.files.len() + draft.omitted.len() > self.limits.max_input_files {
            return Err(RegistryError::Invalid(
                "checkpoint input exceeds the path-count limit".into(),
            ));
        }
        let prepared = prepare_checkpoint_files(draft.files, draft.omitted, self.limits)?;
        let files = prepared
            .files
            .iter()
            .enumerate()
            .map(|(index, input)| CheckpointFile {
                path: input.path.clone(),
                blob: format!("{:06}", index + 1),
                bytes: u64::try_from(input.content.len()).unwrap_or(u64::MAX),
                sha256: format!("{:x}", Sha256::digest(&input.content)),
                mode: input.mode & 0o777,
            })
            .collect();

        let manifest = CheckpointManifest {
            version: 1,
            id: draft.id,
            kind: draft.kind,
            created_at: draft.created_at,
            repository_root: draft.repository_root,
            repository_head: draft.repository_head,
            source_checkpoint_id: draft.source_checkpoint_id,
            files,
            omitted: prepared.omitted,
            total_bytes: u64::try_from(prepared.total_bytes).unwrap_or(u64::MAX),
        };
        validate_manifest(&manifest, &manifest.id, self.limits)?;
        if manifest.kind == CheckpointKind::Turn && manifest.files.is_empty() {
            return Ok(manifest);
        }
        let manifest_bytes = json_line(&manifest)?;
        if manifest_bytes.len() > self.limits.max_manifest_bytes {
            return Err(RegistryError::Invalid(
                "checkpoint manifest exceeds its safety limit".into(),
            ));
        }
        assert_missing_entry(&self.directory, &manifest.id)?;
        let staging_name = format!(".{}.{}.tmp", manifest.id, Uuid::new_v4());
        // Neither the staging tree nor its `blobs` directory needs durable
        // membership until the barrier below: nothing references them before
        // the rename, and a failure removes them.
        let staging = self.directory.create_unpublished_child(&staging_name)?;
        let destination = self.directory.path().join(&manifest.id);
        let staged = (|| -> Result<()> {
            let blobs = staging.create_unpublished_child("blobs")?;
            for (input, entry) in prepared.files.iter().zip(&manifest.files) {
                write_private_file(&blobs, &entry.blob, &input.content, durability)?;
            }
            write_private_file(&staging, "manifest.json", &manifest_bytes, durability)?;
            let journal = CheckpointJournal {
                version: 1,
                checkpoint_id: &manifest.id,
                state: "committed",
                committed_at: &manifest.created_at,
            };
            write_private_file(&staging, "journal.json", &json_line(&journal)?, durability)?;
            hand_off_directory(&blobs, durability)?;
            // The one drive-cache barrier before publication: it syncs the
            // staging directory's entries and persists every hand-off above,
            // so no rename can publish a manifest whose blobs are not durable.
            durability.flush(&staging)?;
            self.lock.assert_held_for(&self.directory)?;
            self.directory.assert_unchanged()?;
            assert_missing_entry(&self.directory, &manifest.id)?;
            fs::rename(staging.path(), &destination).map_err(|error| {
                RegistryError::io("publish checkpoint directory", &destination, error)
            })
        })();
        if let Err(error) = staged {
            // Nothing else lists or reaps `.{id}.{uuid}.tmp`, so an unpublished
            // staging tree would keep its blob bytes on disk forever.
            let _ = fs::remove_dir_all(staging.path());
            return Err(error);
        }
        let publication = (|| -> Result<()> {
            durability.flush(&self.directory)?;
            let committed = self.directory.open_child(&manifest.id)?;
            committed.assert_unchanged()?;
            self.lock.assert_held_for(&self.directory)?;
            Ok(())
        })();
        if let Err(error) = publication {
            return Err(StorageError::WriteUncertain {
                path: destination,
                reason: error.to_string(),
            }
            .into());
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| RegistryError::Invalid("checkpoint revision is exhausted".into()))?;
        if manifest.kind == CheckpointKind::Turn {
            self.prune_turn_checkpoints(&manifest.repository_root);
        }
        Ok(manifest)
    }

    fn prune_turn_checkpoints(&mut self, repository_root: &Path) {
        let _ = self.try_prune_turn_checkpoints(repository_root);
    }

    /// Removes this repository's automatic turn checkpoints past the newest
    /// `TURN_CHECKPOINT_RETENTION`. Best effort by design: a checkpoint that
    /// will not delete is left where it is, because failing the capture that
    /// just succeeded would lose the user a restore point over a housekeeping
    /// error.
    fn try_prune_turn_checkpoints(&mut self, repository_root: &Path) -> Result<()> {
        self.lock.assert_held_for(&self.directory)?;
        let names = read_directory_names(&self.directory)?;
        let count = names
            .iter()
            .filter(|name| valid_checkpoint_id(name))
            .count();
        // THE CHEAP GATE, and the reason it is not exact. Deciding what belongs
        // to THIS repository means reading every manifest in a store shared by
        // all of them — per prompt, on a machine that is also running an agent.
        // Counting directory entries costs one readdir and no parsing, so the
        // scan is paid for only once the store as a whole could plausibly hold
        // an over-quota repository. Retention is therefore enforced with
        // headroom rather than on every send.
        if count <= TURN_CHECKPOINT_RETENTION.saturating_mul(2) {
            return Ok(());
        }
        // The store-wide count is what the gate can see cheaply, but the scan is
        // root-scoped: several repositories each under their own ceiling hold
        // the gate open forever while every scan frees nothing. So a scan also
        // records the total it left behind, and the next one waits for another
        // retention's worth of growth. The root that triggers a scan is cut back
        // to its ceiling; a root that never triggers one is bounded only in
        // expectation. What this removes is the per-send scan.
        if count
            < self
                .last_pruned_total
                .saturating_add(TURN_CHECKPOINT_RETENTION)
        {
            return Ok(());
        }
        let stale = list_turn_ids_for(&self.directory, self.limits, repository_root)?;
        let mut removed = 0usize;
        for id in stale.into_iter().skip(TURN_CHECKPOINT_RETENTION) {
            if remove_turn_checkpoint(&self.directory, &id).is_ok() {
                removed += 1;
            }
        }
        self.last_pruned_total = count.saturating_sub(removed);
        Ok(())
    }

    /// Lists newest valid committed checkpoints, optionally scoped to a root.
    ///
    /// # Errors
    ///
    /// Returns an error when the store directory itself is unsafe or changes.
    pub fn list(&self, repository_root: Option<&Path>) -> Result<Vec<CheckpointSummary>> {
        list_from_directory(&self.directory, self.limits, repository_root)
    }

    /// Loads and validates one bounded manifest without opening its blobs.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid ID, unsafe entry, or invalid manifest.
    pub fn inspect(&self, id: &str) -> Result<CheckpointManifest> {
        load_checkpoint(&self.directory, id, self.limits)
    }

    /// Loads one blob only after revalidating its manifest membership and hash.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe storage or a missing, changed, or corrupt blob.
    pub fn read_blob(
        &self,
        manifest: &CheckpointManifest,
        file: &CheckpointFile,
    ) -> Result<Vec<u8>> {
        read_checkpoint_blob(&self.directory, manifest, file, self.limits)
    }
}

/// Opened checkpoint blob directories reused across a restore loop.
pub struct CheckpointBlobReader {
    store: PrivateDirectory,
    checkpoint: PrivateDirectory,
    blobs: PrivateDirectory,
    limits: CheckpointLimits,
    members: HashSet<String>,
}

impl CheckpointBlobReader {
    /// Opens the store, checkpoint, and blobs directories once for a restore.
    ///
    /// Validates the manifest once here and binds the reader to it, so the
    /// per-blob membership guard `read_checkpoint_blob` applies survives the
    /// loop without re-walking the manifest for every file.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid manifest or unsafe storage.
    pub fn open_read_only(
        directory: impl AsRef<Path>,
        manifest: &CheckpointManifest,
        limits: CheckpointLimits,
    ) -> Result<Self> {
        let limits = limits.validate()?;
        validate_manifest(manifest, &manifest.id, limits)?;
        let store = PrivateDirectory::open(directory)?;
        let checkpoint = store.open_child(&manifest.id)?;
        let blobs = checkpoint.open_child("blobs")?;
        Ok(Self {
            store,
            checkpoint,
            blobs,
            limits,
            members: manifest
                .files
                .iter()
                .map(|file| file.blob.clone())
                .collect(),
        })
    }

    /// Reads one verified blob without re-walking the manifest.
    ///
    /// # Errors
    ///
    /// Returns an error for an entry outside this manifest, or a missing,
    /// changed, or corrupt blob.
    pub fn read(&self, file: &CheckpointFile) -> Result<Vec<u8>> {
        if !self.members.contains(&file.blob) {
            return Err(RegistryError::Invalid(
                "blob entry does not belong to this manifest".into(),
            ));
        }
        read_opened_checkpoint_blob(
            &self.store,
            &self.checkpoint,
            &self.blobs,
            file,
            self.limits,
        )
    }
}

struct PreparedCheckpointFile {
    path: String,
    content: Vec<u8>,
    mode: u32,
}

struct PreparedCheckpoint {
    files: Vec<PreparedCheckpointFile>,
    omitted: Vec<CheckpointOmission>,
    total_bytes: usize,
}

fn prepare_checkpoint_files(
    inputs: Vec<CheckpointInputFile>,
    omissions: Vec<CheckpointOmission>,
    limits: CheckpointLimits,
) -> Result<PreparedCheckpoint> {
    validate_omissions(&omissions)?;
    let mut files = Vec::new();
    let mut omitted = omissions;
    let mut seen = HashSet::new();
    let mut total_bytes = 0usize;
    for input in inputs {
        let (path, reason) = match safe_relative(&input.path) {
            None => (
                bounded_unsafe_path(&input.path),
                Some(CheckpointOmissionReason::UnsafePath),
            ),
            Some(path) if is_credential_path(&path) => {
                (path, Some(CheckpointOmissionReason::Credential))
            }
            Some(path) if is_binary_path(&path) || !is_utf8_text(&input.content) => {
                (path, Some(CheckpointOmissionReason::Binary))
            }
            Some(path) if !seen.insert(path.clone()) => {
                (path, Some(CheckpointOmissionReason::Changed))
            }
            Some(path) if files.len() >= limits.max_files => {
                (path, Some(CheckpointOmissionReason::FileLimit))
            }
            Some(path)
                if input.content.len() > limits.max_file_bytes
                    || total_bytes
                        .checked_add(input.content.len())
                        .is_none_or(|value| value > limits.max_total_bytes) =>
            {
                (path, Some(CheckpointOmissionReason::ByteLimit))
            }
            Some(path) => (path, None),
        };
        if let Some(reason) = reason {
            omitted.push(CheckpointOmission { path, reason });
            continue;
        }
        total_bytes += input.content.len();
        files.push(PreparedCheckpointFile {
            path,
            content: input.content,
            mode: input.mode,
        });
    }
    Ok(PreparedCheckpoint {
        files,
        omitted,
        total_bytes,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointJournal<'a> {
    version: u8,
    checkpoint_id: &'a str,
    state: &'static str,
    committed_at: &'a str,
}

fn deserialize_required_nullable<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

/// Lists checkpoints without creating, locking, repairing, or rewriting state.
///
/// # Errors
///
/// Returns an error when an existing store directory is unsafe or changes.
pub fn list_checkpoints_read_only(
    directory: impl AsRef<Path>,
    limits: CheckpointLimits,
    repository_root: Option<&Path>,
) -> Result<Vec<CheckpointSummary>> {
    let limits = limits.validate()?;
    let directory = match PrivateDirectory::open(directory) {
        Ok(directory) => directory,
        Err(StorageError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    list_from_directory(&directory, limits, repository_root)
}

/// Inspects one checkpoint without creating, locking, repairing, or rewriting.
///
/// # Errors
///
/// Returns an error for a missing store, invalid ID, unsafe entry, or invalid manifest.
pub fn inspect_checkpoint_read_only(
    directory: impl AsRef<Path>,
    id: &str,
    limits: CheckpointLimits,
) -> Result<CheckpointManifest> {
    let directory = PrivateDirectory::open(directory)?;
    load_checkpoint(&directory, id, limits.validate()?)
}

/// Reads one verified checkpoint blob without mutating storage.
///
/// # Errors
///
/// Returns an error for unsafe storage or a missing, changed, or corrupt blob.
pub fn read_checkpoint_blob_read_only(
    directory: impl AsRef<Path>,
    manifest: &CheckpointManifest,
    file: &CheckpointFile,
    limits: CheckpointLimits,
) -> Result<Vec<u8>> {
    let directory = PrivateDirectory::open(directory)?;
    read_checkpoint_blob(&directory, manifest, file, limits.validate()?)
}

fn list_from_directory(
    directory: &PrivateDirectory,
    limits: CheckpointLimits,
    repository_root: Option<&Path>,
) -> Result<Vec<CheckpointSummary>> {
    let mut names = read_directory_names(directory)?
        .into_iter()
        .filter(|name| valid_checkpoint_id(name))
        .collect::<Vec<_>>();
    names.sort_by(|left, right| right.cmp(left));
    let mut summaries = Vec::new();
    for id in names {
        if summaries.len() >= limits.max_listed_checkpoints {
            break;
        }
        let Ok(manifest) = load_checkpoint(directory, &id, limits) else {
            continue;
        };
        if repository_root.is_some_and(|root| manifest.repository_root != root) {
            continue;
        }
        summaries.push(CheckpointSummary::from(&manifest));
    }
    summaries.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(summaries)
}

/// One root's turn checkpoint ids, newest first, with no display cap.
///
/// The prune must see every turn a root owns: capped at
/// `max_listed_checkpoints` like [`list_from_directory`], a root holding that
/// many newer non-turn checkpoints would never show a turn again and would stop
/// pruning for good.
fn list_turn_ids_for(
    directory: &PrivateDirectory,
    limits: CheckpointLimits,
    repository_root: &Path,
) -> Result<Vec<String>> {
    let mut names = read_directory_names(directory)?
        .into_iter()
        .filter(|name| valid_checkpoint_id(name))
        .collect::<Vec<_>>();
    names.sort_by(|left, right| right.cmp(left));
    let mut turns = Vec::new();
    for id in names {
        let Ok(manifest) = load_checkpoint(directory, &id, limits) else {
            continue;
        };
        if manifest.kind != CheckpointKind::Turn || manifest.repository_root != repository_root {
            continue;
        }
        turns.push((manifest.created_at, manifest.id));
    }
    turns.sort_by(|left, right| right.cmp(left));
    Ok(turns.into_iter().map(|(_, id)| id).collect())
}

fn load_checkpoint(
    store: &PrivateDirectory,
    id: &str,
    limits: CheckpointLimits,
) -> Result<CheckpointManifest> {
    if !valid_checkpoint_id(id) {
        return Err(RegistryError::Invalid(
            "invalid checkpoint identifier".into(),
        ));
    }
    let checkpoint = store.open_child(id)?;
    let Some(file) = read_bounded(&checkpoint, "manifest.json", limits.max_manifest_bytes)? else {
        return Err(RegistryError::Checkpoint {
            path: checkpoint.path().join("manifest.json"),
            reason: "manifest is missing".into(),
        });
    };
    if !is_utf8_text(file.bytes()) {
        return Err(RegistryError::Checkpoint {
            path: checkpoint.path().join("manifest.json"),
            reason: "manifest is not UTF-8 text".into(),
        });
    }
    let manifest: CheckpointManifest = decode_json_strict(
        file.bytes(),
        manifest_limits(limits)?,
        checkpoint.path().join("manifest.json"),
    )?;
    validate_manifest(&manifest, id, limits)?;
    checkpoint.assert_unchanged()?;
    store.assert_unchanged()?;
    Ok(manifest)
}

fn read_checkpoint_blob(
    store: &PrivateDirectory,
    manifest: &CheckpointManifest,
    file: &CheckpointFile,
    limits: CheckpointLimits,
) -> Result<Vec<u8>> {
    validate_manifest(manifest, &manifest.id, limits)?;
    if !manifest.files.iter().any(|candidate| candidate == file) {
        return Err(RegistryError::Invalid(
            "blob entry does not belong to this manifest".into(),
        ));
    }
    let checkpoint = store.open_child(&manifest.id)?;
    let blobs = checkpoint.open_child("blobs")?;
    read_opened_checkpoint_blob(store, &checkpoint, &blobs, file, limits)
}

fn read_opened_checkpoint_blob(
    store: &PrivateDirectory,
    checkpoint: &PrivateDirectory,
    blobs: &PrivateDirectory,
    file: &CheckpointFile,
    limits: CheckpointLimits,
) -> Result<Vec<u8>> {
    let max_bytes = usize::try_from(file.bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(1)
        .min(limits.max_file_bytes);
    let Some(blob) = read_bounded(blobs, &file.blob, max_bytes)? else {
        return Err(RegistryError::Checkpoint {
            path: blobs.path().join(&file.blob),
            reason: "checkpoint-corrupt".into(),
        });
    };
    let content = blob.bytes();
    // The bounded read already hashed exactly these bytes.
    if u64::try_from(content.len()).unwrap_or(u64::MAX) != file.bytes
        || !is_lower_hex_of(&file.sha256, blob.snapshot().sha256())
        || !is_utf8_text(content)
    {
        return Err(RegistryError::Checkpoint {
            path: blobs.path().join(&file.blob),
            reason: "checkpoint-corrupt".into(),
        });
    }
    blobs.assert_unchanged()?;
    checkpoint.assert_unchanged()?;
    store.assert_unchanged()?;
    let (bytes, _) = blob.into_parts();
    Ok(bytes)
}

/// Whether `hex` is exactly the lowercase hex spelling of `digest`, the form
/// manifests record (`format!("{:x}", Sha256::digest(..))`).
fn is_lower_hex_of(hex: &str, digest: &[u8; 32]) -> bool {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    hex.len() == 64
        && hex
            .as_bytes()
            .chunks_exact(2)
            .zip(digest)
            .all(|(pair, byte)| {
                pair[0] == DIGITS[usize::from(byte >> 4)]
                    && pair[1] == DIGITS[usize::from(byte & 0x0f)]
            })
}

fn remove_turn_checkpoint(store: &PrivateDirectory, id: &str) -> Result<()> {
    if !valid_checkpoint_id(id) {
        return Ok(());
    }
    let checkpoint = store.open_child(id)?;
    store.assert_unchanged()?;
    checkpoint.assert_unchanged()?;
    fs::remove_dir_all(checkpoint.path()).map_err(|error| {
        RegistryError::io("remove stale turn checkpoint", checkpoint.path(), error)
    })?;
    Ok(())
}

fn validate_draft_header(draft: &CheckpointDraft) -> Result<()> {
    if !valid_checkpoint_id(&draft.id)
        || !draft.repository_root.is_absolute()
        || DateTime::parse_from_rfc3339(&draft.created_at).is_err()
        || draft
            .source_checkpoint_id
            .as_ref()
            .is_some_and(|id| !valid_checkpoint_id(id))
    {
        return Err(RegistryError::Invalid(
            "checkpoint draft has an invalid identifier, timestamp, or repository root".into(),
        ));
    }
    Ok(())
}

fn validate_manifest(
    manifest: &CheckpointManifest,
    expected_id: &str,
    limits: CheckpointLimits,
) -> Result<()> {
    if manifest.version != 1
        || manifest.id != expected_id
        || !valid_checkpoint_id(&manifest.id)
        || DateTime::parse_from_rfc3339(&manifest.created_at).is_err()
        || !manifest.repository_root.is_absolute()
        || manifest
            .source_checkpoint_id
            .as_ref()
            .is_some_and(|id| !valid_checkpoint_id(id))
        || manifest.files.len() > limits.max_files
        || manifest.files.len() + manifest.omitted.len() > limits.max_input_files
    {
        return Err(RegistryError::Invalid("invalid checkpoint manifest".into()));
    }
    let mut paths = HashSet::new();
    let mut blobs = HashSet::new();
    let mut total = 0u64;
    for file in &manifest.files {
        let Some(relative) = safe_relative(&file.path) else {
            return Err(RegistryError::Invalid("invalid checkpoint manifest".into()));
        };
        if is_credential_path(&relative)
            || is_binary_path(&relative)
            || file.blob.len() != 6
            || !file.blob.bytes().all(|byte| byte.is_ascii_digit())
            || usize::try_from(file.bytes)
                .ok()
                .is_none_or(|bytes| bytes > limits.max_file_bytes)
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || file.mode > 0o777
            || !paths.insert(relative)
            || !blobs.insert(file.blob.clone())
        {
            return Err(RegistryError::Invalid("invalid checkpoint manifest".into()));
        }
        total = total
            .checked_add(file.bytes)
            .ok_or_else(|| RegistryError::Invalid("invalid checkpoint manifest".into()))?;
    }
    if total != manifest.total_bytes
        || usize::try_from(total)
            .ok()
            .is_none_or(|bytes| bytes > limits.max_total_bytes)
    {
        return Err(RegistryError::Invalid("invalid checkpoint manifest".into()));
    }
    validate_omissions(&manifest.omitted)?;
    Ok(())
}

fn validate_omissions(omissions: &[CheckpointOmission]) -> Result<()> {
    for omission in omissions {
        let safe = safe_relative(&omission.path).is_some();
        let bounded_unsafe = omission.reason == CheckpointOmissionReason::UnsafePath
            && !omission.path.is_empty()
            && omission.path.len() <= 4_096
            && !omission.path.contains('\0')
            && !omission.path.contains(['\r', '\n']);
        if !safe && !bounded_unsafe {
            return Err(RegistryError::Invalid("invalid checkpoint omission".into()));
        }
    }
    Ok(())
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn valid_checkpoint_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    id.len() <= 128
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && id != "."
        && id != ".."
        && !id.ends_with(".tmp")
}

fn safe_relative(raw: &str) -> Option<String> {
    if raw.is_empty()
        || raw.len() > 4_096
        || raw.contains('\0')
        || raw.contains(['\r', '\n'])
        || raw.starts_with('/')
        || raw.contains('\\')
    {
        return None;
    }
    if raw
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    Some(raw.to_owned())
}

#[must_use]
pub fn bounded_unsafe_path(raw: &str) -> String {
    raw.chars()
        .take(4_096)
        .filter(|character| !matches!(character, '\0' | '\r' | '\n'))
        .collect()
}

#[must_use]
pub fn is_utf8_text(content: &[u8]) -> bool {
    !content.contains(&0) && std::str::from_utf8(content).is_ok()
}

// Diverges from `interface_workspace::is_likely_binary_path`, which lowercases the
// extension with `to_lowercase`; checkpoints fold ASCII only. Same table.
fn is_binary_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            BINARY_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

// Diverges from `interface_workspace::is_search_excluded_path`: no generated-path
// rules, hand-rolled backup-suffix stripping instead of regexes, and no
// `.docker/config.json` / `.aws/credentials` special cases. Only the directory
// table is shared, and which paths a checkpoint omits is observable.
fn is_credential_path(path: &str) -> bool {
    path.split('/').any(|segment| {
        let stem = credential_stem(segment.to_ascii_lowercase());
        SENSITIVE_DIRECTORIES.contains(&stem.as_str()) || credential_leaf(&stem)
    })
}

fn credential_stem(mut stem: String) -> String {
    const SUFFIXES: &[&str] = &[
        ".bak", ".backup", ".old", ".orig", ".save", ".tmp", ".copy", ".swp", ".swo", ".swx",
        ".rej", "~",
    ];
    loop {
        let before = stem.len();
        if let Some(suffix) = SUFFIXES.iter().find(|suffix| stem.ends_with(**suffix)) {
            stem.truncate(stem.len() - suffix.len());
        } else if stem
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix.bytes().all(|byte| byte.is_ascii_digit()))
        {
            stem.truncate(stem.rfind('.').unwrap_or(stem.len()));
        }
        if stem.len() == before {
            return stem;
        }
    }
}

fn credential_leaf(name: &str) -> bool {
    const EXACT: &[&str] = &[
        ".envrc",
        ".git-credentials",
        ".netrc",
        ".npmrc",
        ".pypirc",
        "auth.json",
        "credentials",
        "credentials.json",
        "application_default_credentials.json",
        "phone-connection.v1.json",
        ".pgpass",
        "pgpass.conf",
        ".my.cnf",
        "my.cnf",
    ];
    if EXACT.contains(&name) || name.starts_with(".env") || name.contains(".env.") {
        return true;
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if ["key", "pem", "p12", "pfx", "jks", "keystore", "ppk"].contains(&extension) {
        return true;
    }
    let json_secret = extension == "json"
        && (name.contains("client_secret")
            || name.contains("client-secret")
            || name.contains("service_account")
            || name.contains("service-account"));
    let credential_manifest = ["json", "yaml", "yml", "ini"].contains(&extension)
        && (name.starts_with("credential") || name.contains("credentials."));
    let private_key_name = name.starts_with("id_rsa")
        || name.starts_with("id_dsa")
        || name.starts_with("id_ecdsa")
        || name.starts_with("id_ed25519")
        || (name.starts_with("ssh_host_") && name.contains("_key"))
        || name.starts_with("private-key")
        || name.starts_with("private_key")
        || name == "identity";
    json_secret
        || credential_manifest
        || private_key_name
        || name == "secret"
        || name == "secrets"
        || name.starts_with("secret.")
        || name.starts_with("secrets.")
}

fn json_line<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| RegistryError::Invalid(format!("invalid checkpoint JSON: {error}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn manifest_limits(limits: CheckpointLimits) -> Result<JsonLimits> {
    Ok(JsonLimits::new(
        limits.max_manifest_bytes,
        32,
        limits.max_input_files.saturating_mul(16).max(1_000),
    )?)
}

/// How a checkpoint's staged entries reach stable storage. Production uses
/// [`DeviceDurability`]; tests substitute a recorder that can fail a step.
trait CheckpointDurability {
    /// Hands one staged file's or directory's dirty data and metadata to the
    /// device. It need not be persistent until the next [`Self::flush`].
    fn hand_off(&mut self, operation: &'static str, fd: BorrowedFd<'_>, path: &Path) -> Result<()>;

    /// Syncs `directory` and persists every earlier hand-off on its device.
    fn flush(&mut self, directory: &PrivateDirectory) -> Result<()>;
}

/// `hand_off` is a plain `fsync(2)`: on Apple platforms it moves the data to
/// the drive without draining the drive's write cache. `flush` is std's
/// `sync_all`, which is `F_FULLFSYNC` there: fcntl(2) documents it as a
/// barrier after which "data that had been fsync'd on the same device before
/// is guaranteed to be persisted". A checkpoint's staging tree sits inside
/// the store directory it is renamed within, so it is always one device.
/// Elsewhere both calls are `fsync(2)`, exactly what this code did before.
struct DeviceDurability;

impl CheckpointDurability for DeviceDurability {
    fn hand_off(&mut self, operation: &'static str, fd: BorrowedFd<'_>, path: &Path) -> Result<()> {
        loop {
            match rustix::fs::fsync(fd) {
                // std's sync_all retries an interrupted call; so does this.
                Err(rustix::io::Errno::INTR) => {}
                result => {
                    return result
                        .map_err(|error| RegistryError::io(operation, path, error.into()));
                }
            }
        }
    }

    fn flush(&mut self, directory: &PrivateDirectory) -> Result<()> {
        Ok(directory.sync()?)
    }
}

fn hand_off_directory(
    directory: &PrivateDirectory,
    durability: &mut impl CheckpointDurability,
) -> Result<()> {
    directory.assert_unchanged()?;
    durability.hand_off("fsync directory", directory.as_fd(), directory.path())?;
    directory.assert_unchanged()?;
    Ok(())
}

fn write_private_file(
    directory: &PrivateDirectory,
    name: &str,
    bytes: &[u8],
    durability: &mut impl CheckpointDurability,
) -> Result<()> {
    directory.assert_unchanged()?;
    let path = private_entry_path(directory, name)?;
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    configure_private_create(&mut options);
    let mut file = options
        .open(&path)
        .map_err(|error| RegistryError::io("create private checkpoint file", &path, error))?;
    set_private_permissions(&file, &path)?;
    file.write_all(bytes)
        .map_err(|error| RegistryError::io("write private checkpoint file", &path, error))?;
    durability.hand_off("fsync private checkpoint file", file.as_fd(), &path)?;
    verify_private_file(directory, &path, &file, bytes.len())?;
    drop(file);
    directory.assert_unchanged()?;
    Ok(())
}

fn configure_private_create(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options
        .mode(interface_storage::OWNER_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

fn set_private_permissions(file: &File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(
        interface_storage::OWNER_FILE_MODE,
    ))
    .map_err(|error| RegistryError::io("set private checkpoint permissions on", path, error))?;
    Ok(())
}

fn verify_private_file(
    directory: &PrivateDirectory,
    path: &Path,
    file: &File,
    expected_len: usize,
) -> Result<()> {
    directory.assert_unchanged()?;
    let opened = file
        .metadata()
        .map_err(|error| RegistryError::io("inspect private checkpoint file", path, error))?;
    let named = fs::symlink_metadata(path)
        .map_err(|error| RegistryError::io("revalidate private checkpoint file", path, error))?;
    if !opened.is_file()
        || !named.file_type().is_file()
        || named.file_type().is_symlink()
        || opened.len() != u64::try_from(expected_len).unwrap_or(u64::MAX)
        || !same_file_identity(&opened, &named)
        || !private_file_metadata(&opened)
        || !private_file_metadata(&named)
    {
        return Err(RegistryError::Checkpoint {
            path: path.to_path_buf(),
            reason: "private file identity, type, size, links, or mode changed".into(),
        });
    }
    directory.assert_unchanged()?;
    Ok(())
}

fn read_directory_names(directory: &PrivateDirectory) -> Result<Vec<String>> {
    directory.assert_unchanged()?;
    let entries = fs::read_dir(directory.path()).map_err(|error| {
        RegistryError::io(
            "enumerate private checkpoint directory",
            directory.path(),
            error,
        )
    })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            RegistryError::io(
                "read private checkpoint directory entry",
                directory.path(),
                error,
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| RegistryError::Checkpoint {
                path: entry.path(),
                reason: "checkpoint entry name is not UTF-8".into(),
            })?;
        names.push(name);
    }
    directory.assert_unchanged()?;
    Ok(names)
}

fn assert_missing_entry(directory: &PrivateDirectory, name: &str) -> Result<()> {
    directory.assert_unchanged()?;
    let path = private_entry_path(directory, name)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(RegistryError::io(
                "inspect checkpoint publication target",
                path,
                error,
            ));
        }
        Ok(_) => {
            return Err(RegistryError::Invalid(
                "checkpoint identifier already exists".into(),
            ));
        }
    }
    directory.assert_unchanged()?;
    Ok(())
}

fn private_entry_path(directory: &PrivateDirectory, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains(['/', '\\', '\0', '\r', '\n'])
    {
        return Err(RegistryError::Invalid(
            "invalid private checkpoint entry name".into(),
        ));
    }
    Ok(directory.path().join(name))
}

fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn private_file_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o7777 == interface_storage::OWNER_FILE_MODE && metadata.nlink() == 1
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
