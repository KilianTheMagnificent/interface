#![allow(clippy::missing_errors_doc)]

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use interface_contracts::orchestration::{
    ORCHESTRATION_LIMITS, OrchestrationMutation, OrchestrationSnapshotV1,
    create_empty_orchestration_snapshot, upgrade_orchestration_snapshot,
};
use interface_storage::{
    HARD_MAX_FILE_BYTES, JsonLimits, LoadedJson, PrivateDirectory, Revisioned, StorageError,
    TargetSnapshot, WriterLock, atomic_write_json, compare_and_set_json, read_bounded, read_json,
    snapshot_target,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::reducer::reduce_orchestration_indexed;
use crate::{OrchestrationError, OrchestrationErrorCode, Result};

pub const ORCHESTRATION_STORE_DIRECTORY: &str = "state";
pub const ORCHESTRATION_STORE_RECORDS_DIRECTORY: &str = "records";
pub const ORCHESTRATION_STORE_QUARANTINE_DIRECTORY: &str = "quarantine";
pub const ORCHESTRATION_STORE_FILE: &str =
    "77a5b0b845b1cbdd8405ece9a0fe083955c5fc0c0e0545673093b9058b72fef8.json";
pub const ORCHESTRATION_STORE_LOCK: &str = "orchestration.lock";
pub const ORCHESTRATION_STORE_RECORD_ID: &str = "interface-orchestration-host-snapshot-v1";
pub const ORCHESTRATION_STORE_PAYLOAD_HASH: &str =
    "0ce0af9a319b55c9284a732133661e407101391eff01e3ecf471678f2e1a9014";
pub const ORCHESTRATION_STORE_MAX_BYTES: usize = HARD_MAX_FILE_BYTES;

fn limits() -> Result<JsonLimits> {
    // The reducer keeps 50,000 events plus its entity tables; a 1,000,000-node
    // budget ran out before that window could ever compact.
    JsonLimits::new(ORCHESTRATION_STORE_MAX_BYTES, 128, 4_000_000).map_err(Into::into)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationStoreRecordV1 {
    pub version: u8,
    pub collaboration_id: String,
    pub revision: u64,
    pub state: String,
    pub dedupe_key: String,
    pub payload_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub snapshot: OrchestrationSnapshotV1,
}

impl Revisioned for OrchestrationStoreRecordV1 {
    fn revision(&self) -> u64 {
        self.revision
    }
}

fn record_for(snapshot: OrchestrationSnapshotV1) -> OrchestrationStoreRecordV1 {
    OrchestrationStoreRecordV1 {
        version: 1,
        collaboration_id: ORCHESTRATION_STORE_RECORD_ID.to_owned(),
        revision: snapshot.revision,
        state: "active".to_owned(),
        dedupe_key: ORCHESTRATION_STORE_RECORD_ID.to_owned(),
        payload_hash: ORCHESTRATION_STORE_PAYLOAD_HASH.to_owned(),
        created_at: snapshot.created_at.clone(),
        updated_at: snapshot.updated_at.clone(),
        snapshot,
    }
}

fn validate_record(record: &OrchestrationStoreRecordV1) -> Result<()> {
    if record.version != 1
        || record.collaboration_id != ORCHESTRATION_STORE_RECORD_ID
        || record.state != "active"
        || record.dedupe_key != ORCHESTRATION_STORE_RECORD_ID
        || record.payload_hash != ORCHESTRATION_STORE_PAYLOAD_HASH
    {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::StateCorrupt,
            "orchestration store envelope is invalid",
        ));
    }
    let upgraded = upgrade_orchestration_snapshot(record.snapshot.clone());
    upgraded.validate().map_err(|error| {
        OrchestrationError::new(OrchestrationErrorCode::StateCorrupt, error.to_string())
    })?;
    if upgraded.revision != record.revision
        || upgraded.created_at != record.created_at
        || upgraded.updated_at != record.updated_at
    {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::StateCorrupt,
            "orchestration store envelope disagrees with snapshot",
        ));
    }
    Ok(())
}

