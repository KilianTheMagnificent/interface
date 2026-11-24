//! Explicit, bounded Away pilot. Separate v1 store; no ordinary pane delegation
//! or legacy agent state is enabled. Providers only return content proposals.
use crate::{clip, snapshot_file::SnapshotFile};
use async_trait::async_trait;
use interface_storage::{
    JsonLimits, PrivateDirectory, TargetSnapshot, WriterLock, atomic_write_json, read_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

const FILE: &str = "away-run.v1.json";
const MAX_REFINEMENT_FEEDBACK_BYTES: usize = 2_048;
const MAX_INFRASTRUCTURE_ERROR_CHARS: usize = 2_048;
pub type Progress = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub file: String,
    pub prompt: String,
    pub gate: String,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Queued,
    Working,
    Validating,
    Applying,
    Accepted,
    Blocked,
    Rejected,
    Unknown,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attempt {
    pub number: u8,
    pub proposal: Option<Value>,
    pub proposal_sha256: Option<String>,
    pub validation: Option<Value>,
    pub application: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskRecord {
    pub task: Task,
    pub status: Status,
    pub detail: String,
    pub baseline: Option<Value>,
    pub attempts: Vec<Attempt>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    pub version: u8,
    pub revision: u64,
    pub project: PathBuf,
    pub requested_model: String,
    pub max_attempts: u8,
    pub finished: bool,
    pub stopped: bool,
    pub tasks: Vec<TaskRecord>,
}

fn limits() -> JsonLimits {
    JsonLimits::new(8 * 1024 * 1024, 48, 500_000).expect("fixed bounded limits")
}

fn validate(run: &Run) -> Result<(), String> {
    if run.version != 1
        || !run.project.is_absolute()
        || !(1..=3).contains(&run.max_attempts)
        || run.tasks.is_empty()
        || run.tasks.len() > 8
        || run.requested_model.is_empty()
        || run.requested_model.len() > 128
        || run.revision > 1000
    {
        return Err("Invalid or unsupported Away pilot record".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for record in &run.tasks {
        let task = &record.task;
        if task.id.is_empty()
            || task.id.len() > 64
            || !task
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
            || !ids.insert(&task.id)
            || task.prompt.len() > 16_384
            || task.file.len() > 128
            || task.gate.len() > 64
            || record.attempts.len() > usize::from(run.max_attempts)
            || record.detail.len() > 16_384
        {
            return Err("Invalid Away task record".into());
        }
        for (index, attempt) in record.attempts.iter().enumerate() {
            if usize::from(attempt.number) != index + 1
                || attempt.proposal.as_ref().map(digest) != attempt.proposal_sha256
            {
                return Err("Away proposal identity mismatch".into());
            }
        }
        if record.status == Status::Accepted
            && !record.attempts.last().is_some_and(|a| {
                a.validation.as_ref().is_some_and(|v| v["passed"] == true)
                    && a.application
                        .as_ref()
                        .is_some_and(|v| v["state"] == "applied")
                    && a.proposal.is_some()
            })
        {
            return Err("Accepted Away task has no matching evidence".into());
        }
    }
    Ok(())
}

#[must_use]
pub fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

/// Proposal envelopes retain complete provider evidence, but an envelope from
/// `CliDelegate` also carries per-turn metadata. Only its frozen `changes`
/// projection describes the candidate that validation and application receive.
/// Non-envelope pilot executors retain full-value comparison.
fn candidate_identity(proposal: &Value) -> String {
    proposal
        .get("changes")
        .filter(|changes| changes.is_array())
        .map_or_else(|| digest(proposal), digest)
}

async fn wait_for_stop(mut cancel: watch::Receiver<bool>) {
    if !*cancel.borrow() {
        let _ = cancel.changed().await;
    }
}

fn refinement_feedback(attempts: &[Attempt]) -> String {
    const TRUNCATED: &str = "\n[earlier validation feedback truncated]";
    let mut feedback = String::new();
    for attempt in attempts {
        let Some(validation) = &attempt.validation else {
            continue;
        };
        let entry = format!("Attempt {} validation: {validation}\n", attempt.number);
        let remaining = MAX_REFINEMENT_FEEDBACK_BYTES.saturating_sub(feedback.len());
        if entry.len() <= remaining {
            feedback.push_str(&entry);
            continue;
        }
        feedback.push_str(clip::head(
            &entry,
            remaining.saturating_sub(TRUNCATED.len()),
        ));
        feedback.push_str(&TRUNCATED[..remaining.min(TRUNCATED.len())]);
        break;
    }
    feedback
}

fn infrastructure_failure(error: &str) -> Value {
    json!({
        "passed": false,
        "error": error.chars().take(MAX_INFRASTRUCTURE_ERROR_CHARS).collect::<String>(),
    })
}

pub struct Store {
    file: SnapshotFile,
    run: Run,
    poisoned: bool,
}

impl Store {
    pub fn request_stop(root: &Path) -> Result<(), String> {
        Self::read(root)?;
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let lock = WriterLock::acquire(&directory, "stop.lock").map_err(|e| e.to_string())?;
        if read_json::<bool>(&directory, "stop.json", limits())
            .map_err(|e| e.to_string())?
            .is_none()
        {
            atomic_write_json(
                &directory,
                &lock,
                "stop.json",
                &true,
                &TargetSnapshot::Missing,
                limits(),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn stop_requested(root: &Path) -> Result<bool, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        Ok(read_json::<bool>(&directory, "stop.json", limits())
            .map_err(|e| e.to_string())?
            .is_some_and(|value| *value.value()))
    }

    pub fn create(
        root: &Path,
        project: PathBuf,
        model: String,
        tasks: Vec<Task>,
    ) -> Result<Self, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (file, existing) = SnapshotFile::open::<Run>(directory, "away.lock", FILE, limits())
            .map_err(|e| e.to_string())?;
        if existing.is_some() {
            return Err("This Away run already exists. Inspect it; it will not be replayed".into());
        }
        let run = Run {
            version: 1,
            revision: 0,
            project,
            requested_model: model,
            max_attempts: 3,
            finished: false,
            stopped: false,
            tasks: tasks
                .into_iter()
                .map(|task| TaskRecord {
                    task,
                    status: Status::Queued,
                    detail: String::new(),
                    baseline: None,
                    attempts: Vec::new(),
                })
                .collect(),
        };
        let mut store = Self {
            file,
            run,
            poisoned: false,
        };
        store.flush()?;
        Ok(store)
    }

    pub fn read(root: &Path) -> Result<Run, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (run, _) = read_json::<Run>(&directory, FILE, limits())
            .map_err(|e| e.to_string())?
            .ok_or("No Away pilot record in this directory")?
            .into_parts();
        validate(&run)?;
        Ok(run)
    }

    /// Mark interrupted effects unknown under the writer lease. Never dispatch.
    pub fn recover(root: &Path) -> Result<Run, String> {
        let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
        let (file, run) = SnapshotFile::open::<Run>(directory, "away.lock", FILE, limits())
            .map_err(|e| e.to_string())?;
        let mut store = Self {
            file,
            run: run.ok_or("No Away pilot record")?,
            poisoned: false,
        };
        validate(&store.run)?;
        if !store.run.finished {
            for task in &mut store.run.tasks {
                task.status = match task.status {
                    Status::Working | Status::Validating | Status::Applying => Status::Unknown,
                    Status::Queued => Status::Stopped,
                    status => status,
                };
            }
            store.run.finished = true;
            store.run.stopped = true;
            store.flush()?;
        }
        Ok(store.run)
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.poisoned {
            return Err("Away storage requires recovery".into());
        }
        self.run.revision += 1;
        validate(&self.run)?;
        match self.file.write(&self.run) {
            Ok(_) => Ok(()),
            Err(error) => {
                // Every edit lands in `run` before this write, so even a
                // refusal leaves memory ahead of disk: always poison.
                self.poisoned = true;
                Err(error.to_string())
            }
        }
    }

    /// The attempt `drive` claimed last for this task.
    fn attempt(&mut self, index: usize) -> &mut Attempt {
        self.run.tasks[index]
            .attempts
            .last_mut()
            .expect("claimed attempt")
    }

    fn state(
        &mut self,
        index: usize,
        status: Status,
        detail: String,
        progress: Option<&Progress>,
    ) -> Result<(), String> {
        self.run.tasks[index].status = status;
        self.run.tasks[index].detail.clone_from(&detail);
        self.flush()?;
        if let Some(progress) = progress {
            progress(&format!(
                "{}: {status:?} — {detail}",
                self.run.tasks[index].task.id
            ));
        }
        Ok(())
    }
}

/// Adapters execute effects; the coordinator alone decides admission, retry,
/// acceptance and stopping. An error after provider dispatch is always unknown.
#[async_trait]
pub trait Executor: Send + Sync {
    async fn baseline(&self, task: &Task) -> Result<Value, String>;
    async fn propose(
        &self,
        task: &Task,
        number: u8,
        feedback: &str,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, String>;
    async fn validate(&self, task: &Task, proposal: &Value) -> Result<Value, String>;
    async fn apply(&self, task: &Task, proposal: &Value) -> Result<Value, String>;
}

pub async fn drive(
    mut store: Store,
    executor: &dyn Executor,
    cancel: watch::Receiver<bool>,
    progress: Option<Progress>,
) -> Result<Run, String> {
    for index in 0..store.run.tasks.len() {
        if *cancel.borrow() {
            break;
        }
        let task = store.run.tasks[index].task.clone();
        if let Some(reason) = &task.blocked_reason {
            store.state(index, Status::Blocked, reason.clone(), progress.as_ref())?;
            continue;
        }
        let baseline = tokio::select! {
            biased;
            () = wait_for_stop(cancel.clone()) => None,
            result = executor.baseline(&task) => Some(result),
        };
        let Some(baseline) = baseline else {
            break;
        };
        // Each arm's next state() persists the baseline with it, and the
        // demonstrated one is persisted with the claimed attempt before any
        // provider dispatch (or by the final flush when nothing is claimed).
        match baseline {
            Ok(value) if value["passed"].is_boolean() => {
                let demonstrated = value["passed"] == false;
                store.run.tasks[index].baseline = Some(value);
                if !demonstrated {
                    store.state(
                        index,
                        Status::Blocked,
                        "Baseline did not demonstrate the defect".into(),
                        progress.as_ref(),
                    )?;
                    continue;
                }
            }
            failure => {
                let (error, detail) = match failure {
                    Ok(_) => (
                        "Baseline result did not report boolean passed".to_owned(),
                        "Baseline infrastructure failed; no provider dispatch".to_owned(),
                    ),
                    Err(error) => (error.clone(), error),
                };
                store.run.tasks[index].baseline = Some(infrastructure_failure(&error));
                store.state(index, Status::Blocked, detail, progress.as_ref())?;
                continue;
            }
        }
        for number in 1..=store.run.max_attempts {
            if *cancel.borrow() {
                break;
            }
            let feedback = refinement_feedback(&store.run.tasks[index].attempts);
            store.run.tasks[index].attempts.push(Attempt {
                number,
                proposal: None,
                proposal_sha256: None,
                validation: None,
                application: None,
            });
            store.state(
                index,
                Status::Working,
                format!("Attempt {number}; provider dispatch claimed"),
                progress.as_ref(),
            )?;
            let proposal = match tokio::select! {
                biased;
                () = wait_for_stop(cancel.clone()) => None,
                result = tokio::time::timeout(
                    Duration::from_secs(180),
                    executor.propose(&task, number, &feedback, cancel.clone()),
                ) => Some(result),
            } {
                Some(Ok(Ok(value))) => value,
                Some(result) => {
                    let detail = match result {
                        Ok(Err(error)) => error,
                        _ => "Provider deadline; outcome unknown, never replayed".into(),
                    };
                    store.state(index, Status::Unknown, detail, progress.as_ref())?;
                    break;
                }
                None => break,
            };
            let proposal_sha256 = digest(&proposal);
            let candidate_sha256 = candidate_identity(&proposal);
            let repeated = store.run.tasks[index]
                .attempts
                .iter()
                .filter_map(|previous| previous.proposal.as_ref())
                .any(|previous| candidate_identity(previous) == candidate_sha256);
            let attempt = store.attempt(index);
            attempt.proposal_sha256 = Some(proposal_sha256);
            attempt.proposal = Some(proposal.clone());
            if repeated {
                store.state(
                    index,
                    Status::Rejected,
                    format!("Attempt {number} repeated a prior proposal; no progress"),
                    progress.as_ref(),
                )?;
                break;
            }
            store.state(
                index,
                Status::Validating,
                "Checking the exact proposal".into(),
                progress.as_ref(),
            )?;
            if *cancel.borrow() {
                break;
            }
            let validation = match executor.validate(&task, &proposal).await {
                Ok(value) if value["passed"].is_boolean() => value,
                result => {
                    let error = match result {
                        Ok(_) => "Validation result did not report boolean passed".into(),
                        Err(error) => error,
                    };
                    store.attempt(index).validation = Some(infrastructure_failure(&error));
                    store.state(
                        index,
                        Status::Blocked,
                        "Validation infrastructure failed; no application".into(),
                        progress.as_ref(),
                    )?;
                    break;
                }
            };
            let passed = validation["passed"] == true;
            store.attempt(index).validation = Some(validation);
            if !passed {
                store.state(
                    index,
                    Status::Rejected,
                    format!("Attempt {number} failed; retaining evidence"),
                    progress.as_ref(),
                )?;
                continue;
            }
            if *cancel.borrow() {
                break;
            }
            store.state(
                index,
                Status::Applying,
                "Behavior checks passed; compare-and-save candidate".into(),
                progress.as_ref(),
            )?;
            if *cancel.borrow() {
                break;
            }
            let application = executor.apply(&task, &proposal).await;
            let accepted = application
                .as_ref()
                .is_ok_and(|value| value["state"] == "applied");
            store.attempt(index).application = application.ok();
            store.state(
                index,
                if accepted {
                    Status::Accepted
                } else {
                    Status::Unknown
                },
                if accepted {
                    "Validated change applied"
                } else {
                    "Application not confirmed; inspect before further work"
                }
                .into(),
                progress.as_ref(),
            )?;
            // An uncertain write can affect the shared project; stop this run.
            if !accepted {
                store.run.stopped = true;
            }
            break;
        }
        if store.run.stopped {
            break;
        }
    }
    store.run.stopped |= *cancel.borrow();
    if store.run.stopped {
        for record in &mut store.run.tasks {
            if matches!(
                record.status,
                Status::Queued | Status::Validating | Status::Rejected
            ) {
                record.status = Status::Stopped;
            } else if matches!(record.status, Status::Working | Status::Applying) {
                record.status = Status::Unknown;
            }
        }
    }
    store.run.finished = true;
    store.flush()?;
    Ok(store.run)
}

#[cfg(test)]
#[path = "away_tests.rs"]
mod tests;
