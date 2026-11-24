//! Single-writer, full-record collaboration journal.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use interface_storage::{
    FileSnapshot, JsonLimits, PrivateDirectory, Revisioned, StorageError, TargetSnapshot,
    WriterLock, atomic_write_json, compare_and_set_json, read_bounded, read_json, snapshot_target,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::state::{CollaborationRecordV1, CollaborationState, JS_MAX_SAFE_INTEGER};

const MAX_RECORD_BYTES: usize = 16 * 1_024 * 1_024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;
const RECORDS_DIRECTORY: &str = "records";
const QUARANTINE_DIRECTORY: &str = "quarantine";
const WRITER_LOCK: &str = "collaboration.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalErrorCode {
    InvalidRecord,
    UnsafeStorage,
    StateCorrupt,
    RecordNotFound,
    RecordExists,
    IdempotencyConflict,
    CasConflict,
    WriteFailed,
    WriteUncertain,
    WriterLeaseLost,
    JournalClosed,
    JournalPoisoned,
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct JournalError {
    pub code: JournalErrorCode,
    pub current: Option<(u64, CollaborationState)>,
    message: String,
    #[source]
    source: Option<StorageError>,
}

impl JournalError {
    fn new(code: JournalErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            current: None,
            message: message.into(),
            source: None,
        }
    }

    fn with_current(
        code: JournalErrorCode,
        message: impl Into<String>,
        record: &CollaborationRecordV1,
    ) -> Self {
        Self {
            code,
            current: Some((record.revision, record.state)),
            message: message.into(),
            source: None,
        }
    }

    fn storage(error: StorageError) -> Self {
        let code = match error {
            StorageError::Corrupt { .. } => JournalErrorCode::StateCorrupt,
            StorageError::CasConflict { .. } => JournalErrorCode::CasConflict,
            StorageError::WriteFailed { .. } => JournalErrorCode::WriteFailed,
            StorageError::WriteUncertain { .. } => JournalErrorCode::WriteUncertain,
            StorageError::LockLost { .. } | StorageError::LockHeld { .. } => {
                JournalErrorCode::WriterLeaseLost
            }
            _ => JournalErrorCode::UnsafeStorage,
        };
        Self {
            code,
            current: None,
            message: error.to_string(),
            source: Some(error),
        }
    }
}

pub type JournalResult<T> = Result<T, JournalError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalCreateResult {
    pub record: CollaborationRecordV1,
    pub replayed: bool,
}

impl Revisioned for CollaborationRecordV1 {
    fn revision(&self) -> u64 {
        self.revision
    }
}

/// A full-record journal. Holding this value holds the process' one-writer
/// lease; all mutations additionally compare exact on-disk bytes/inodes.
pub struct CollaborationJournal {
    // Never read, but its open handle keeps the writer lease's parent alive.
    #[allow(dead_code)]
    root: PrivateDirectory,
    records_directory: PrivateDirectory,
    writer_lock: WriterLock,
    records: BTreeMap<String, CollaborationRecordV1>,
    dedupe: BTreeMap<String, String>,
    snapshots: BTreeMap<String, FileSnapshot>,
    accepting_writes: bool,
    poisoned: Option<JournalErrorCode>,
}

impl CollaborationJournal {
    /// Opens or creates an owner-private journal and acquires its durable
    /// one-writer lease.
    pub fn open(root: impl AsRef<Path>) -> JournalResult<Self> {
        validate_root(root.as_ref())?;
        let root = PrivateDirectory::open_or_create(root).map_err(JournalError::storage)?;
        let records_directory = root
            .open_or_create_child(RECORDS_DIRECTORY)
            .map_err(JournalError::storage)?;
        let quarantine_directory = root
            .open_or_create_child(QUARANTINE_DIRECTORY)
            .map_err(JournalError::storage)?;
        // The frozen TypeScript journal permits only committed JSON and stale
        // temporary files inside `records`. Keep Rust's persistent advisory
        // lock in the parent while binding its lease to the records inode, so
        // a rollback to Interface 0.1.169 can still open the same journal.
        let writer_lock = WriterLock::acquire_for(&root, WRITER_LOCK, &records_directory)
            .map_err(JournalError::storage)?;
        writer_lock
            .assert_held_for(&records_directory)
            .map_err(JournalError::storage)?;
        recover_legacy_layout(&records_directory, &quarantine_directory, &writer_lock)?;
        let (records, dedupe, snapshots) = load_records(&records_directory)?;
        writer_lock
            .assert_held_for(&records_directory)
            .map_err(JournalError::storage)?;
        Ok(Self {
            root,
            records_directory,
            writer_lock,
            records,
            dedupe,
            snapshots,
            accepting_writes: true,
            poisoned: None,
        })
    }