fn prefix_identity<T>(
    before: &[T],
    after: &[T],
    identity: impl Fn(&T) -> &str,
    label: &str,
) -> Result<()> {
    if after.len() < before.len() {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::InvalidState,
            format!("{label} cannot remove records"),
        ));
    }
    if before
        .iter()
        .zip(after)
        .any(|(left, right)| identity(left) != identity(right))
    {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::InvalidState,
            format!("{label} cannot reorder or replace identities"),
        ));
    }
    Ok(())
}

/// Verify one append plus the sole permitted oldest-prefix retention compaction.
pub fn assert_orchestration_event_window_transition(
    before: &OrchestrationSnapshotV1,
    after: &OrchestrationSnapshotV1,
) -> Result<()> {
    event_window_transition(before, after, true)
}

/// The window check, comparing every retained event only when `retained` is
/// set; otherwise the counts, sequences, floor and the first retained event's
/// id still have to match.
fn event_window_transition(
    before: &OrchestrationSnapshotV1,
    after: &OrchestrationSnapshotV1,
    retained: bool,
) -> Result<()> {
    let Some(appended) = after.events.last() else {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::InvalidState,
            "CAS must append one event and may compact only the oldest retained prefix",
        ));
    };
    let overflow = (before.events.len() + 1).saturating_sub(ORCHESTRATION_LIMITS.events);
    let expected_len = (before.events.len() + 1).saturating_sub(overflow);
    let first = before.events.get(overflow).unwrap_or(appended);
    let expected_floor = first.sequence.saturating_sub(1);
    if after.events.len() != expected_len
        || after.events[0].event_id != first.event_id
        || (retained && after.events[..after.events.len() - 1] != before.events[overflow..])
        || appended.sequence != before.revision.saturating_add(1)
        || after.revision != before.revision.saturating_add(1)
        || crate::orchestration_event_floor(after) != expected_floor
    {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::InvalidState,
            "CAS must append one event and may compact only the oldest retained prefix",
        ));
    }
    Ok(())
}

fn assert_append_only(
    before: &OrchestrationSnapshotV1,
    after: &OrchestrationSnapshotV1,
    retained_events: bool,
) -> Result<()> {
    event_window_transition(before, after, retained_events)?;
    prefix_identity(
        &before.agents,
        &after.agents,
        |item| &item.agent_id,
        "agents",
    )?;
    prefix_identity(
        &before.messages,
        &after.messages,
        |item| &item.message_id,
        "messages",
    )?;
    prefix_identity(
        &before.role_claims,
        &after.role_claims,
        |item| &item.claim_id,
        "role claims",
    )?;
    prefix_identity(
        &before.effects,
        &after.effects,
        |item| &item.effect_id,
        "effects",
    )?;
    prefix_identity(
        &before.worktree_leases,
        &after.worktree_leases,
        |item| &item.lease_id,
        "worktree leases",
    )?;
    Ok(())
}

fn storage_error(error: StorageError, current_revision: Option<u64>) -> OrchestrationError {
    let current_revision = match &error {
        StorageError::CasConflict { current, .. } => Some(*current),
        _ => current_revision,
    };
    let mut mapped = OrchestrationError::from(error);
    mapped.current_revision = current_revision;
    mapped
}

fn directory_names(directory: &PrivateDirectory) -> Result<Vec<String>> {
    directory.assert_unchanged()?;
    let mut names = Vec::new();
    for entry in fs::read_dir(directory.path()).map_err(|error| {
        OrchestrationError::new(
            OrchestrationErrorCode::UnsafeStorage,
            format!("could not enumerate orchestration journal: {error}"),
        )
    })? {
        let entry = entry.map_err(|error| {
            OrchestrationError::new(
                OrchestrationErrorCode::UnsafeStorage,
                format!("could not enumerate orchestration journal entry: {error}"),
            )
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            OrchestrationError::new(
                OrchestrationErrorCode::StateCorrupt,
                "orchestration journal contains a non-UTF-8 entry name",
            )
        })?;
        names.push(name);
    }
    names.sort_unstable();
    directory.assert_unchanged()?;
    Ok(names)
}

