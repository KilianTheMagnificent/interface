use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, PoisonError};

use interface_contracts::engine::{
    EngineEvent, EngineEventKind, EngineId, SessionKind, SessionOrigin, SessionStatus,
    SessionSummary, next_status, session_key,
};
use interface_storage::{
    JsonLimits, PrivateDirectory, QuarantineReason, TargetSnapshot, WriterLock, atomic_write,
    decode_json_value_strict, ensure_current_revision, quarantine_file,
    quarantine_stale_atomic_writes_for, read_bounded, snapshot_target,
};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{RegistryError, Result};

pub const REGISTRY_MAX_BYTES: usize = 16 * 1024 * 1024;
const REGISTRY_FILE: &str = "registry.json";
const REGISTRY_LOCK: &str = "registry.lock";

#[derive(Debug)]
pub struct SessionRegistry {
    writer: Arc<CacheWriter>,
    cache_path: PathBuf,
    unavailable_reason: Option<String>,
    /// The rows at open came from a readable cache file.
    restored: bool,
    /// Where an unreadable cache found at open was set aside.
    quarantined: Option<PathBuf>,
    sessions: Vec<(String, SessionSummary)>,
    titles: Vec<(String, String)>,
    forgotten: Vec<String>,
    // Ordered vectors remain the persistence authority. These derived indexes
    // make provider refreshes linear without changing legacy JSON ordering.
    session_positions: HashMap<String, usize>,
    title_positions: HashMap<String, usize>,
    forgotten_keys: HashSet<String>,
    revision: u64,
    content_revision: u64,
}

#[derive(Debug)]
struct RegistryPersistence {
    directory: PrivateDirectory,
    lock: WriterLock,
    file_name: String,
    target: TargetSnapshot,
}

/// The cache file's own writer state, locked apart from the registry so a
/// save can serialize and write after the registry lock is released.
#[derive(Debug)]
struct CacheWriter {
    persistence: Mutex<Option<RegistryPersistence>>,
    /// Registry revision of the last image on disk, 0 before the first write.
    /// Saves can finish out of order, so an image no newer than this is
    /// dropped. Advanced only while `persistence` is locked; read without it.
    written: AtomicU64,
}

impl CacheWriter {
    fn covers(&self, revision: u64) -> bool {
        self.written.load(AtomicOrdering::Acquire) >= revision
    }
}

/// A registry cache image copied under the registry lock by
/// [`SessionRegistry::prepare_save`]; [`Self::write`] serializes, validates
/// and writes it without that lock.
#[must_use = "a prepared save writes nothing until `write` runs"]
pub struct RegistrySave {
    revision: u64,
    sessions: Vec<(String, SessionSummary)>,
    titles: Vec<(String, String)>,
    forgotten: Vec<String>,
    cache_path: PathBuf,
    unavailable_reason: Option<String>,
    writer: Arc<CacheWriter>,
}

impl RegistrySave {
    /// Serializes, strictly re-reads and atomically writes the image, in the
    /// same order `SessionRegistry::save` always has. An image no newer than
    /// the last one written is dropped, never written over it, and an image
    /// whose bytes equal the file this writer last installed is not rewritten.
    ///
    /// # Errors
    ///
    /// Returns serialization, validation, unavailable-cache and storage
    /// errors exactly as [`SessionRegistry::save`] does.
    pub fn write(self) -> Result<()> {
        // A save of this revision (or a later one) already finished: skip the
        // serialization as well as the write.
        if self.writer.covers(self.revision) {
            return Ok(());
        }
        let value = PersistedRegistry {
            sessions: OrderedMap(&self.sessions),
            titles: OrderedMap(&self.titles),
            forgotten: &self.forgotten,
        };
        let bytes = serde_json::to_vec(&value).map_err(|error| {
            RegistryError::Invalid(format!("registry cannot be serialized: {error}"))
        })?;
        let limits = registry_limits()?;
        decode_json_value_strict(&bytes, limits, &self.cache_path)?;
        let mut persistence = self
            .writer
            .persistence
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(persistence) = persistence.as_mut() else {
            return Err(RegistryError::Invalid(format!(
                "registry cache is unavailable: {}",
                self.unavailable_reason
                    .as_deref()
                    .unwrap_or("startup cache initialization failed")
            )));
        };
        if self.writer.covers(self.revision) {
            return Ok(());
        }
        // Only a file this writer installed (and synced) is known durable, so
        // only then may identical bytes stand in for a write. Anything short
        // of an exact match falls through, and the write reports it as before.
        if self.writer.written.load(AtomicOrdering::Acquire) != 0 && persistence.holds(&bytes) {
            self.writer
                .written
                .store(self.revision, AtomicOrdering::Release);
            return Ok(());
        }
        let snapshot = atomic_write(
            &persistence.directory,
            &persistence.lock,
            &persistence.file_name,
            &bytes,
            &persistence.target,
            REGISTRY_MAX_BYTES,
        )?;
        persistence.target = TargetSnapshot::File(snapshot);
        self.writer
            .written
            .store(self.revision, AtomicOrdering::Release);
        Ok(())
    }
}