    pub fn get(&self, collaboration_id: &str) -> JournalResult<Option<CollaborationRecordV1>> {
        Ok(self.record(collaboration_id)?.cloned())
    }

    /// Borrowed form of [`Self::get`] for read-only callers that project a
    /// few fields and must not copy the whole record.
    pub fn record(&self, collaboration_id: &str) -> JournalResult<Option<&CollaborationRecordV1>> {
        self.assert_healthy()?;
        Ok(self.records.get(collaboration_id))
    }

    pub fn find_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> JournalResult<Option<CollaborationRecordV1>> {
        self.assert_healthy()?;
        let Some(collaboration_id) = self.dedupe.get(dedupe_key) else {
            return Ok(None);
        };
        let record = self.records.get(collaboration_id).ok_or_else(|| {
            JournalError::new(
                JournalErrorCode::StateCorrupt,
                "dedupe index points to a missing collaboration",
            )
        })?;
        Ok(Some(record.clone()))
    }

    pub fn list(&self) -> JournalResult<Vec<CollaborationRecordV1>> {
        Ok(self.records()?.cloned().collect())
    }

    pub fn records(&self) -> JournalResult<impl Iterator<Item = &CollaborationRecordV1>> {
        self.assert_healthy()?;
        Ok(self.records.values())
    }