fn is_legacy_temp_name(name: &str) -> bool {
    // The 0.1.169 journal deliberately treats only lower-case `.tmp` as an
    // interrupted write; preserve that byte-for-byte filename contract.
    // (strip_suffix, not ends_with: clippy pedantic forbids case-sensitive
    // extension comparison via ends_with, and this one must stay case-sensitive.)
    name.strip_suffix(".tmp").is_some()
}

fn validate_legacy_layout(
    records: &PrivateDirectory,
    quarantine: &PrivateDirectory,
) -> Result<Vec<String>> {
    let names = directory_names(records)?;
    for name in &names {
        if name != ORCHESTRATION_STORE_FILE && !is_legacy_temp_name(name) {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::StateCorrupt,
                format!("unexpected entry {name} in committed orchestration journal"),
            ));
        }
    }
    for name in directory_names(quarantine)? {
        read_bounded(quarantine, &name, ORCHESTRATION_STORE_MAX_BYTES)?.ok_or_else(|| {
            OrchestrationError::new(
                OrchestrationErrorCode::UnsafeStorage,
                format!("quarantined orchestration artifact disappeared: {name}"),
            )
        })?;
    }
    Ok(names)
}

fn recover_legacy_temps(
    records: &PrivateDirectory,
    quarantine: &PrivateDirectory,
    lock: &WriterLock,
) -> Result<()> {
    let names = validate_legacy_layout(records, quarantine)?;
    for name in names.into_iter().filter(|name| is_legacy_temp_name(name)) {
        lock.assert_held_for(records)?;
        let before =
            read_bounded(records, &name, ORCHESTRATION_STORE_MAX_BYTES)?.ok_or_else(|| {
                OrchestrationError::new(
                    OrchestrationErrorCode::UnsafeStorage,
                    format!("stale orchestration journal temp disappeared: {name}"),
                )
            })?;
        let destination_name = format!("{name}.{}.quarantined", Uuid::new_v4());
        if snapshot_target(quarantine, &destination_name, ORCHESTRATION_STORE_MAX_BYTES)?
            != TargetSnapshot::Missing
        {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::UnsafeStorage,
                "orchestration quarantine destination already exists",
            ));
        }
        let source = records.path().join(&name);
        let destination = quarantine.path().join(&destination_name);
        fs::rename(&source, &destination).map_err(|error| {
            OrchestrationError::new(
                OrchestrationErrorCode::WriteFailed,
                format!("could not quarantine stale orchestration temp: {error}"),
            )
        })?;
        let persisted = (|| -> Result<()> {
            lock.assert_held_for(records)?;
            let installed =
                read_bounded(quarantine, &destination_name, ORCHESTRATION_STORE_MAX_BYTES)?
                    .ok_or_else(|| {
                        OrchestrationError::new(
                            OrchestrationErrorCode::UnsafeStorage,
                            "quarantined orchestration temp is missing",
                        )
                    })?;
            if installed.snapshot() != before.snapshot() || installed.bytes() != before.bytes() {
                return Err(OrchestrationError::new(
                    OrchestrationErrorCode::UnsafeStorage,
                    "quarantined orchestration temp changed identity or bytes",
                ));
            }
            records.sync()?;
            quarantine.sync()?;
            lock.assert_held_for(records)?;
            Ok(())
        })();
        if let Err(error) = persisted {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::WriteUncertain,
                format!("stale orchestration temp may have been quarantined: {error}"),
            ));
        }
    }
    let remaining = directory_names(records)?;
    if remaining
        .iter()
        .any(|name| name != ORCHESTRATION_STORE_FILE)
    {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::UnsafeStorage,
            "orchestration journal changed during stale-temp recovery",
        ));
    }
    Ok(())
}

