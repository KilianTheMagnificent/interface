use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use interface_common::sync::lock;
use interface_contracts::engine::EngineId;
use interface_contracts::{canonical_timestamp, parse_canonical_timestamp};
use interface_storage::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    argv::is_engine_id,
    sanitize::{for_display, is_display_safe},
};

pub const STEER_HISTORY_SNAPSHOT_VERSION: u8 = 1;
pub const MAX_STEER_HISTORY_ENTRIES: usize = 256;
pub const MAX_SESSION_STEER_HISTORY_ENTRIES: usize = 40;
pub const MAX_STEER_PROMPT_PREVIEW_CHARS: usize = 240;
pub const MAX_STEER_DETAIL_CHARS: usize = 512;

const MAX_STEER_ID_CHARS: usize = 128;
const MAX_SESSION_KEY_CHARS: usize = 512;
const MAX_MODEL_CHARS: usize = 256;
const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SteerOutcome {
    Pending,
    AdapterAccepted,
    DefinitelyUnsent,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SteerSettledOutcome {
    AdapterAccepted,
    DefinitelyUnsent,
    OutcomeUnknown,
}

impl From<SteerSettledOutcome> for SteerOutcome {
    fn from(value: SteerSettledOutcome) -> Self {
        match value {
            SteerSettledOutcome::AdapterAccepted => Self::AdapterAccepted,
            SteerSettledOutcome::DefinitelyUnsent => Self::DefinitelyUnsent,
            SteerSettledOutcome::OutcomeUnknown => Self::OutcomeUnknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerHistoryEntry {
    pub id: String,
    pub session_key: String,
    pub engine: EngineId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub requested_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    pub outcome: SteerOutcome,
    pub prompt_preview: String,
    pub prompt_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize)]
struct SteerHistorySnapshot<'a> {
    version: u8,
    entries: &'a [SteerHistoryEntry],
}

#[derive(Clone, Debug)]
pub struct SteerHistoryStore {
    file: Arc<PathBuf>,
    entries: Arc<Mutex<Vec<SteerHistoryEntry>>>,
}

impl SteerHistoryStore {
    /// Opens the exact legacy steer-history snapshot path without reading it.
    ///
    /// # Errors
    ///
    /// Returns an error when `file` is not absolute.
    pub fn new(file: impl Into<PathBuf>) -> Result<Self, String> {
        let file = file.into();
        if !file.is_absolute() {
            return Err("Steer history path must be absolute".into());
        }
        Ok(Self {
            file: Arc::new(file),
            entries: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Loads a safe snapshot, or quarantines rejected state and starts empty.
    /// Rejected bytes are never returned in the error or display surface.
    pub fn load(&self) {
        let parsed = match load_snapshot(&self.file) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                quarantine_corrupt(&self.file);
                Vec::new()
            }
            // A transient or environmental failure is not proof of rejected
            // state, so the snapshot is left exactly as it is.
            Err(_) => Vec::new(),
        };
        *lock(&self.entries) = parsed;
    }

    /// Durably writes a pending record before returning it to the caller.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input or an unproven durable atomic write.
    pub fn begin(
        &self,
        session_key: &str,
        engine: &EngineId,
        model: Option<&str>,
        prompt: &str,
        requested_at: Option<&str>,
    ) -> Result<SteerHistoryEntry, String> {
        if !safe_identity(session_key, MAX_SESSION_KEY_CHARS) || !is_engine_id(engine.as_str()) {
            return Err("Invalid steer history input".into());
        }
        let requested_at = requested_at.map_or_else(now, str::to_owned);
        if !is_timestamp(&requested_at) {
            return Err("Invalid steer history timestamp".into());
        }
        let model = model
            .map(|value| sanitized_bounded(value, MAX_MODEL_CHARS))
            .filter(|value| !value.is_empty());
        let entry = SteerHistoryEntry {
            id: format!("steer_{}", Uuid::new_v4()),
            session_key: session_key.to_owned(),
            engine: engine.clone(),
            model,
            requested_at,
            settled_at: None,
            outcome: SteerOutcome::Pending,
            prompt_preview: sanitized_bounded(prompt, MAX_STEER_PROMPT_PREVIEW_CHARS),
            prompt_digest: format!("{:x}", Sha256::digest(prompt.as_bytes())),
            detail: None,
        };
        let mut entries = lock(&self.entries);
        entries.push(entry.clone());
        let dropped = (entries.len() > MAX_STEER_HISTORY_ENTRIES).then(|| entries.remove(0));
        if let Err(error) = write_atomic(&self.file, &entries) {
            if let Some(dropped) = dropped {
                entries.insert(0, dropped);
            }
            let _ = entries.pop();
            return Err(error);
        }
        Ok(entry)
    }

    /// Durably settles an existing pending record. Repeated settlements are
    /// idempotent and return the already-settled row unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input or an unproven durable atomic write.
    pub fn settle(
        &self,
        id: &str,
        outcome: SteerSettledOutcome,
        settled_at: Option<&str>,
        detail: Option<&str>,
    ) -> Result<Option<SteerHistoryEntry>, String> {
        if !safe_identity(id, MAX_STEER_ID_CHARS) {
            return Err("Invalid steer history settlement".into());
        }
        let mut entries = lock(&self.entries);
        let Some(index) = entries.iter().position(|entry| entry.id == id) else {
            return Ok(None);
        };
        if entries[index].outcome != SteerOutcome::Pending {
            return Ok(Some(entries[index].clone()));
        }
        let settled_at = settled_at.map_or_else(now, str::to_owned);
        if !is_timestamp(&settled_at) {
            return Err("Invalid steer history timestamp".into());
        }
        let detail = detail
            .map(|value| sanitized_bounded(value, MAX_STEER_DETAIL_CHARS))
            .filter(|value| !value.is_empty());
        let saved = entries[index].clone();
        entries[index].settled_at = Some(settled_at);
        entries[index].outcome = outcome.into();
        entries[index].detail = detail;
        let settled = entries[index].clone();
        if let Err(error) = write_atomic(&self.file, &entries) {
            entries[index] = saved;
            return Err(error);
        }
        Ok(Some(settled))
    }

    /// Returns insertion-newest rows for one exact session, bounded to 40.
    #[must_use]
    pub fn list_for_session(
        &self,
        session_key: &str,
        requested_limit: usize,
    ) -> Vec<SteerHistoryEntry> {
        let limit = requested_limit.min(MAX_SESSION_STEER_HISTORY_ENTRIES);
        lock(&self.entries)
            .iter()
            .rev()
            .filter(|entry| entry.session_key == session_key)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn flush(&self) {
        drop(lock(&self.entries));
    }
}

fn load_snapshot(file: &Path) -> std::io::Result<Vec<SteerHistoryEntry>> {
    let named = fs::symlink_metadata(file)?;
    if !named.is_file() || named.nlink() != 1 || named.len() > MAX_SNAPSHOT_BYTES {
        return Err(invalid_snapshot());
    }
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut handle = options.open(file)?;
    let opened = handle.metadata()?;
    if !opened.is_file() || opened.nlink() != 1 || opened.len() > MAX_SNAPSHOT_BYTES {
        return Err(invalid_snapshot());
    }
    let mut source = String::new();
    handle.read_to_string(&mut source)?;
    let value = serde_json::from_str::<Value>(&source).map_err(|_| invalid_snapshot())?;
    let entries = parse_snapshot(&value).ok_or_else(invalid_snapshot)?;
    let parent = file.parent().ok_or_else(invalid_snapshot)?;
    // Read paths only re-assert the mode; refusing a symlinked parent here would
    // make persisted steer history read as empty.
    fs::set_permissions(parent, fs::Permissions::from_mode(OWNER_DIRECTORY_MODE))?;
    handle.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))?;
    Ok(entries)
}

fn parse_snapshot(value: &Value) -> Option<Vec<SteerHistoryEntry>> {
    let record = value.as_object()?;
    let version = record.get("version")?.as_f64()?;
    if version.to_bits() != f64::from(STEER_HISTORY_SNAPSHOT_VERSION).to_bits() {
        return None;
    }
    let raw_entries = record.get("entries")?.as_array()?;
    if raw_entries.len() > MAX_STEER_HISTORY_ENTRIES {
        return None;
    }
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(raw_entries.len());
    for value in raw_entries {
        let entry = parse_entry(value)?;
        if !ids.insert(entry.id.clone()) {
            return None;
        }
        entries.push(entry);
    }
    Some(entries)
}

fn parse_entry(value: &Value) -> Option<SteerHistoryEntry> {
    let record = value.as_object()?;
    let id = record.get("id")?.as_str()?;
    let session_key = record.get("sessionKey")?.as_str()?;
    let engine = record.get("engine")?.as_str()?;
    let requested_at = record.get("requestedAt")?.as_str()?;
    let outcome = parse_outcome(record.get("outcome")?.as_str()?)?;
    let prompt_preview = record.get("promptPreview")?.as_str()?;
    let prompt_digest = record.get("promptDigest")?.as_str()?;
    if !safe_identity(id, MAX_STEER_ID_CHARS)
        || !safe_identity(session_key, MAX_SESSION_KEY_CHARS)
        || !is_engine_id(engine)
        || !is_timestamp(requested_at)
        || !safe_display_text(prompt_preview, MAX_STEER_PROMPT_PREVIEW_CHARS)
        || prompt_digest.len() != 64
        || !prompt_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let model = match record.get("model") {
        None => None,
        Some(value) => Some(value.as_str()?),
    };
    if model.is_some_and(|value| value.is_empty() || !safe_display_text(value, MAX_MODEL_CHARS)) {
        return None;
    }
    let settled_at = match record.get("settledAt") {
        None => None,
        Some(value) => Some(value.as_str()?),
    };
    let detail = match record.get("detail") {
        None => None,
        Some(value) => Some(value.as_str()?),
    };
    if detail.is_some_and(|value| !safe_display_text(value, MAX_STEER_DETAIL_CHARS)) {
        return None;
    }
    match outcome {
        SteerOutcome::Pending if settled_at.is_some() || detail.is_some() => return None,
        SteerOutcome::Pending => {}
        _ if settled_at.is_none_or(|value| !is_timestamp(value)) => return None,
        _ => {}
    }
    Some(SteerHistoryEntry {
        id: id.to_owned(),
        session_key: session_key.to_owned(),
        engine: EngineId::new(engine).ok()?,
        model: model.map(str::to_owned),
        requested_at: requested_at.to_owned(),
        settled_at: settled_at.map(str::to_owned),
        outcome,
        prompt_preview: prompt_preview.to_owned(),
        prompt_digest: prompt_digest.to_owned(),
        detail: detail.map(str::to_owned),
    })
}

fn parse_outcome(value: &str) -> Option<SteerOutcome> {
    match value {
        "pending" => Some(SteerOutcome::Pending),
        "adapter-accepted" => Some(SteerOutcome::AdapterAccepted),
        "definitely-unsent" => Some(SteerOutcome::DefinitelyUnsent),
        "outcome-unknown" => Some(SteerOutcome::OutcomeUnknown),
        _ => None,
    }
}

fn safe_identity(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= maximum
        && !value
            .chars()
            .any(|character| matches!(character as u32, 0..=0x1f | 0x7f..=0x9f))
        && is_display_safe(value)
}

fn safe_display_text(value: &str, maximum: usize) -> bool {
    value.chars().count() <= maximum && is_display_safe(value)
}

fn sanitized_bounded(value: &str, maximum: usize) -> String {
    for_display(value).chars().take(maximum).collect()
}

fn is_timestamp(value: &str) -> bool {
    parse_canonical_timestamp(value).is_some()
}

fn now() -> String {
    canonical_timestamp(Utc::now())
}

fn write_atomic(file: &Path, entries: &[SteerHistoryEntry]) -> Result<(), String> {
    let mut payload = serde_json::to_vec(&SteerHistorySnapshot {
        version: STEER_HISTORY_SNAPSHOT_VERSION,
        entries,
    })
    .map_err(|error| error.to_string())?;
    payload.push(b'\n');
    crate::private_write::write_private_bytes(file, &payload)
}

fn quarantine_corrupt(file: &Path) {
    let corrupt = PathBuf::from(format!("{}.corrupt", file.display()));
    if fs::rename(file, &corrupt).is_ok()
        && fs::symlink_metadata(&corrupt)
            .is_ok_and(|metadata| metadata.is_file() && metadata.nlink() == 1)
    {
        let _ = fs::set_permissions(corrupt, fs::Permissions::from_mode(OWNER_FILE_MODE));
    }
}

fn invalid_snapshot() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Steer history snapshot is invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PathBuf, SteerHistoryStore) {
        let root = tempfile::tempdir().expect("temp");
        let file = root.path().join("nested/steers.json");
        let store = SteerHistoryStore::new(&file).expect("store");
        (root, file, store)
    }

    #[test]
    fn pending_is_durable_before_return_settles_same_record_and_reloads() {
        let (_root, file, history) = store();
        history.load();
        let engine = EngineId::new("claude").expect("engine");
        let first = history
            .begin(
                "claude#current",
                &engine,
                Some("fable"),
                "change direction",
                Some("2026-08-24T12:00:00.000Z"),
            )
            .expect("begin");
        let snapshot: Value =
            serde_json::from_slice(&fs::read(&file).expect("pending snapshot")).expect("json");
        assert_eq!(snapshot["entries"][0]["id"], first.id);
        assert_eq!(first.outcome, SteerOutcome::Pending);
        assert_eq!(
            first.prompt_digest,
            "af08c078b1d1b2e4880f49e6a214a25e437557e081aa08b8a61ec4655df033bf"
        );
        let settled = history
            .settle(
                &first.id,
                SteerSettledOutcome::AdapterAccepted,
                Some("2026-08-24T12:00:02.000Z"),
                Some("provider accepted"),
            )
            .expect("settle")
            .expect("record");
        let reloaded = SteerHistoryStore::new(&file).expect("reload");
        reloaded.load();
        assert_eq!(reloaded.list_for_session("claude#current", 40), [settled]);
        assert!(reloaded.list_for_session("claude#curr", 40).is_empty());
    }

    #[test]
    fn preview_model_and_detail_are_sanitized_bounded_while_digest_is_raw() {
        let (_root, file, history) = store();
        let raw = format!("safe\u{1b}]52;c;clipboard-secret\u{7}{}", "😀".repeat(300));
        let begun = history
            .begin(
                "codex#safe",
                &EngineId::new("codex").expect("engine"),
                Some("gpt\u{1b}[2J-5.6"),
                &raw,
                Some("2026-08-24T12:00:00.000Z"),
            )
            .expect("begin");
        assert_eq!(begun.prompt_preview.chars().count(), 240);
        assert_eq!(begun.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(
            begun.prompt_digest,
            format!("{:x}", Sha256::digest(raw.as_bytes()))
        );
        let settled = history
            .settle(
                &begun.id,
                SteerSettledOutcome::OutcomeUnknown,
                Some("2026-08-24T12:00:01.000Z"),
                Some(&format!("timeout\u{1b}]9;notify\u{7}{}", "x".repeat(700))),
            )
            .expect("settle")
            .expect("record");
        assert_eq!(settled.detail.expect("detail").chars().count(), 512);
        let source = fs::read_to_string(file).expect("source");
        assert!(!source.contains("clipboard-secret"));
        assert!(!source.contains("notify"));
    }

    #[test]
    fn global_and_exact_session_retention_match_legacy_bounds() {
        let (_root, file, history) = store();
        let engine = EngineId::new("claude").expect("engine");
        for index in 0..(MAX_STEER_HISTORY_ENTRIES + 4) {
            history
                .begin(
                    "claude#retention",
                    &engine,
                    None,
                    &format!("prompt-{index}"),
                    Some(&format!("2026-08-24T00:{:02}:00.000Z", index / 60)),
                )
                .expect("begin");
        }
        let snapshot: Value =
            serde_json::from_slice(&fs::read(file).expect("snapshot")).expect("json");
        assert_eq!(snapshot["entries"].as_array().expect("entries").len(), 256);
        assert_eq!(snapshot["entries"][0]["promptPreview"], "prompt-4");
        let visible = history.list_for_session("claude#retention", usize::MAX);
        assert_eq!(visible.len(), 40);
        assert_eq!(visible[0].prompt_preview, "prompt-259");
        assert_eq!(visible[39].prompt_preview, "prompt-220");
    }

    #[test]
    fn corrupt_and_symlink_snapshots_are_quarantined_without_following_targets() {
        let (root, file, history) = store();
        fs::create_dir_all(file.parent().expect("parent")).expect("parent");
        fs::write(&file, "{private corrupt steer content").expect("corrupt");
        history.load();
        assert!(history.list_for_session("claude#current", 40).is_empty());
        assert_eq!(
            fs::metadata(format!("{}.corrupt", file.display()))
                .expect("corrupt metadata")
                .mode()
                & 0o777,
            0o600
        );

        let linked = root.path().join("linked.json");
        let target = root.path().join("outside.json");
        fs::write(&target, "private target").expect("target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).expect("target mode");
        std::os::unix::fs::symlink(&target, &linked).expect("symlink");
        let linked_history = SteerHistoryStore::new(&linked).expect("linked store");
        linked_history.load();
        assert!(
            fs::symlink_metadata(format!("{}.corrupt", linked.display()))
                .expect("quarantined symlink")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(target).expect("target metadata").mode() & 0o777,
            0o644
        );
    }
}