    pub fn create(&mut self, record: CollaborationRecordV1) -> JournalResult<JournalCreateResult> {
        self.assert_mutable()?;
        record.validate().map_err(|_| {
            JournalError::new(
                JournalErrorCode::InvalidRecord,
                "journal record failed full schema validation",
            )
        })?;
        if record.revision != 0 {
            return Err(JournalError::new(
                JournalErrorCode::InvalidRecord,
                "a new collaboration record must start at revision 0",
            ));
        }
        if let Some(existing_id) = self.dedupe.get(&record.dedupe_key) {
            let existing = self.records.get(existing_id).ok_or_else(|| {
                JournalError::new(
                    JournalErrorCode::StateCorrupt,
                    "dedupe index points to a missing collaboration",
                )
            })?;
            if existing.payload_hash != record.payload_hash {
                return Err(JournalError::with_current(
                    JournalErrorCode::IdempotencyConflict,
                    "dedupe key is already bound to a different normalized payload",
                    existing,
                ));
            }
            return Ok(JournalCreateResult {
                record: existing.clone(),
                replayed: true,
            });
        }
        if self.records.contains_key(&record.collaboration_id) {
            return Err(JournalError::new(
                JournalErrorCode::RecordExists,
                "collaboration ID already exists",
            ));
        }
        let name = record_file_name(&record.collaboration_id);
        self.prove_lease()?;
        let expected = snapshot_target(&self.records_directory, &name, MAX_RECORD_BYTES)
            .map_err(|error| self.poison_storage(error))?;
        if expected != TargetSnapshot::Missing {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "record pathname is already occupied",
            ));
        }
        let installed = atomic_write_json(
            &self.records_directory,
            &self.writer_lock,
            &name,
            &record,
            &expected,
            json_limits(),
        )
        .map_err(|error| self.poison_storage(error))?;
        self.records
            .insert(record.collaboration_id.clone(), record.clone());
        self.dedupe
            .insert(record.dedupe_key.clone(), record.collaboration_id.clone());
        self.snapshots.insert(
            record.collaboration_id.clone(),
            installed.snapshot().clone(),
        );
        Ok(JournalCreateResult {
            record,
            replayed: false,
        })
    }

    pub fn compare_and_set(
        &mut self,
        collaboration_id: &str,
        expected_revision: u64,
        expected_state: CollaborationState,
        replacement: CollaborationRecordV1,
    ) -> JournalResult<CollaborationRecordV1> {
        self.assert_mutable()?;
        if expected_revision > JS_MAX_SAFE_INTEGER {
            return Err(JournalError::new(
                JournalErrorCode::InvalidRecord,
                "expected revision must be a non-negative JavaScript-safe integer",
            ));
        }
        let current = self.records.get(collaboration_id).cloned().ok_or_else(|| {
            JournalError::new(
                JournalErrorCode::RecordNotFound,
                "collaboration does not exist",
            )
        })?;
        if current.revision != expected_revision || current.state != expected_state {
            return Err(JournalError::with_current(
                JournalErrorCode::CasConflict,
                "collaboration revision or state is stale",
                &current,
            ));
        }
        if current.revision == JS_MAX_SAFE_INTEGER {
            self.poisoned = Some(JournalErrorCode::StateCorrupt);
            return Err(JournalError::with_current(
                JournalErrorCode::StateCorrupt,
                "collaboration exhausted its revision range",
                &current,
            ));
        }
        replacement.validate().map_err(|_| {
            JournalError::new(
                JournalErrorCode::InvalidRecord,
                "journal record failed full schema validation",
            )
        })?;
        if replacement.version != current.version
            || replacement.collaboration_id != current.collaboration_id
            || replacement.dedupe_key != current.dedupe_key
            || replacement.payload_hash != current.payload_hash
            || replacement.created_at != current.created_at
            || replacement.revision != current.revision.checked_add(1).unwrap_or(u64::MAX)
            || replacement.updated_at < current.updated_at
        {
            return Err(JournalError::new(
                JournalErrorCode::InvalidRecord,
                "CAS cannot change identity or skip/regress a record revision",
            ));
        }
        self.prove_lease()?;
        let name = record_file_name(collaboration_id);
        let loaded =
            read_json::<CollaborationRecordV1>(&self.records_directory, &name, json_limits())
                .map_err(|error| self.poison_storage(error))?
                .ok_or_else(|| {
                    self.poisoned = Some(JournalErrorCode::StateCorrupt);
                    JournalError::new(
                        JournalErrorCode::StateCorrupt,
                        "committed record disappeared before CAS",
                    )
                })?;
        if loaded.value() != &current
            || self.snapshots.get(collaboration_id) != Some(loaded.snapshot())
        {
            self.poisoned = Some(JournalErrorCode::StateCorrupt);
            return Err(JournalError::with_current(
                JournalErrorCode::StateCorrupt,
                "authoritative record changed outside this writer",
                loaded.value(),
            ));
        }
        let installed = compare_and_set_json(
            &self.records_directory,
            &self.writer_lock,
            &name,
            &loaded,
            expected_revision,
            &replacement,
            json_limits(),
        )
        .map_err(|error| self.poison_storage(error))?;
        self.records
            .insert(collaboration_id.to_owned(), replacement.clone());
        self.snapshots
            .insert(collaboration_id.to_owned(), installed.snapshot().clone());
        Ok(replacement)
    }

    /// Synchronously fences new writes and proves the lease is still held.
    pub fn close(&mut self) -> JournalResult<()> {
        self.accepting_writes = false;
        self.assert_healthy()?;
        self.prove_lease()
    }

    fn assert_healthy(&self) -> JournalResult<()> {
        if let Some(code) = self.poisoned {
            return Err(JournalError::new(
                JournalErrorCode::JournalPoisoned,
                format!("journal must be reopened after {code:?}"),
            ));
        }
        Ok(())
    }

    fn assert_mutable(&self) -> JournalResult<()> {
        self.assert_healthy()?;
        if !self.accepting_writes {
            return Err(JournalError::new(
                JournalErrorCode::JournalClosed,
                "collaboration journal is closed to new writes",
            ));
        }
        Ok(())
    }

    fn prove_lease(&self) -> JournalResult<()> {
        self.writer_lock
            .assert_held_for(&self.records_directory)
            .map_err(JournalError::storage)
    }

    fn poison_storage(&mut self, error: StorageError) -> JournalError {
        let mapped = JournalError::storage(error);
        self.poisoned = Some(mapped.code);
        mapped
    }
}