fn legacy_record_directory_read_only(root: &Path) -> Result<Option<PrivateDirectory>> {
    let root = PrivateDirectory::open(root)?;
    let state_path = root.path().join(ORCHESTRATION_STORE_DIRECTORY);
    match fs::symlink_metadata(&state_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::UnsafeStorage,
                format!("could not inspect orchestration state directory: {error}"),
            ));
        }
        Ok(_) => {}
    }
    let state = root.open_child(ORCHESTRATION_STORE_DIRECTORY)?;
    let records_path = state.path().join(ORCHESTRATION_STORE_RECORDS_DIRECTORY);
    match fs::symlink_metadata(&records_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(OrchestrationError::new(
            OrchestrationErrorCode::UnsafeStorage,
            format!("could not inspect orchestration records directory: {error}"),
        )),
        Ok(_) => Ok(Some(
            state.open_child(ORCHESTRATION_STORE_RECORDS_DIRECTORY)?,
        )),
    }
}

/// Read an existing authoritative record without creating, locking, repairing, or rewriting it.
pub fn read_orchestration_store_read_only(
    root: impl AsRef<Path>,
) -> Result<Option<OrchestrationSnapshotV1>> {
    let Some(directory) = legacy_record_directory_read_only(root.as_ref())? else {
        return Ok(None);
    };
    let Some(record) =
        read_json::<OrchestrationStoreRecordV1>(&directory, ORCHESTRATION_STORE_FILE, limits()?)?
    else {
        return Ok(None);
    };
    validate_record(record.value())?;
    Ok(Some(upgrade_orchestration_snapshot(
        record.value().snapshot.clone(),
    )))
}

/// Owner-private one-writer orchestration snapshot store.
#[derive(Debug)]
pub struct OrchestrationStore {
    root: PathBuf,
    records: PrivateDirectory,
    lock: Option<WriterLock>,
    loaded: LoadedJson<OrchestrationStoreRecordV1>,
    event_index: Option<EventIndex>,
    poisoned: bool,
    closed: bool,
}

/// The ids of the retained events of the snapshot at `revision`, so a
/// mutation's duplicate-event check is a lookup instead of a window scan.
/// Kept across mutations; rebuilt whenever its revision is not the loaded one.
#[derive(Debug)]
struct EventIndex {
    revision: u64,
    ids: HashSet<String>,
}

impl EventIndex {
    fn of(snapshot: &OrchestrationSnapshotV1) -> Self {
        Self {
            revision: snapshot.revision,
            ids: snapshot
                .events
                .iter()
                .map(|event| event.event_id.clone())
                .collect(),
        }
    }
}