impl RegistryPersistence {
    /// Whether the named file is still exactly the recorded target and holds
    /// `bytes`, with the writer lease held: the checks `atomic_write` makes
    /// before it would replace that file with the same bytes.
    fn holds(&self, bytes: &[u8]) -> bool {
        let TargetSnapshot::File(target) = &self.target else {
            return false;
        };
        target.sha256()[..] == Sha256::digest(bytes)[..]
            && self.lock.assert_held_for(&self.directory).is_ok()
            && snapshot_target(&self.directory, &self.file_name, REGISTRY_MAX_BYTES)
                .is_ok_and(|current| current == self.target)
    }
}

struct OpenedRegistryCache {
    persistence: Option<RegistryPersistence>,
    loaded: RegistryState,
    /// `loaded` came from a cache file that was there and readable.
    restored: bool,
    /// Where an unreadable cache file was set aside.
    quarantined: Option<PathBuf>,
}

enum OpenCacheFailure {
    Recoverable(RegistryError),
    WriterLease(RegistryError),
}

impl From<RegistryError> for OpenCacheFailure {
    fn from(error: RegistryError) -> Self {
        Self::Recoverable(error)
    }
}

impl From<interface_storage::StorageError> for OpenCacheFailure {
    fn from(error: interface_storage::StorageError) -> Self {
        Self::Recoverable(error.into())
    }
}