fn validate_root(root: &Path) -> JournalResult<()> {
    if !root.is_absolute() || root.parent().is_none() || root.as_os_str().is_empty() {
        return Err(JournalError::new(
            JournalErrorCode::UnsafeStorage,
            "journal root must be a non-root absolute path",
        ));
    }
    Ok(())
}

fn json_limits() -> JsonLimits {
    JsonLimits::new(MAX_RECORD_BYTES, MAX_JSON_DEPTH, MAX_JSON_NODES)
        .expect("collaboration journal limits are valid")
}

fn record_file_name(collaboration_id: &str) -> String {
    format!("{:x}.json", Sha256::digest(collaboration_id.as_bytes()))
}

fn entry_names(directory: &PrivateDirectory, what: &str) -> JournalResult<Vec<String>> {
    let entries = fs::read_dir(directory.path()).map_err(|error| {
        JournalError::new(
            JournalErrorCode::UnsafeStorage,
            format!("could not enumerate collaboration {what}: {error}"),
        )
    })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            JournalError::new(
                JournalErrorCode::UnsafeStorage,
                format!("could not enumerate collaboration {what} entry: {error}"),
            )
        })?;
        names.push(entry.file_name().into_string().map_err(|_| {
            JournalError::new(
                JournalErrorCode::StateCorrupt,
                format!("{what} contains a non-UTF-8 entry name"),
            )
        })?);
    }
    Ok(names)
}

fn load_records(
    directory: &PrivateDirectory,
) -> JournalResult<(
    BTreeMap<String, CollaborationRecordV1>,
    BTreeMap<String, String>,
    BTreeMap<String, FileSnapshot>,
)> {
    directory
        .assert_unchanged()
        .map_err(JournalError::storage)?;
    let mut names = entry_names(directory, "journal")?;
    if !names.iter().all(|name| is_record_file(name)) {
        return Err(JournalError::new(
            JournalErrorCode::StateCorrupt,
            "unexpected entry in committed journal directory",
        ));
    }
    directory
        .assert_unchanged()
        .map_err(JournalError::storage)?;
    names.sort();
    let mut records = BTreeMap::new();
    let mut dedupe = BTreeMap::new();
    let mut snapshots = BTreeMap::new();
    // `names` comes from one read_dir, so it holds no duplicate pathnames.
    for name in names {
        let (record, snapshot) =
            read_json::<CollaborationRecordV1>(directory, &name, json_limits())
                .map_err(JournalError::storage)?
                .ok_or_else(|| {
                    JournalError::new(
                        JournalErrorCode::StateCorrupt,
                        "journal record disappeared during load",
                    )
                })?
                .into_parts();
        record.validate().map_err(|_| {
            JournalError::new(
                JournalErrorCode::StateCorrupt,
                "journal record failed full schema validation",
            )
        })?;
        if record_file_name(&record.collaboration_id) != name {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "journal pathname does not match collaboration identity",
            ));
        }
        // Kept as a guard although the name check above already implies it.
        if records.contains_key(&record.collaboration_id) {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "collaboration has more than one committed record",
            ));
        }
        if dedupe
            .insert(record.dedupe_key.clone(), record.collaboration_id.clone())
            .is_some()
        {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "dedupe key is committed by more than one collaboration",
            ));
        }
        snapshots.insert(record.collaboration_id.clone(), snapshot);
        records.insert(record.collaboration_id.clone(), record);
    }
    Ok((records, dedupe, snapshots))
}