impl OrchestrationStore {
    /// Open or initialize the authoritative v1 record in Interface's legacy
    /// `state/records` journal layout. A corrupt committed record is preserved
    /// in place and refused so an older app can still diagnose or recover it.
    pub fn open(
        root: impl AsRef<Path>,
        host_id: impl Into<String>,
        runtime_epoch: impl Into<String>,
        at: impl Into<String>,
    ) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let root_directory = PrivateDirectory::open_or_create(&root)?;
        let state = root_directory.open_or_create_child(ORCHESTRATION_STORE_DIRECTORY)?;
        let records = state.open_or_create_child(ORCHESTRATION_STORE_RECORDS_DIRECTORY)?;
        let quarantine = state.open_or_create_child(ORCHESTRATION_STORE_QUARANTINE_DIRECTORY)?;
        let lock = WriterLock::acquire_for(&root_directory, ORCHESTRATION_STORE_LOCK, &records)?;
        recover_legacy_temps(&records, &quarantine, &lock)?;
        let json_limits = limits()?;
        let loaded = match read_json::<OrchestrationStoreRecordV1>(
            &records,
            ORCHESTRATION_STORE_FILE,
            json_limits,
        ) {
            Ok(Some(loaded)) => {
                validate_record(loaded.value())?;
                loaded
            }
            Ok(None) => {
                let snapshot = create_empty_orchestration_snapshot(
                    host_id.into(),
                    runtime_epoch.into(),
                    at.into(),
                )?;
                atomic_write_json(
                    &records,
                    &lock,
                    ORCHESTRATION_STORE_FILE,
                    &record_for(snapshot),
                    &TargetSnapshot::Missing,
                    json_limits,
                )?
            }
            Err(error) => return Err(storage_error(error, None)),
        };
        Ok(Self {
            root,
            records,
            lock: Some(lock),
            loaded,
            event_index: None,
            poisoned: false,
            closed: false,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn ensure_usable(&self) -> Result<()> {
        if self.closed {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::StoreClosed,
                "orchestration store is closed",
            ));
        }
        if self.poisoned {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::StorePoisoned,
                "orchestration store is poisoned by an uncertain write",
            ));
        }
        Ok(())
    }

    fn ensure_writable(&self) -> Result<&WriterLock> {
        self.ensure_usable()?;
        self.lock.as_ref().ok_or_else(|| {
            OrchestrationError::new(
                OrchestrationErrorCode::StoreClosed,
                "orchestration store is closed",
            )
        })
    }

    pub fn snapshot(&self) -> Result<OrchestrationSnapshotV1> {
        self.ensure_usable()?;
        Ok(upgrade_orchestration_snapshot(
            self.loaded.value().snapshot.clone(),
        ))
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.loaded.value().revision
    }

    /// Retained parity surface: no in-repo caller by design.
    pub fn compare_and_set(
        &mut self,
        expected_revision: u64,
        replacement: OrchestrationSnapshotV1,
    ) -> Result<OrchestrationSnapshotV1> {
        self.compare_and_set_with(expected_revision, |_| Ok(replacement))
    }

    pub fn compare_and_set_with(
        &mut self,
        expected_revision: u64,
        replacement: impl FnOnce(&OrchestrationSnapshotV1) -> Result<OrchestrationSnapshotV1>,
    ) -> Result<OrchestrationSnapshotV1> {
        self.compare_and_set_inner(expected_revision, replacement, true)
    }

    /// The closure gets the stored snapshot by reference: `apply`'s reducer
    /// upgrades its own copy, and the store's checks below read no field the
    /// upgrade fills. `revalidate` is false only for reducer output, which
    /// `reduce_orchestration` has already validated as its last step.
    fn compare_and_set_inner(
        &mut self,
        expected_revision: u64,
        replacement: impl FnOnce(&OrchestrationSnapshotV1) -> Result<OrchestrationSnapshotV1>,
        revalidate: bool,
    ) -> Result<OrchestrationSnapshotV1> {
        let lock = self.ensure_writable()?;
        let current_record = self.loaded.value();
        if expected_revision != current_record.revision {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::CasConflict,
                format!(
                    "stale revision: expected {expected_revision}, current revision is {}",
                    current_record.revision
                ),
            )
            .revision(current_record.revision));
        }
        let before = &current_record.snapshot;
        let next = upgrade_orchestration_snapshot(replacement(before)?);
        if revalidate {
            next.validate().map_err(|error| {
                OrchestrationError::new(OrchestrationErrorCode::InvalidState, error.to_string())
            })?;
        }
        if next.version != before.version
            || next.host_id != before.host_id
            || next.created_at != before.created_at
        {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::InvalidState,
                "CAS cannot change snapshot identity or creation time",
            ));
        }
        if next.revision != before.revision.checked_add(1).unwrap_or(u64::MAX) {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::InvalidState,
                "CAS replacement revision must increase by exactly one",
            ));
        }
        if next.updated_at < before.updated_at {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::InvalidState,
                "CAS replacement cannot move updatedAt backwards",
            ));
        }
        // Reducer output only appends (`append_event`), so release builds skip
        // comparing every retained event of a window up to 50,000 long; debug
        // builds and tests still prove it on every mutation.
        assert_append_only(before, &next, revalidate || cfg!(debug_assertions))?;
        let next_record = record_for(next);
        match compare_and_set_json(
            &self.records,
            lock,
            ORCHESTRATION_STORE_FILE,
            &self.loaded,
            expected_revision,
            &next_record,
            limits()?,
        ) {
            Ok(loaded) => {
                self.loaded = loaded;
                // The snapshot just written; re-reading storage would clone the
                // same all-string/int/bool graph.
                Ok(next_record.snapshot)
            }
            Err(error) => {
                if matches!(error, StorageError::WriteUncertain { .. }) {
                    self.poisoned = true;
                }
                Err(storage_error(error, Some(current_record.revision)))
            }
        }
    }

    pub fn apply(
        &mut self,
        expected_revision: u64,
        mutation: OrchestrationMutation,
    ) -> Result<OrchestrationSnapshotV1> {
        let current = &self.loaded.value().snapshot;
        let mut index = match self.event_index.take() {
            Some(index) if index.revision == current.revision => index,
            _ => EventIndex::of(current),
        };
        // At the window's cap the reducer drops the oldest event as it
        // appends one; those leave the index with it.
        let overflow = (current.events.len() + 1).saturating_sub(ORCHESTRATION_LIMITS.events);
        let dropped = current
            .events
            .iter()
            .take(overflow)
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>();
        let applied = self.compare_and_set_inner(
            expected_revision,
            |current| reduce_orchestration_indexed(current, mutation, &index.ids),
            false,
        );
        if let Ok(next) = &applied {
            for id in &dropped {
                index.ids.remove(id);
            }
            if let Some(appended) = next.events.last() {
                index.ids.insert(appended.event_id.clone());
            }
            index.revision = next.revision;
        }
        // A refused or failed write left the loaded revision, and so the
        // index, as it was.
        self.event_index = Some(index);
        applied
    }

    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        if let Some(lock) = self.lock.take() {
            lock.release(&self.records)?;
        }
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::orchestration::{
        OrchestrationActorKind, OrchestrationActorV1, OrchestrationEntityKind, OrchestrationEventV1,
    };

    fn tick(sequence: u64) -> OrchestrationEventV1 {
        OrchestrationEventV1 {
            version: 1,
            event_id: format!("event_{sequence}"),
            sequence,
            kind: "host.tick".to_owned(),
            entity_kind: OrchestrationEntityKind::Host,
            entity_id: "host_test".to_owned(),
            actor: OrchestrationActorV1 {
                kind: OrchestrationActorKind::Host,
                actor_id: "host_test".to_owned(),
            },
            at: "2026-10-06T00:00:00.000Z".to_owned(),
            summary: "tick".to_owned(),
        }
    }

    #[test]
    fn reducer_output_skips_only_the_retained_event_comparison() {
        let mut before = create_empty_orchestration_snapshot(
            "host_test".to_owned(),
            "epoch".to_owned(),
            "2026-10-06T00:00:00.000Z".to_owned(),
        )
        .unwrap();
        before.revision = 3;
        before.events = vec![tick(1), tick(2), tick(3)];
        let mut after = before.clone();
        after.revision = 4;
        after.events.push(tick(4));
        for retained in [true, false] {
            event_window_transition(&before, &after, retained).unwrap();
        }

        // Only the full check reads a changed middle event.
        let mut edited = after.clone();
        edited.events[1].summary = "edited".to_owned();
        assert!(event_window_transition(&before, &edited, true).is_err());
        event_window_transition(&before, &edited, false).unwrap();

        // Both still refuse a changed first event, a missing append and a
        // wrong sequence.
        let mut first = after.clone();
        first.events[0] = tick(9);
        let mut missing = after.clone();
        missing.events.pop();
        let mut sequence = after.clone();
        sequence.events[3].sequence = 7;
        for broken in [first, missing, sequence] {
            for retained in [true, false] {
                assert!(event_window_transition(&before, &broken, retained).is_err());
            }
        }
    }
}
