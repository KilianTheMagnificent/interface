//! Provider-neutral Away cycle v1. The controller owns admission, feedback and
//! acceptance; model output can neither extend time nor waive a review.
use async_trait::async_trait;
use interface_contracts::delegate_validation::ValidationPlan;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

use crate::away::{Progress, digest};
mod store;
pub use store::{Control, Feedback, Store};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub project: PathBuf,
    pub objective: String,
    pub files: Vec<String>,
    pub context_files: Vec<String>,
    pub validation: ValidationPlan,
    pub worker_model: String,
    pub review_model: String,
    pub duration_seconds: u64,
    pub max_iterations: u8,
}

impl Config {
    /// # Errors
    /// Requires an explicit bounded scope, two model selections and fixed checks.
    pub fn validate(&self) -> Result<(), String> {
        self.validation.validate()?;
        if self.version != 1
            || !self.project.is_absolute()
            || self.objective.trim().is_empty()
            || self.objective.len() > 16_384
            || self.files.is_empty()
            || self.files.len() + self.context_files.len() > 48
            || !(1..=86_400).contains(&self.duration_seconds)
            || !(1..=20).contains(&self.max_iterations)
            || self
                .files
                .iter()
                .any(|p| self.validation.protected_files.contains(p))
            || self
                .validation
                .protected_files
                .iter()
                .any(|p| !self.context_files.contains(p))
        {
            return Err("Cycle v1 requires explicit files, protected checks, 1-86400 seconds and 1-20 iterations".into());
        }
        for model in [&self.worker_model, &self.review_model] {
            let name = model
                .strip_prefix("codex/")
                .or_else(|| model.strip_prefix("claude/"));
            if name.is_none_or(|s| {
                s.is_empty()
                    || s.len() > 100
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            }) {
                return Err("Select explicit codex/<model> or claude/<model> for worker and review; no fallback".into());
            }
        }
        let mut paths = std::collections::BTreeSet::new();
        for path in self.files.iter().chain(&self.context_files) {
            if path.is_empty() || path.len() > 1024 || !paths.insert(path.to_lowercase()) {
                return Err("Cycle scope contains empty, oversized or aliased paths".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Running,
    Completed,
    Stopped,
    Expired,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Proposing,
    Validating,
    Reviewing,
    Applying,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finding {
    pub id: String,
    pub target: FindingTarget,
    pub evidence: String,
    pub requested_change: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FindingTarget {
    Candidate,
    Validation,
    Scope,
}

fn review_identity(proposal: &Value, validation: &Value, plan: &ValidationPlan) -> String {
    digest(&json!({"changes":proposal["changes"],"validation":validation,"plan":plan}))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub version: u8,
    pub candidate_sha256: String,
    pub feedback_revision: u64,
    pub approved: bool,
    pub summary: String,
    pub findings: Vec<Finding>,
}

impl Review {
    fn validate(&self, candidate: &str, revision: u64) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();
        if self.version != 1
            || self.candidate_sha256 != candidate
            || self.feedback_revision != revision
            || self.summary.trim().is_empty()
            || self.summary.len() > 4096
            || self.findings.len() > 12
            || self.approved != self.findings.is_empty()
            || self.findings.iter().any(|f| {
                f.id.is_empty()
                    || f.id.len() > 64
                    || !ids.insert(&f.id)
                    || f.evidence.trim().is_empty()
                    || f.evidence.len() > 2048
                    || f.requested_change.trim().is_empty()
                    || f.requested_change.len() > 2048
            })
        {
            return Err("Review is malformed, stale, or lacks actionable evidence".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Iteration {
    pub number: u8,
    pub feedback_revision: u64,
    pub proposal: Option<Value>,
    pub proposal_sha256: Option<String>,
    pub candidate_sha256: Option<String>,
    pub validation: Option<Value>,
    pub review: Option<Review>,
    pub application: Option<Value>,
    pub outcome: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    pub version: u8,
    pub revision: u64,
    pub config: Config,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
    pub status: Status,
    pub phase: Phase,
    pub detail: String,
    pub iterations: Vec<Iteration>,
}

/// # Errors
/// Refuses unavailable or unrepresentable system time.
pub fn now_ms() -> Result<u64, String> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis(),
    )
    .map_err(|e| e.to_string())
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn propose(
        &self,
        number: u8,
        feedback: &Value,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, String>;
    async fn validate(&self, number: u8, proposal: &Value) -> Result<Value, String>;
    async fn review(
        &self,
        number: u8,
        proposal: &Value,
        validation: &Value,
        candidate: &str,
        feedback: &Value,
        cancel: watch::Receiver<bool>,
    ) -> Result<Review, String>;
    /// Application is a short compare-and-save checkpoint. Do not cancel it
    /// midway; never admit it after Stop, expiry, or a feedback revision change.
    async fn apply(&self, proposal: &Value, validation: &Value) -> Result<Value, String>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Acknowledgment {
    feedback_revision: u64,
    addressed_feedback_ids: Vec<String>,
    addressed_review_ids: Vec<String>,
    summary: String,
}

fn acknowledgment(proposal: &Value, feedback: &Value) -> Result<String, String> {
    let outer: Value = serde_json::from_str(
        proposal["summary"]
            .as_str()
            .ok_or("Proposal summary missing")?,
    )
    .map_err(|_| "Proposal summary is not structured")?;
    if outer["outcome"] != "changes" {
        return Err("Worker needs context or is blocked; inspect its retained proposal".into());
    }
    let ack: Acknowledgment = serde_json::from_str(
        outer["summary"]
            .as_str()
            .ok_or("Feedback acknowledgment missing")?,
    )
    .map_err(|_| "Feedback acknowledgment is malformed")?;
    let expected = feedback["messages"]
        .as_array()
        .ok_or("Feedback messages missing")?
        .iter()
        .map(|v| v["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    let findings = feedback["findings"]
        .as_array()
        .ok_or("Review findings missing")?
        .iter()
        .map(|v| v["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    if Some(ack.feedback_revision) != feedback["revision"].as_u64()
        || ack.addressed_feedback_ids != expected
        || ack.addressed_review_ids != findings
        || ack.summary.trim().is_empty()
        || ack.summary.len() > 4096
    {
        return Err("Worker did not acknowledge the exact feedback and review findings".into());
    }
    Ok(ack.summary)
}

fn feedback(run: &Run, control: &Control) -> Value {
    // Newest failed check and unresolved review stay in full within their own
    // bounds. Older attempt outcomes are compact; they cannot crowd them out.
    let mut recent = run.iterations.iter().rev().take_while(|i| {
        !i.application
            .as_ref()
            .is_some_and(|a| a["state"] == "applied")
    });
    let unresolved = recent
        .clone()
        .find_map(|i| i.review.as_ref().filter(|r| !r.approved));
    let latest_validation = recent.find_map(|i| i.validation.as_ref());
    json!({"revision":control.revision,"messages":control.feedback,
        "findings":unresolved.map(|r| r.findings.clone()).unwrap_or_default(),
        "latestValidation":latest_validation,
        "history":run.iterations.iter().rev().take(8).map(|i| json!({"number":i.number,"outcome":i.outcome.chars().take(512).collect::<String>(),"candidateSha256":i.candidate_sha256})).collect::<Vec<_>>()})
}

async fn bounded<T>(
    store: &Store,
    deadline: tokio::time::Instant,
    cancel: &mut watch::Receiver<bool>,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::pin!(work);
    loop {
        if *cancel.borrow() || Store::control(store.root())?.stop {
            return Err("Stop requested".into());
        }
        if tokio::time::Instant::now() >= deadline || now_ms()? >= store.run.deadline_at_ms {
            return Err("Cycle deadline reached".into());
        }
        tokio::select! {
            biased;
            changed = cancel.changed() => { if changed.is_err() { return Err("Controller cancellation channel closed".into()); } }
            () = tokio::time::sleep_until(deadline) => return Err("Cycle deadline reached".into()),
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
            result = &mut work => return result,
        }
    }
}

fn stopped(
    store: &Store,
    deadline: tokio::time::Instant,
    cancel: &watch::Receiver<bool>,
) -> Result<Option<Status>, String> {
    if *cancel.borrow() || Store::control(store.root())?.stop {
        return Ok(Some(Status::Stopped));
    }
    if tokio::time::Instant::now() >= deadline || now_ms()? >= store.run.deadline_at_ms {
        return Ok(Some(Status::Expired));
    }
    Ok(None)
}

/// # Errors
/// Storage failure never permits another effect. Provider ambiguity is retained
/// as Unknown and is never automatically replayed.
#[allow(clippy::too_many_lines)]
pub async fn drive(
    mut store: Store,
    executor: &dyn Executor,
    mut cancel: watch::Receiver<bool>,
    progress: Option<Progress>,
) -> Result<Run, String> {
    let remaining = store
        .run
        .deadline_at_ms
        .saturating_sub(now_ms()?)
        .min(store.run.config.duration_seconds * 1000);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
    for number in 1..=store.run.config.max_iterations {
        if let Some(status) = stopped(&store, deadline, &cancel)? {
            store.finish(status, "Admission closed")?;
            break;
        }
        let control = Store::control(store.root())?;
        let context = feedback(&store.run, &control);
        store.run.iterations.push(Iteration {
            number,
            feedback_revision: control.revision,
            proposal: None,
            proposal_sha256: None,
            candidate_sha256: None,
            validation: None,
            review: None,
            application: None,
            outcome: "proposal dispatch claimed".into(),
        });
        store.phase(Phase::Proposing)?;
        if let Some(p) = &progress {
            p(&format!(
                "Away iteration {number}: proposal; feedback revision {}",
                control.revision
            ));
        }
        let result = iteration(
            &mut store,
            executor,
            number,
            &context,
            control.revision,
            deadline,
            &mut cancel,
        )
        .await;
        if let Err(error) = result {
            if store.run.phase == Phase::Validating {
                store.current()?.validation = Some(
                    json!({"version":1,"passed":false,"infrastructureError":error.chars().take(4096).collect::<String>()}),
                );
            }
            let terminal = stopped(&store, deadline, &cancel)?.unwrap_or({
                if matches!(
                    store.run.phase,
                    Phase::Proposing | Phase::Reviewing | Phase::Applying
                ) {
                    Status::Unknown
                } else {
                    Status::Blocked
                }
            });
            // A cancelled provider's delivery remains unknown even when the
            // run ends at Stop/deadline. No subsequent iteration is dispatched.
            let phase = store.run.phase;
            store.current()?.outcome = format!("{phase:?}: {error}");
            store.finish(terminal, &error)?;
            break;
        }
        if store.run.status != Status::Running {
            break;
        }
        store.phase(Phase::Idle)?;
    }
    if store.run.status == Status::Running {
        let accepted = store.run.iterations.last().is_some_and(|i| {
            i.application
                .as_ref()
                .is_some_and(|a| a["state"] == "applied")
        });
        store.finish(
            if accepted {
                Status::Completed
            } else {
                Status::Blocked
            },
            "Iteration limit reached; inspect accepted and rejected outcomes",
        )?;
    }
    Ok(store.run)
}

/// `revision` is the feedback revision `context` was built from.
#[allow(clippy::too_many_lines)]
async fn iteration(
    store: &mut Store,
    executor: &dyn Executor,
    number: u8,
    context: &Value,
    revision: u64,
    deadline: tokio::time::Instant,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let proposal = bounded(
        store,
        deadline,
        cancel,
        executor.propose(number, context, cancel.clone()),
    )
    .await?;
    let full_hash = digest(&proposal);
    {
        let item = store.current()?;
        item.proposal = Some(proposal.clone());
        item.proposal_sha256 = Some(full_hash);
    }
    store.flush()?;
    // The provider completed; malformed acknowledgments are known rejection,
    // never an implicit feedback acknowledgment or an ambiguous-effect retry.
    if let Err(error) = acknowledgment(&proposal, context) {
        store.finish(Status::Blocked, &error)?;
        return Ok(());
    }
    if Store::control(store.root())?.revision != revision {
        store.outcome("New feedback invalidated proposal before validation")?;
        return Ok(());
    }
    let changes = proposal["changes"]
        .as_array()
        .ok_or("Proposal changes missing")?;
    if changes.is_empty() {
        if context["findings"]
            .as_array()
            .is_some_and(|findings| !findings.is_empty())
        {
            store.finish(
                Status::Blocked,
                "Worker returned no repair for unresolved review findings",
            )?;
            return Ok(());
        }
        store.finish(
            Status::Completed,
            "Worker found no further change in the authorized scope",
        )?;
        return Ok(());
    }
    let identity = digest(&proposal["changes"]);
    if store
        .run
        .iterations
        .iter()
        .take(store.run.iterations.len() - 1)
        .any(|i| {
            i.feedback_revision == revision
                && i.proposal
                    .as_ref()
                    .is_some_and(|p| digest(&p["changes"]) == identity)
        })
    {
        store.finish(Status::Blocked, "Repeated candidate; no progress")?;
        return Ok(());
    }
    store.phase(Phase::Validating)?;
    let validation = bounded(
        store,
        deadline,
        cancel,
        executor.validate(number, &proposal),
    )
    .await?;
    store.current()?.validation = Some(validation.clone());
    store.flush()?;
    if !validation["passed"].is_boolean() {
        return Err("Malformed validation result".into());
    }
    if validation["passed"] != true {
        store.outcome("Checks failed; refining from retained evidence")?;
        return Ok(());
    }
    let candidate = review_identity(&proposal, &validation, &store.run.config.validation);
    store.current()?.candidate_sha256 = Some(candidate.clone());
    if Store::control(store.root())?.revision != revision {
        store.outcome("New feedback invalidated candidate before review")?;
        return Ok(());
    }
    store.phase(Phase::Reviewing)?;
    if let Some(status) = stopped(store, deadline, cancel)? {
        store.finish(status, "Review admission closed")?;
        return Ok(());
    }
    let review = bounded(
        store,
        deadline,
        cancel,
        executor.review(
            number,
            &proposal,
            &validation,
            &candidate,
            context,
            cancel.clone(),
        ),
    )
    .await?;
    store.current()?.review = Some(review.clone());
    store.flush()?;
    review.validate(&candidate, revision)?;
    if !review.approved {
        store.outcome("Review requested changes; findings retained for refinement")?;
        if review
            .findings
            .iter()
            .any(|f| f.target != FindingTarget::Candidate)
        {
            store.finish(Status::Blocked,"Review requires validation or scope changes outside the candidate worker's authority")?;
        }
        return Ok(());
    }
    // Serialize admission with feedback/Stop writers. Feedback received after
    // this checkpoint is for the next iteration; it cannot race acceptance.
    let guard = Store::control_lock(store.root())?;
    if let Some(status) = stopped(store, deadline, cancel)? {
        store.finish(status, "Application admission closed")?;
        return Ok(());
    }
    if Store::control(store.root())?.revision != review.feedback_revision {
        store.outcome("New feedback invalidated review before application")?;
        return Ok(());
    }
    store.phase(Phase::Applying)?;
    let application = executor.apply(&proposal, &validation).await?;
    store.current()?.application = Some(application.clone());
    store.flush()?;
    drop(guard);
    if application["state"] == "held" {
        store.outcome("Checked candidate held for an explicit lead decision")?;
        store.finish(Status::Completed, "Candidate held; no source files applied")?;
        return Ok(());
    }
    if application["state"] != "applied" {
        return Err("Application not confirmed; inspect receipt without replay".into());
    }
    store.outcome("Accepted after checks and fresh review")?;
    Ok(())
}

#[cfg(test)]
mod tests;