impl SessionRegistry {
    /// Opens the provider-reconstructible cache when it is available.
    /// Invalid JSON is preserved in quarantine before an empty cache is used,
    /// and reconstructible cache-data failures degrade to an in-memory empty
    /// projection. Writer-lease failures remain explicit so one data root never
    /// has two live runtime owners.
    ///
    /// # Errors
    ///
    /// Provider-reconstructible cache I/O does not make Interface startup fail;
    /// later explicit [`Self::save`] calls report that persistence is
    /// unavailable. A held or unprovable writer lease remains an explicit
    /// startup failure so two live runtimes cannot share one data root.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        Self::open_named(directory, REGISTRY_FILE)
    }

    /// Opens a named provider-reconstructible cache when it is available.
    ///
    /// # Errors
    ///
    /// This constructor retains a result for API compatibility. Cache I/O does
    /// not make Interface startup fail; later explicit [`Self::save`] calls
    /// report that persistence is unavailable.
    pub fn open_named(directory: impl AsRef<Path>, file_name: &str) -> Result<Self> {
        let root = directory.as_ref();
        let cache_path = root.join(file_name);
        if let Err(error) = repair_legacy_cache_mode(&cache_path) {
            eprintln!(
                "[registry] could not restrict existing cache permissions; hardened loading will decide whether it remains usable: {error}"
            );
        }
        let (persistence, loaded, unavailable_reason, restored, quarantined) = match open_cache(
            root, file_name,
        ) {
            Ok(opened) => (
                opened.persistence,
                opened.loaded,
                None,
                opened.restored,
                opened.quarantined,
            ),
            Err(OpenCacheFailure::Recoverable(error)) => {
                eprintln!(
                    "[registry] starting empty; the provider-reconstructible cache is unavailable: {error}"
                );
                (
                    None,
                    RegistryState::default(),
                    Some(error.to_string()),
                    false,
                    None,
                )
            }
            Err(OpenCacheFailure::WriterLease(error)) => return Err(error),
        };
        Ok(Self {
            writer: Arc::new(CacheWriter {
                persistence: Mutex::new(persistence),
                written: AtomicU64::new(0),
            }),
            cache_path,
            unavailable_reason,
            restored,
            quarantined,
            session_positions: positions(&loaded.sessions),
            title_positions: positions(&loaded.titles),
            forgotten_keys: loaded.forgotten.iter().cloned().collect(),
            sessions: loaded.sessions,
            titles: loaded.titles,
            forgotten: loaded.forgotten,
            // Legacy `load()` emits one generation change, including on a
            // missing file, so the first observable snapshot starts at one.
            revision: 1,
            content_revision: 1,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether this registry's rows at open came from a readable cache file.
    /// False when it started empty because the cache was missing,
    /// unavailable or quarantined: its sessions then reappear only as each
    /// provider lists them, so an absent row proves nothing for the life of
    /// this registry.
    #[must_use]
    pub const fn restored_from_cache(&self) -> bool {
        self.restored
    }

    /// Where an unreadable cache found at open was set aside, when one was.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// Whether the cache file already holds this revision's image, written by
    /// this registry. False until its first save lands, and whenever a change
    /// or a failed save leaves the file behind; a caller that changed nothing
    /// can skip scheduling a save exactly when this is true.
    #[must_use]
    pub fn cache_is_current(&self) -> bool {
        self.writer.covers(self.revision)
    }

    /// Advances with [`Self::revision`] on every change except an
    /// [`Self::apply_event`] that moved only `last_activity`. Two equal
    /// readings therefore prove the rows, their order and every field other
    /// than `last_activity` are unchanged in between, whoever else read or
    /// pushed the registry meanwhile. Not persisted.
    #[must_use]
    pub const fn content_revision(&self) -> u64 {
        self.content_revision
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&SessionSummary> {
        self.session_positions
            .get(key)
            .map(|&index| &self.sessions[index].1)
    }

    #[must_use]
    pub fn all(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .map(|(_, summary)| summary.clone())
            .collect()
    }

    /// The same rows as [`Self::all`], in the same order, without cloning any.
    pub fn summaries(&self) -> impl Iterator<Item = &SessionSummary> {
        self.sessions.iter().map(|(_, summary)| summary)
    }

    #[must_use]
    pub fn list(&self) -> Vec<SessionSummary> {
        let mut visible = self.list_unsorted();
        sort_listed(&mut visible);
        visible
    }

    /// The rows [`Self::list`] returns, in stored order. A caller holding the
    /// registry lock clones under it and runs [`sort_listed`] after releasing
    /// it; the stable sort over the same input yields the same order.
    #[must_use]
    pub fn list_unsorted(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .filter(|(_, summary)| !summary.archived)
            .map(|(_, summary)| summary.clone())
            .collect()
    }

    #[must_use]
    pub fn is_forgotten(&self, key: &str) -> bool {
        self.forgotten_keys.contains(key)
    }

    /// Merges a complete provider listing using the legacy archive, title,
    /// origin, worktree, and live-status rules. The revision advances only
    /// when a row was added or a merged row differs from the stored one.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn upsert_all(
        &mut self,
        expected_revision: u64,
        engine: &EngineId,
        listed: impl IntoIterator<Item = SessionSummary>,
    ) -> Result<u64> {
        self.guard(expected_revision)?;
        let mut changed = false;
        for mut incoming in listed {
            incoming.engine = engine.clone();
            let key = session_key(engine, &incoming.session_id);
            if self.is_forgotten(&key) {
                continue;
            }
            sanitize_summary(&mut incoming);
            let title_override = self.title_for(&key).map(ToOwned::to_owned);
            if let Some(&index) = self.session_positions.get(&key) {
                let previous = &mut self.sessions[index].1;
                let merged = merge_summary(previous, incoming, title_override.as_deref());
                if merged != *previous {
                    *previous = merged;
                    changed = true;
                }
            } else {
                if let Some(title) = title_override {
                    incoming.title = title;
                }
                self.session_positions
                    .insert(key.clone(), self.sessions.len());
                self.sessions.push((key, incoming));
                changed = true;
            }
        }
        if changed {
            self.bump()
        } else {
            Ok(self.revision)
        }
    }

    /// Applies a provider event without treating command catalogues as activity.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn apply_event(
        &mut self,
        expected_revision: u64,
        key: &str,
        event: &EngineEvent,
        at_iso: impl Into<String>,
    ) -> Result<u64> {
        self.guard(expected_revision)?;
        if matches!(
            event.event,
            EngineEventKind::CommandsAvailable { .. } | EngineEventKind::CommandsInvalidated
        ) {
            return Ok(self.revision);
        }
        let Some(summary) = self.get_mut(key) else {
            return Ok(self.revision);
        };
        let status = next_status(summary.status, event);
        let status_changed = summary.status != status;
        summary.status = status;
        summary.last_activity = at_iso.into();
        if status_changed {
            self.bump()
        } else {
            self.bump_activity()
        }
    }

    /// Sets an existing session's archive state.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn mark_archived(
        &mut self,
        expected_revision: u64,
        key: &str,
        archived: bool,
    ) -> Result<u64> {
        self.guard(expected_revision)?;
        let Some(summary) = self.get_mut(key) else {
            return Ok(self.revision);
        };
        summary.archived = archived;
        self.bump()
    }

    /// Sets the durable user title override for an existing session.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn set_title(
        &mut self,
        expected_revision: u64,
        key: &str,
        title: impl Into<String>,
    ) -> Result<u64> {
        self.guard(expected_revision)?;
        let Some(summary) = self.get_mut(key) else {
            return Ok(self.revision);
        };
        let title = title.into();
        summary.title.clone_from(&title);
        if let Some(&index) = self.title_positions.get(key) {
            self.titles[index].1 = title;
        } else {
            self.title_positions
                .insert(key.to_owned(), self.titles.len());
            self.titles.push((key.to_owned(), title));
        }
        self.bump()
    }

    /// Explicitly changes an existing session's origin in either direction.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn set_origin(
        &mut self,
        expected_revision: u64,
        key: &str,
        origin: SessionOrigin,
    ) -> Result<u64> {
        self.guard(expected_revision)?;
        let Some(summary) = self.get_mut(key) else {
            return Ok(self.revision);
        };
        if summary.origin == Some(origin) {
            return Ok(self.revision);
        }
        summary.origin = Some(origin);
        self.bump()
    }

    /// Removes a row and its title override without changing its forget fence.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn remove(&mut self, expected_revision: u64, key: &str) -> Result<(u64, bool)> {
        self.guard(expected_revision)?;
        let (existed, title_removed) = self.drop_rows(key);
        if existed || title_removed {
            self.bump().map(|revision| (revision, existed))
        } else {
            Ok((self.revision, false))
        }
    }

    /// Removes many rows and their title overrides in one pass with one
    /// revision bump, without changing their forget fences. Returns the new
    /// revision and how many session rows existed.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn remove_many(
        &mut self,
        expected_revision: u64,
        keys: &HashSet<String>,
    ) -> Result<(u64, usize)> {
        self.guard(expected_revision)?;
        let before = self.sessions.len();
        self.sessions
            .retain(|(candidate, _)| !keys.contains(candidate));
        let removed = before - self.sessions.len();
        let title_before = self.titles.len();
        self.titles
            .retain(|(candidate, _)| !keys.contains(candidate));
        let titles_removed = title_before != self.titles.len();
        if removed > 0 {
            self.session_positions = positions(&self.sessions);
        }
        if titles_removed {
            self.title_positions = positions(&self.titles);
        }
        if removed > 0 || titles_removed {
            self.bump().map(|revision| (revision, removed))
        } else {
            Ok((self.revision, 0))
        }
    }

    /// Removes a row and records a durable fence against future listings.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn forget(&mut self, expected_revision: u64, key: &str) -> Result<(u64, bool)> {
        self.guard(expected_revision)?;
        let (existed, title_removed) = self.drop_rows(key);
        let newly_forgotten = self.forgotten_keys.insert(key.to_owned());
        if newly_forgotten {
            self.forgotten.push(key.to_owned());
        }
        if existed || title_removed || newly_forgotten {
            self.bump().map(|revision| (revision, existed))
        } else {
            Ok((self.revision, false))
        }
    }

    /// Drops a key's row and title override, reporting whether each existed.
    fn drop_rows(&mut self, key: &str) -> (bool, bool) {
        let before = self.sessions.len();
        self.sessions.retain(|(candidate, _)| candidate != key);
        let existed = before != self.sessions.len();
        let title_before = self.titles.len();
        self.titles.retain(|(candidate, _)| candidate != key);
        if existed {
            self.session_positions = positions(&self.sessions);
        }
        if title_before != self.titles.len() {
            self.title_positions = positions(&self.titles);
        }
        (existed, title_before != self.titles.len())
    }

    /// Clears a durable forget fence.
    ///
    /// # Errors
    ///
    /// Returns a CAS error when `expected_revision` is stale or the revision
    /// counter is exhausted.
    pub fn remember(&mut self, expected_revision: u64, key: &str) -> Result<u64> {
        self.guard(expected_revision)?;
        if self.forgotten_keys.remove(key) {
            self.forgotten.retain(|candidate| candidate != key);
            self.bump()
        } else {
            Ok(self.revision)
        }
    }

    /// Atomically persists exactly the legacy `{sessions,titles?,forgotten?}`
    /// shape. A changed named inode is never overwritten.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale revision, external drift, a lost lock, or
    /// an unproven durable atomic replacement.
    pub fn save(&mut self, expected_revision: u64) -> Result<()> {
        self.prepare_save(expected_revision)?.write()
    }

    /// Copies the cache image (a clone of the ordered rows, not their JSON)
    /// so the caller can release the registry lock before
    /// [`RegistrySave::write`] serializes, validates and writes it.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict when `expected_revision` is stale.
    pub fn prepare_save(&self, expected_revision: u64) -> Result<RegistrySave> {
        self.guard(expected_revision)?;
        Ok(RegistrySave {
            revision: self.revision,
            sessions: self.sessions.clone(),
            titles: self.titles.clone(),
            forgotten: self.forgotten.clone(),
            cache_path: self.cache_path.clone(),
            unavailable_reason: self.unavailable_reason.clone(),
            writer: Arc::clone(&self.writer),
        })
    }

    fn title_for(&self, key: &str) -> Option<&str> {
        self.title_positions
            .get(key)
            .map(|&index| self.titles[index].1.as_str())
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut SessionSummary> {
        let index = *self.session_positions.get(key)?;
        Some(&mut self.sessions[index].1)
    }

    fn guard(&self, expected_revision: u64) -> Result<()> {
        ensure_current_revision(expected_revision, self.revision)?;
        Ok(())
    }

    fn bump(&mut self) -> Result<u64> {
        let revision = self.bump_activity()?;
        // Never exhausts first: it advances at most as often as `revision`.
        self.content_revision += 1;
        Ok(revision)
    }

    /// A revision step that changed only `last_activity`.
    fn bump_activity(&mut self) -> Result<u64> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| RegistryError::Invalid("registry revision is exhausted".into()))?;
        Ok(self.revision)
    }
}