// The frozen TypeScript reader recognizes only the exact lowercase `.tmp`
// suffix; accepting `.TMP` would widen rollback-compatible storage authority.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn recover_legacy_layout(
    records: &PrivateDirectory,
    quarantine: &PrivateDirectory,
    lock: &WriterLock,
) -> JournalResult<()> {
    records.assert_unchanged().map_err(JournalError::storage)?;
    quarantine
        .assert_unchanged()
        .map_err(JournalError::storage)?;
    lock.assert_held_for(records)
        .map_err(JournalError::storage)?;

    // The legacy reader accepts any private regular forensic filename. It does
    // not assign authority to quarantine bytes, but links, devices, oversized
    // files, or identity drift still fail the store closed.
    for name in entry_names(quarantine, "quarantine")? {
        read_bounded(quarantine, &name, MAX_RECORD_BYTES)
            .map_err(JournalError::storage)?
            .ok_or_else(|| {
                JournalError::new(
                    JournalErrorCode::StateCorrupt,
                    "quarantine artifact disappeared during verification",
                )
            })?;
    }

    let mut stale = Vec::new();
    for name in entry_names(records, "journal")? {
        if name.ends_with(".tmp") {
            let file = read_bounded(records, &name, MAX_RECORD_BYTES)
                .map_err(JournalError::storage)?
                .ok_or_else(|| {
                    JournalError::new(
                        JournalErrorCode::StateCorrupt,
                        "stale journal temp disappeared during verification",
                    )
                })?;
            stale.push((name, file.snapshot().clone(), file.bytes().to_vec()));
        } else if !is_record_file(&name) {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "unexpected entry in committed journal directory",
            ));
        }
    }

    for (name, expected, bytes) in stale {
        lock.assert_held_for(records)
            .map_err(JournalError::storage)?;
        let before = read_bounded(records, &name, MAX_RECORD_BYTES)
            .map_err(JournalError::storage)?
            .ok_or_else(|| {
                JournalError::new(
                    JournalErrorCode::StateCorrupt,
                    "stale journal temp disappeared before quarantine",
                )
            })?;
        if before.snapshot() != &expected || before.bytes() != bytes {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "stale journal temp changed before quarantine",
            ));
        }
        let destination_name = format!("{name}.{}.quarantined", Uuid::new_v4());
        if snapshot_target(quarantine, &destination_name, MAX_RECORD_BYTES)
            .map_err(JournalError::storage)?
            != TargetSnapshot::Missing
        {
            return Err(JournalError::new(
                JournalErrorCode::StateCorrupt,
                "quarantine destination is already occupied",
            ));
        }
        let source = records.path().join(&name);
        let destination = quarantine.path().join(&destination_name);
        fs::rename(&source, &destination).map_err(|error| {
            JournalError::new(
                JournalErrorCode::WriteFailed,
                format!("could not quarantine stale journal temp: {error}"),
            )
        })?;
        lock.assert_held_for(records)
            .map_err(JournalError::storage)?;
        let installed = read_bounded(quarantine, &destination_name, MAX_RECORD_BYTES)
            .map_err(JournalError::storage)?
            .ok_or_else(|| {
                JournalError::new(
                    JournalErrorCode::WriteUncertain,
                    "quarantined journal temp is missing after rename",
                )
            })?;
        if installed.snapshot() != &expected || installed.bytes() != bytes {
            return Err(JournalError::new(
                JournalErrorCode::WriteUncertain,
                "quarantined journal temp does not preserve the source inode and bytes",
            ));
        }
        records.sync().map_err(JournalError::storage)?;
        quarantine.sync().map_err(JournalError::storage)?;
        lock.assert_held_for(records)
            .map_err(JournalError::storage)?;
        if snapshot_target(records, &name, MAX_RECORD_BYTES).map_err(JournalError::storage)?
            != TargetSnapshot::Missing
        {
            return Err(JournalError::new(
                JournalErrorCode::WriteUncertain,
                "stale journal temp reappeared after quarantine",
            ));
        }
    }
    records.assert_unchanged().map_err(JournalError::storage)?;
    quarantine.assert_unchanged().map_err(JournalError::storage)
}

fn is_record_file(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(|stem| {
        stem.len() == 64
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[must_use]
pub fn journal_record_path(root: &Path, collaboration_id: &str) -> PathBuf {
    root.join(RECORDS_DIRECTORY)
        .join(record_file_name(collaboration_id))
}
