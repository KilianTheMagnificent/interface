use super::{Options, Run, Snapshot, State, valid_id};
use crate::snapshot_file::{SnapshotFile, WriteError, read_snapshot};
use interface_contracts::away::STORE_VERSION;
use interface_storage::{JsonLimits, PrivateDirectory, StorageError};
use std::path::Path;

/// Store version 3 (protocol 47: options may name a reviewer; protocol 45's
/// version 2 named each run's thread). When it does not exist, the newest
/// older file (version 2, else version 1) is read once, never changed, moved
/// or quarantined, and carried over with every newer field absent.
pub(super) const FILE: &str = "project-away.v3.json";
/// The older stores, newest first, with the version each holds.
pub(super) const LEGACY_FILES: [(&str, u8); 2] =
    [("project-away.v2.json", 2), ("project-away.v1.json", 1)];
const FILE_BYTES: usize = 32 * 1024 * 1024;
/// A commit refused at the byte cap. Nothing was written, so this is a plain
/// refusal, never uncertain storage.
pub(super) const HISTORY_FULL: &str = "Away history reached its size limit, so this change was not saved; preserve the records before maintenance.";
pub(super) struct Store {
    file: SnapshotFile,
    pub snapshot: Snapshot,
    /// Set once a commit is refused at the byte cap: new runs and new turn
    /// receipts are refused until restart, while finish and stop edits still
    /// try to save. In memory only; the snapshot shape is unchanged.
    pub(super) history_full: bool,
}
fn limits_for(file_bytes: usize) -> JsonLimits {
    JsonLimits::new(file_bytes, 32, 1_000_000).expect("fixed limits")
}
impl Store {
    pub fn open(root: &Path) -> Result<Self, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (file, record) = SnapshotFile::open::<Snapshot>(
            directory,
            "project-away.lock",
            FILE,
            limits_for(FILE_BYTES),
        )
        .map_err(|e| e.to_string())?;
        // Read under the same lease. An unreadable older store refuses, as an
        // unreadable store always did: its unknown runs are never dropped for
        // a fresh one.
        let (snapshot, carried) = match record {
            Some(snapshot) => (snapshot, false),
            None => match read_legacy(file.directory())? {
                Some(legacy) => (legacy, true),
                None => (
                    Snapshot {
                        version: STORE_VERSION,
                        revision: 0,
                        defaults: Options::default(),
                        presets: vec![],
                        runs: vec![],
                        storage_uncertain: false,
                        cancelled_starts: vec![],
                    },
                    false,
                ),
            },
        };
        validate(&snapshot)?;
        let mut store = Self {
            file,
            snapshot,
            history_full: false,
        };
        let mut next = store.snapshot.clone();
        for run in &mut next.runs {
            if run.state.active() {
                run.state = State::Unknown;
                run.detail = "Interface stopped during this loop. Inspect the project and retained sessions; no work was replayed.".into();
                for turn in &mut run.turns {
                    if turn.state == "started" {
                        turn.state = "unknown".into();
                    }
                }
            }
        }
        // A carried-over store is written at once, so the version 1 file is
        // never read again.
        if carried || next != store.snapshot {
            store.commit(next)?;
        }
        Ok(store)
    }
    pub fn commit(&mut self, mut next: Snapshot) -> Result<(), String> {
        if self.snapshot.storage_uncertain {
            return Err(
                "Away storage is uncertain. Restart and inspect before starting work.".into(),
            );
        }
        next.revision = self
            .snapshot
            .revision
            .checked_add(1)
            .ok_or("Away revision exhausted")?;
        validate(&next)?;
        // Preparation refusals and a write that failed before its rename leave
        // the file as the snapshot says. Only a write that may have landed (or
        // concerns a replaced target) latches.
        match self.file.replace(next) {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                Ok(())
            }
            Err(WriteError::Refused(StorageError::BoundsExceeded { .. })) => {
                self.history_full = true;
                Err(HISTORY_FULL.into())
            }
            Err(WriteError::Refused(error @ StorageError::InvalidData { .. })) => {
                Err(error.to_string())
            }
            Err(error) => {
                self.snapshot.storage_uncertain |= error.is_uncertain();
                Err(format!("Away storage uncertain: {error}"))
            }
        }
    }
    /// Lowers the committed file's byte cap, as a nearly full store has.
    #[cfg(test)]
    pub(super) fn set_file_bytes(&mut self, file_bytes: usize) {
        self.file.set_limits(limits_for(file_bytes));
    }
    pub fn edit(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut Run) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut next = self.snapshot.clone();
        edit(
            next.runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("Unknown Away loop")?,
        )?;
        self.commit(next)
    }
}
/// A version 1 store as version 2: the same runs, presets and receipts, every
/// run's thread and turn ordinal absent (the loop worked in the project's own
/// checkout).
/// The newest older store there is, carried over to the current version;
/// `None` when there is none.
fn read_legacy(directory: &PrivateDirectory) -> Result<Option<Snapshot>, String> {
    for (name, version) in LEGACY_FILES {
        if let Some(legacy) = read_snapshot::<Snapshot>(directory, name, limits_for(FILE_BYTES))
            .map_err(|e| e.to_string())?
            .0
        {
            return carry_over(legacy, version).map(Some);
        }
    }
    Ok(None)
}
/// An older store as the current version: the same runs, presets and
/// receipts. Version 1 had no threads; neither had a reviewer, which means
/// the worker's provider reviews, as it did.
fn carry_over(mut legacy: Snapshot, version: u8) -> Result<Snapshot, String> {
    let reviewer = std::iter::once(&legacy.defaults)
        .chain(legacy.presets.iter().map(|preset| &preset.options))
        .chain(legacy.runs.iter().map(|run| &run.options))
        .any(|options| options.reviewer.is_some());
    let threads = legacy
        .runs
        .iter()
        .any(|run| run.thread.is_some() || run.turns.iter().any(|t| t.thread_turn.is_some()));
    if legacy.version != version || reviewer || (version < 2 && threads) {
        return Err(
            "Away history is unsupported or full; preserve the records before maintenance.".into(),
        );
    }
    legacy.version = STORE_VERSION;
    Ok(legacy)
}
fn validate(snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.version != STORE_VERSION
        || snapshot.runs.len() > 256
        || snapshot.presets.len() > 256
    {
        return Err(
            "Away history is unsupported or full; preserve the records before maintenance.".into(),
        );
    }
    snapshot.defaults.validate()?;
    if snapshot.cancelled_starts.len() > 4096
        || snapshot.cancelled_starts.iter().any(|id| !valid_id(id))
    {
        return Err("Away cancellation receipts are malformed or full".into());
    }
    let mut ids = std::collections::HashSet::new();
    for preset in &snapshot.presets {
        valid_cwd(&preset.cwd)?;
        preset.options.validate()?;
    }
    for run in &snapshot.runs {
        if !valid_id(&run.id)
            || !ids.insert(&run.id)
            || run.turns.len() > 40
            || run.detail.len() > 8192
            || run.deadline_at_ms
                != run
                    .started_at_ms
                    .saturating_add(run.options.duration_seconds * 1000)
        {
            return Err("Malformed Away run; no work admitted.".into());
        }
        valid_cwd(&run.cwd)?;
        run.options.validate()?;
        if let Some(thread) = &run.thread
            && (thread.key.is_empty()
                || thread.key.len() > 512
                || thread.key.chars().any(char::is_control)
                || thread.branch.as_ref().is_some_and(|branch| {
                    branch.len() > 512 || branch.chars().any(char::is_control)
                })
                || valid_cwd(&thread.worktree).is_err())
        {
            return Err("Malformed Away run; no work admitted.".into());
        }
        for turn in &run.turns {
            if turn.iteration == 0
                || turn.iteration > run.options.max_iterations
                || turn.report.len() > 32768
                || !matches!(
                    turn.state.as_str(),
                    "started" | "completed" | "blocked" | "unknown" | "cancelled"
                )
            {
                return Err("Malformed Away turn.".into());
            }
        }
    }
    Ok(())
}
pub(super) fn valid_cwd(cwd: &str) -> Result<(), String> {
    if !Path::new(cwd).is_absolute() || cwd.len() > 4096 || cwd.contains('\0') {
        return Err("Choose an absolute project folder.".into());
    }
    Ok(())
}
