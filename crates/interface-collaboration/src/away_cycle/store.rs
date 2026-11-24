use super::{Config, Iteration, Phase, Run, Status, now_ms, review_identity};
use crate::away::digest;
use crate::snapshot_file::SnapshotFile;
use interface_storage::{
    JsonLimits, PrivateDirectory, StorageError, TargetSnapshot, WriterLock, atomic_write_json,
    read_json,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const FILE: &str = "cycle-run.v1.json";
const CONTROL: &str = "cycle-control.v1.json";
const CONTROL_LOCK: &str = "control.lock";
/// How long an owner's Stop or feedback waits for the controller's apply
/// checkpoint to release the control lock before it reports failure.
const CONTROL_LOCK_WAIT: Duration = Duration::from_secs(3);
const CONTROL_LOCK_RETRY: Duration = Duration::from_millis(50);

/// The control lock for an owner write. The controller holds it briefly
/// across an apply checkpoint; only acquisition is retried while it is held,
/// never an effect, and a refusal says the control was not recorded.
fn owner_control_lock(dir: &PrivateDirectory) -> Result<WriterLock, String> {
    let deadline = Instant::now() + CONTROL_LOCK_WAIT;
    loop {
        match WriterLock::acquire(dir, CONTROL_LOCK) {
            Ok(lock) => return Ok(lock),
            Err(StorageError::LockHeld { .. }) if Instant::now() < deadline => {
                std::thread::sleep(CONTROL_LOCK_RETRY);
            }
            Err(error @ StorageError::LockHeld { .. }) => {
                return Err(format!(
                    "{error}; the Stop/feedback was not recorded. Retry it."
                ));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}
fn limits() -> JsonLimits {
    JsonLimits::new(24 * 1024 * 1024, 64, 1_000_000).expect("bounded limits")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Feedback {
    pub id: String,
    pub message: String,
    pub received_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub version: u8,
    pub revision: u64,
    pub stop: bool,
    pub feedback: Vec<Feedback>,
}

pub struct Store {
    file: SnapshotFile,
    pub(super) run: Run,
}

impl Store {
    /// # Errors
    /// Refuses existing runs; there is deliberately no automatic resume verb.
    pub fn create(root: &Path, config: Config) -> Result<Self, String> {
        config.validate()?;
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (file, existing) = SnapshotFile::open::<Run>(directory, "cycle.lock", FILE, limits())
            .map_err(|e| e.to_string())?;
        let directory = file.directory();
        if existing.is_some()
            || read_json::<Control>(directory, CONTROL, limits())
                .map_err(|e| e.to_string())?
                .is_some()
        {
            return Err("Run already exists; inspect or recover it, never replay it".into());
        }
        let started = now_ms()?;
        let deadline = started
            .checked_add(config.duration_seconds * 1000)
            .ok_or("Deadline overflow")?;
        let control_lock =
            WriterLock::acquire(directory, CONTROL_LOCK).map_err(|e| e.to_string())?;
        atomic_write_json(
            directory,
            &control_lock,
            CONTROL,
            &Control {
                version: 1,
                revision: 0,
                stop: false,
                feedback: vec![],
            },
            &TargetSnapshot::Missing,
            limits(),
        )
        .map_err(|e| e.to_string())?;
        let mut store = Self {
            file,
            run: Run {
                version: 1,
                revision: 0,
                config,
                started_at_ms: started,
                deadline_at_ms: deadline,
                status: Status::Running,
                phase: Phase::Idle,
                detail: String::new(),
                iterations: vec![],
            },
        };
        store.flush()?;
        Ok(store)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.file.directory().path()
    }

    /// # Errors
    /// Rejects unsupported state and altered proposal identities.
    pub fn read(root: &Path) -> Result<Run, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (run, _) = read_json::<Run>(&directory, FILE, limits())
            .map_err(|e| e.to_string())?
            .ok_or("No cycle run in this directory")?
            .into_parts();
        validate(&run)?;
        Ok(run)
    }

    /// # Errors
    /// Missing/corrupt controls fail closed, never silently reset Stop or feedback.
    /// The owner replaces the file atomically while the controller polls it, so a
    /// read that straddles that rename is retried; a fault that persists still fails.
    pub fn control(root: &Path) -> Result<Control, String> {
        let read = || -> Result<Option<Control>, StorageError> {
            let dir = PrivateDirectory::open(root)?;
            Ok(read_json::<Control>(&dir, CONTROL, limits())?.map(|loaded| loaded.into_parts().0))
        };
        let mut result = read();
        for _ in 0..2 {
            if !matches!(
                result,
                Err(StorageError::UnsafeStorage { .. } | StorageError::ExternalDrift { .. })
            ) {
                break;
            }
            result = read();
        }
        let control = result
            .map_err(|e| e.to_string())?
            .ok_or("Cycle controls missing")?;
        check_control(&control)?;
        Ok(control)
    }

    /// # Errors
    /// The same id is idempotent only for the same message. Late feedback is
    /// retained, but cannot alter the immutable deadline or terminal status.
    pub fn feedback(root: &Path, id: &str, message: &str) -> Result<Control, String> {
        if !valid_feedback(id, message) {
            return Err("Feedback requires a 1-64 character id and 1-4096 byte message".into());
        }
        Self::mutate_control(root, |control| {
            if let Some(previous) = control.feedback.iter().find(|f| f.id == id) {
                return if previous.message == message {
                    Ok(())
                } else {
                    Err("Feedback id reused with different content".into())
                };
            }
            if control.feedback.len() >= 32 {
                return Err("Feedback capacity reached; inspect this bounded run".into());
            }
            control.feedback.push(Feedback {
                id: id.into(),
                message: message.into(),
                received_at_ms: now_ms()?,
            });
            control.revision += 1;
            Ok(())
        })
    }

    /// # Errors
    /// Writes Stop durably under the same admission lock as feedback.
    pub fn stop(root: &Path) -> Result<Control, String> {
        Self::mutate_control(root, |control| {
            control.stop = true;
            Ok(())
        })
    }

    fn mutate_control(
        root: &Path,
        edit: impl FnOnce(&mut Control) -> Result<(), String>,
    ) -> Result<Control, String> {
        Self::read(root)?;
        let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let lock = owner_control_lock(&dir)?;
        // One read: the value checked is the one whose snapshot guards the write.
        let (mut control, snapshot) = read_json::<Control>(&dir, CONTROL, limits())
            .map_err(|e| e.to_string())?
            .ok_or("Cycle controls missing")?
            .into_parts();
        check_control(&control)?;
        edit(&mut control)?;
        atomic_write_json(
            &dir,
            &lock,
            CONTROL,
            &control,
            &TargetSnapshot::File(snapshot),
            limits(),
        )
        .map_err(|e| e.to_string())?;
        Ok(control)
    }

    pub(super) fn control_lock(root: &Path) -> Result<WriterLock, String> {
        let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        WriterLock::acquire(&dir, CONTROL_LOCK).map_err(|e| e.to_string())
    }

    /// # Errors
    /// Refuses a live writer. Interrupted effects remain unknown and unreplayed.
    pub fn recover(root: &Path) -> Result<Run, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (file, run) = SnapshotFile::open::<Run>(directory, "cycle.lock", FILE, limits())
            .map_err(|e| e.to_string())?;
        let run = run.ok_or("Cycle state missing")?;
        validate(&run)?;
        let mut store = Self { file, run };
        if store.run.status == Status::Running {
            store.finish(
                Status::Unknown,
                "Interrupted controller; inspect retained effects, no automatic replay",
            )?;
        }
        Ok(store.run)
    }

    pub(super) fn phase(&mut self, phase: Phase) -> Result<(), String> {
        self.run.phase = phase;
        self.flush()
    }
    /// The iteration in progress.
    pub(super) fn current(&mut self) -> Result<&mut Iteration, String> {
        Ok(self.run.iterations.last_mut().ok_or("Iteration missing")?)
    }
    pub(super) fn outcome(&mut self, text: &str) -> Result<(), String> {
        self.current()?.outcome = text.into();
        self.flush()
    }
    pub(super) fn finish(&mut self, status: Status, detail: &str) -> Result<(), String> {
        self.run.status = status;
        self.run.detail = detail.chars().take(4096).collect();
        self.flush()
    }
    pub(super) fn flush(&mut self) -> Result<(), String> {
        self.run.revision += 1;
        validate(&self.run)?;
        self.file.write(&self.run).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn check_control(control: &Control) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    if control.version != 1
        || control.feedback.len() > 32
        || control.revision != control.feedback.len() as u64
        || control
            .feedback
            .iter()
            .any(|f| !valid_feedback(&f.id, &f.message) || !ids.insert(&f.id))
    {
        return Err("Invalid cycle controls".into());
    }
    Ok(())
}

fn valid_feedback(id: &str, message: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        && !message.trim().is_empty()
        && message.len() <= 4096
        && !message.contains('\0')
}

fn validate(run: &Run) -> Result<(), String> {
    run.config.validate()?;
    if run.version != 1
        || run.revision > 10_000
        || run.deadline_at_ms.checked_sub(run.started_at_ms)
            != Some(run.config.duration_seconds * 1000)
        || run.iterations.len() > usize::from(run.config.max_iterations)
        || run.detail.len() > 16_384
    {
        return Err("Invalid cycle state or changed time budget".into());
    }
    for (index, item) in run.iterations.iter().enumerate() {
        if usize::from(item.number) != index + 1
            || item.feedback_revision > 32
            || item.proposal.as_ref().map(digest) != item.proposal_sha256
            || item.outcome.len() > 16_384
        {
            return Err("Cycle proposal integrity mismatch".into());
        }
        if item
            .application
            .as_ref()
            .is_some_and(|a| a["state"] == "applied" || a["state"] == "held")
        {
            let validation = item
                .validation
                .as_ref()
                .ok_or("Accepted cycle has no validation")?;
            let proposal = item
                .proposal
                .as_ref()
                .ok_or("Accepted cycle has no proposal")?;
            let identity = review_identity(proposal, validation, &run.config.validation);
            let review = item.review.as_ref().ok_or("Accepted cycle has no review")?;
            review.validate(&identity, item.feedback_revision)?;
            if validation["passed"] != true
                || !review.approved
                || item.candidate_sha256.as_deref() != Some(&identity)
            {
                return Err("Accepted cycle has mismatched checks or review".into());
            }
        }
    }
    Ok(())
}