/// The [`SessionRegistry::list`] order: status band, then first seen. Stable,
/// so equal rows keep their stored order.
pub fn sort_listed(rows: &mut [SessionSummary]) {
    rows.sort_by(|left, right| {
        status_band(left.status)
            .cmp(&status_band(right.status))
            .then_with(|| left.first_seen.cmp(&right.first_seen))
    });
}

fn positions<T>(rows: &[(String, T)]) -> HashMap<String, usize> {
    rows.iter()
        .enumerate()
        .map(|(index, (key, _))| (key.clone(), index))
        .collect()
}

/// Loads a valid registry through the hardened read path without creating,
/// chmodding, quarantining, or rewriting anything.
///
/// # Errors
///
/// Returns an error for unsafe storage, corrupt JSON, or exceeded bounds.
pub fn read_registry_file(
    directory: impl AsRef<Path>,
    file_name: &str,
) -> Result<Vec<SessionSummary>> {
    let directory = PrivateDirectory::open(directory)?;
    let Some(file) = read_bounded(&directory, file_name, REGISTRY_MAX_BYTES)? else {
        return Ok(Vec::new());
    };
    let state = parse_registry(file.bytes(), directory.path().join(file_name))?;
    Ok(state
        .sessions
        .into_iter()
        .map(|(_, summary)| summary)
        .collect())
}

#[derive(Default)]
struct RegistryState {
    sessions: Vec<(String, SessionSummary)>,
    titles: Vec<(String, String)>,
    forgotten: Vec<String>,
}

fn repair_legacy_cache_mode(path: &Path) -> std::io::Result<()> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "registry cache path has no parent",
        )
    })?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if !before.file_type().is_file()
        || !parent_metadata.file_type().is_dir()
        || before.uid() != parent_metadata.uid()
        || before.nlink() != 1
        || before.mode() & 0o777 == 0o600
    {
        return Ok(());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.uid() != before.uid()
        || opened.nlink() != 1
        || opened.dev() != before.dev()
        || opened.ino() != before.ino()
    {
        return Err(std::io::Error::other(
            "registry cache changed while its permissions were being repaired",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let after = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if after.mode() & 0o777 != 0o600
        || named.mode() & 0o777 != 0o600
        || after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || named.dev() != opened.dev()
        || named.ino() != opened.ino()
    {
        return Err(std::io::Error::other(
            "registry cache mode or identity was not stable after repair",
        ));
    }
    Ok(())
}

fn open_cache(
    root: &Path,
    file_name: &str,
) -> std::result::Result<OpenedRegistryCache, OpenCacheFailure> {
    let directory = PrivateDirectory::open_or_create(root)?;
    let (target, loaded) = match read_bounded(&directory, file_name, REGISTRY_MAX_BYTES)? {
        None => (TargetSnapshot::Missing, RegistryState::default()),
        Some(file) => {
            let target = TargetSnapshot::File(file.snapshot().clone());
            match parse_registry(file.bytes(), directory.path().join(file_name)) {
                Ok(state) => (target, state),
                Err(error) if is_corruption(&error) => {
                    let lock = WriterLock::acquire(&directory, REGISTRY_LOCK)
                        .map_err(|error| OpenCacheFailure::WriterLease(error.into()))?;
                    warn_quarantine_stale_registry_temps(&directory, &lock, file_name);
                    let artifact = quarantine_file(
                        &directory,
                        &lock,
                        file_name,
                        file.snapshot(),
                        QuarantineReason::Corrupt,
                        REGISTRY_MAX_BYTES,
                    )?;
                    return Ok(OpenedRegistryCache {
                        quarantined: Some(artifact.path().to_path_buf()),
                        persistence: Some(RegistryPersistence {
                            directory,
                            lock,
                            file_name: file_name.to_owned(),
                            target: TargetSnapshot::Missing,
                        }),
                        loaded: RegistryState::default(),
                        restored: false,
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    let lock = WriterLock::acquire(&directory, REGISTRY_LOCK)
        .map_err(|error| OpenCacheFailure::WriterLease(error.into()))?;
    warn_quarantine_stale_registry_temps(&directory, &lock, file_name);
    let restored = matches!(target, TargetSnapshot::File(_));
    Ok(OpenedRegistryCache {
        persistence: Some(RegistryPersistence {
            directory,
            lock,
            file_name: file_name.to_owned(),
            target,
        }),
        loaded,
        restored,
        quarantined: None,
    })
}

fn warn_quarantine_stale_registry_temps(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    file_name: &str,
) {
    if let Err(error) =
        quarantine_stale_atomic_writes_for(directory, lock, file_name, REGISTRY_MAX_BYTES)
    {
        eprintln!(
            "[registry] could not quarantine stale atomic writes for {file_name}; continuing with open: {error}"
        );
    }
}

#[derive(Deserialize)]
struct RawRegistry {
    #[serde(deserialize_with = "ordered_value_map")]
    sessions: Vec<(String, Value)>,
    #[serde(default)]
    titles: Option<Value>,
    #[serde(default)]
    forgotten: Option<Value>,
}

fn ordered_value_map<'de, D>(deserializer: D) -> std::result::Result<Vec<(String, Value)>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OrderedMapVisitor;
    impl<'de> Visitor<'de> for OrderedMapVisitor {
        type Value = Vec<(String, Value)>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_map(OrderedMapVisitor)
}

fn parse_registry(bytes: &[u8], path: impl AsRef<Path>) -> Result<RegistryState> {
    let path = path.as_ref();
    let limits = registry_limits()?;
    decode_json_value_strict(bytes, limits, path)?;
    let raw: RawRegistry = serde_json::from_slice(bytes)
        .map_err(|error| RegistryError::corrupt(path, format!("invalid cache shape: {error}")))?;
    let mut sessions = Vec::with_capacity(raw.sessions.len());
    for (key, value) in raw.sessions {
        let summary = parse_summary(&value).ok_or_else(|| {
            RegistryError::corrupt(path, "a session has an invalid required field")
        })?;
        sessions.push((key, summary));
    }

    let session_keys = sessions
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<HashSet<_>>();
    let titles = match raw.titles {
        Some(Value::Object(map)) => map
            .into_iter()
            .filter_map(|(key, value)| {
                let title = value.as_str()?;
                (!title.is_empty() && session_keys.contains(key.as_str()))
                    .then(|| (key, title.to_owned()))
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let mut seen = HashSet::new();
    let forgotten = match raw.forgotten {
        Some(Value::Array(values)) => values
            .into_iter()
            .filter_map(|value| match value {
                Value::String(key) => Some(key),
                _ => None,
            })
            .filter(|key| !key.is_empty() && seen.insert(key.clone()))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let forgotten_set = forgotten.iter().map(String::as_str).collect::<HashSet<_>>();
    sessions.retain(|(key, _)| !forgotten_set.contains(key.as_str()));
    let titles = titles
        .into_iter()
        .filter(|(key, _)| !forgotten_set.contains(key.as_str()))
        .collect();
    Ok(RegistryState {
        sessions,
        titles,
        forgotten,
    })
}

fn parse_summary(value: &Value) -> Option<SessionSummary> {
    let map = value.as_object()?;
    let engine = map.get("engine")?.as_str()?.parse::<EngineId>().ok()?;
    let session_id = optional_string(map, "sessionId")?;
    let title = optional_string(map, "title")?;
    let cwd = optional_string(map, "cwd")?;
    let status = match map.get("status")?.as_str()? {
        "working" => SessionStatus::Working,
        "waiting" => SessionStatus::Waiting,
        "idle" => SessionStatus::Idle,
        "failed" => SessionStatus::Failed,
        _ => return None,
    };
    let cloud = map.get("cloud")?.as_bool()?;
    let last_activity = optional_string(map, "lastActivity")?;
    let first_seen = optional_string(map, "firstSeen")?;
    let archived = map.get("archived")?.as_bool()?;
    let effort = match map.get("effort") {
        Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        None | Some(_) => None,
    };
    Some(SessionSummary {
        engine,
        session_id,
        title,
        cwd,
        status,
        cloud,
        model: optional_string(map, "model"),
        effort,
        last_activity,
        first_seen,
        archived,
        size: map.get("size").and_then(Value::as_u64),
        title_from_summary: map.get("titleFromSummary").and_then(Value::as_bool),
        origin: match map.get("origin").and_then(Value::as_str) {
            Some("app") => Some(SessionOrigin::App),
            Some("store") => Some(SessionOrigin::Store),
            _ => None,
        },
        project_root: optional_string(map, "projectRoot"),
        worktree_home: optional_string(map, "worktreeHome"),
        worktree_path: optional_string(map, "worktreePath"),
        setting_sources: optional_strings(map, "settingSources"),
        disallowed_tools: optional_strings(map, "disallowedTools"),
        kind: map
            .get("kind")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<SessionKind>().ok()),
        permission_mode: None,
    })
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)?.as_str().map(ToOwned::to_owned)
}

fn optional_strings(map: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    map.get(key)?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToOwned::to_owned))
        .collect()
}

fn sanitize_summary(summary: &mut SessionSummary) {
    // The Rust contract cannot carry NaN, infinities, or negative sizes. Keep
    // this boundary explicit so callers cannot bypass the legacy repair rule
    // through a future numeric representation.
    if summary
        .setting_sources
        .as_ref()
        .is_some_and(|values| values.len() > 10_000)
    {
        summary.setting_sources = None;
    }
    if summary
        .disallowed_tools
        .as_ref()
        .is_some_and(|values| values.len() > 10_000)
    {
        summary.disallowed_tools = None;
    }
}

fn merge_summary(
    previous: &SessionSummary,
    mut incoming: SessionSummary,
    title_override: Option<&str>,
) -> SessionSummary {
    let stale_worktree_listing = previous.worktree_home.is_some()
        && previous.worktree_path.is_some()
        && previous.cwd == previous.worktree_home.as_deref().unwrap_or_default()
        && incoming.cwd == previous.worktree_path.as_deref().unwrap_or_default();
    let moved_beyond_worktree = previous.worktree_home.is_some()
        && previous.worktree_path.is_some()
        && incoming.cwd != previous.worktree_home.as_deref().unwrap_or_default()
        && incoming.cwd != previous.worktree_path.as_deref().unwrap_or_default();
    let resolved_cwd = if stale_worktree_listing {
        previous.cwd.clone()
    } else {
        incoming.cwd.clone()
    };
    let title = title_override.map_or_else(
        || {
            if placeholder_title(&previous.title, &previous.session_id)
                || provider_named_code_session(previous, &incoming)
            {
                incoming.title.clone()
            } else {
                previous.title.clone()
            }
        },
        ToOwned::to_owned,
    );
    let status = if incoming.status == SessionStatus::Idle
        && matches!(
            previous.status,
            SessionStatus::Working | SessionStatus::Waiting
        ) {
        previous.status
    } else {
        incoming.status
    };
    let origin = if previous.origin == Some(SessionOrigin::App)
        || incoming.origin == Some(SessionOrigin::App)
    {
        Some(SessionOrigin::App)
    } else {
        incoming.origin.or(previous.origin)
    };
    incoming.title = title;
    incoming.cwd.clone_from(&resolved_cwd);
    incoming.status = status;
    incoming.first_seen.clone_from(&previous.first_seen);
    incoming.last_activity = match incoming.last_activity.cmp(&previous.last_activity) {
        Ordering::Less => previous.last_activity.clone(),
        _ => incoming.last_activity,
    };
    incoming.archived = previous.archived;
    incoming.size = incoming.size.or(previous.size);
    incoming.origin = origin;
    incoming.project_root = incoming.project_root.or_else(|| {
        (resolved_cwd == previous.cwd)
            .then(|| previous.project_root.clone())
            .flatten()
    });
    if moved_beyond_worktree {
        incoming.worktree_home = None;
        incoming.worktree_path = None;
    } else {
        incoming.worktree_home = previous.worktree_home.clone().or(incoming.worktree_home);
        incoming.worktree_path = previous.worktree_path.clone().or(incoming.worktree_path);
    }
    incoming.setting_sources = incoming
        .setting_sources
        .or_else(|| previous.setting_sources.clone());
    incoming.disallowed_tools = incoming
        .disallowed_tools
        .or_else(|| previous.disallowed_tools.clone());
    incoming.model = incoming.model.or_else(|| previous.model.clone());
    incoming.effort = incoming.effort.or_else(|| previous.effort.clone());
    incoming.kind = incoming.kind.or(previous.kind);
    incoming
}

/// The provider just named a Code session itself (Claude's summary or custom
/// title, a Codex thread name: `titleFromSummary`) where it had not before.
/// Interface titles a Code launch with its prompt, a placeholder until then;
/// the provider's first own title replaces it. Chat keeps its prompt title,
/// Agent and Voice launches name themselves, and an owner's or a generated
/// title is an override that never reaches this rule.
fn provider_named_code_session(previous: &SessionSummary, incoming: &SessionSummary) -> bool {
    incoming.title_from_summary == Some(true)
        && previous.title_from_summary != Some(true)
        && !incoming.title.trim().is_empty()
        && matches!(
            previous.kind.or(incoming.kind),
            None | Some(SessionKind::Code)
        )
}

fn placeholder_title(title: &str, session_id: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == session_id || title == "Codex session"
}

const fn status_band(status: SessionStatus) -> u8 {
    match status {
        SessionStatus::Waiting => 0,
        SessionStatus::Working => 1,
        SessionStatus::Idle => 2,
        SessionStatus::Failed => 3,
    }
}

fn registry_limits() -> Result<JsonLimits> {
    // About 25 nodes a session: 200,000 refused every save past ~7,900
    // sessions, long before the 16 MiB byte cap.
    Ok(JsonLimits::new(REGISTRY_MAX_BYTES, 64, 1_000_000)?)
}

fn is_corruption(error: &RegistryError) -> bool {
    matches!(
        error,
        RegistryError::Corrupt { .. }
            | RegistryError::Storage(interface_storage::StorageError::Corrupt { .. })
    )
}

/// Serializes ordered `(key, value)` rows as a JSON object in row order.
struct OrderedMap<'a, V>(&'a [(String, V)]);

impl<V> OrderedMap<'_, V> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<V: Serialize> Serialize for OrderedMap<'_, V> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(self.0.iter().map(|(key, value)| (key, value)))
    }
}

#[derive(Serialize)]
struct PersistedRegistry<'a> {
    sessions: OrderedMap<'a, SessionSummary>,
    #[serde(skip_serializing_if = "OrderedMap::is_empty")]
    titles: OrderedMap<'a, String>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    forgotten: &'a [String],
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
