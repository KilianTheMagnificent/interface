use super::*;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        file: format!("{id}.py"),
        prompt: "Repair a demonstrated defect".into(),
        gate: id.into(),
        blocked_reason: None,
    }
}
fn store(root: &Path, tasks: Vec<Task>) -> Store {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    Store::create(root, root.join("project"), "codex/test".into(), tasks).unwrap()
}

#[test]
fn written_bytes_match_the_pinned_format() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(FILE);
    let mut blocked = task("two");
    blocked.blocked_reason = Some("Needs a fixture".into());
    let created = store(dir.path(), vec![task("one"), blocked]);
    crate::golden::assert_golden("away.created.json", dir.path(), &file, &[]);
    drop(created);
    // Recovery stops the unfinished run and rewrites it once.
    Store::recover(dir.path()).unwrap();
    crate::golden::assert_golden("away.recovered.json", dir.path(), &file, &[]);
    Store::request_stop(dir.path()).unwrap();
    crate::golden::assert_golden(
        "away.stop.json",
        dir.path(),
        &dir.path().join("stop.json"),
        &[],
    );
    assert!(dir.path().join("away.lock").exists());
}

#[test]
fn refinement_feedback_is_utf8_safe_and_bounded() {
    let attempts = vec![Attempt {
        number: 1,
        proposal: None,
        proposal_sha256: None,
        validation: Some(json!({"passed":false,"error":"é".repeat(4_000)})),
        application: None,
    }];
    let feedback = refinement_feedback(&attempts);
    assert!(feedback.len() <= MAX_REFINEMENT_FEEDBACK_BYTES);
    assert!(feedback.contains("[earlier validation feedback truncated]"));
}

struct Mock {
    calls: AtomicUsize,
    validations: AtomicUsize,
    applied: AtomicUsize,
    feedback: Mutex<Vec<String>>,
    baseline_passes: bool,
    baseline_error: bool,
    baseline_invalid: bool,
    non_cooperative_baseline: bool,
    baseline_started: Option<Arc<Notify>>,
    first_invalid: bool,
    always_invalid: bool,
    distinct_validation_feedback: bool,
    validation_error: bool,
    duplicate_proposal: bool,
    volatile_cli_metadata: bool,
    non_cooperative_proposal: bool,
    proposal_started: Option<Arc<Notify>>,
    unknown_apply: bool,
    unknown: bool,
    cancel: Option<watch::Sender<bool>>,
}
impl Mock {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            validations: AtomicUsize::new(0),
            applied: AtomicUsize::new(0),
            feedback: Mutex::new(Vec::new()),
            baseline_passes: false,
            baseline_error: false,
            baseline_invalid: false,
            non_cooperative_baseline: false,
            baseline_started: None,
            first_invalid: false,
            always_invalid: false,
            distinct_validation_feedback: false,
            validation_error: false,
            duplicate_proposal: false,
            volatile_cli_metadata: false,
            non_cooperative_proposal: false,
            proposal_started: None,
            unknown_apply: false,
            unknown: false,
            cancel: None,
        }
    }
}
#[async_trait]
impl Executor for Mock {
    async fn baseline(&self, _: &Task) -> Result<Value, String> {
        if let Some(started) = &self.baseline_started {
            started.notify_one();
        }
        if self.non_cooperative_baseline {
            return std::future::pending::<Result<Value, String>>().await;
        }
        if self.baseline_error {
            return Err("baseline evaluator unavailable".into());
        }
        if self.baseline_invalid {
            return Ok(json!({"error":"missing baseline pass/fail result"}));
        }
        Ok(json!({"passed":self.baseline_passes}))
    }
    async fn propose(
        &self,
        task: &Task,
        number: u8,
        feedback: &str,
        _: watch::Receiver<bool>,
    ) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.feedback.lock().unwrap().push(feedback.into());
        if let Some(started) = &self.proposal_started {
            started.notify_one();
        }
        if self.non_cooperative_proposal {
            return std::future::pending::<Result<Value, String>>().await;
        }
        if self.unknown {
            return Err("lost response after send".into());
        }
        if self.volatile_cli_metadata {
            return Ok(cli_delegate_proposal(task, number));
        }
        if self.duplicate_proposal {
            return Ok(json!({"task":task.id,"candidate":"unchanged"}));
        }
        Ok(json!({"task":task.id,"attempt":number}))
    }
    async fn validate(&self, _: &Task, proposal: &Value) -> Result<Value, String> {
        self.validations.fetch_add(1, Ordering::SeqCst);
        if let Some(cancel) = &self.cancel {
            let _ = cancel.send(true);
        }
        if self.validation_error {
            return Err("evaluator transport unavailable".into());
        }
        let error = if self.distinct_validation_feedback {
            format!("failure from attempt {}", proposal["attempt"])
        } else {
            "edge case failed".into()
        };
        Ok(
            json!({"passed": !(self.always_invalid || (self.first_invalid && proposal["attempt"] == 1)), "error":error}),
        )
    }
    async fn apply(&self, _: &Task, _: &Value) -> Result<Value, String> {
        self.applied.fetch_add(1, Ordering::SeqCst);
        if self.unknown_apply {
            return Err("write outcome uncertain".into());
        }
        Ok(json!({"state":"applied"}))
    }
}

/// Mirrors the `CliDelegate` proposal envelope. Its full metadata changes on
/// each provider turn, but its candidate `changes` projection is identical.
fn cli_delegate_proposal(task: &Task, number: u8) -> Value {
    json!({
        "quality": {
            "version": 1,
            "outcome": "proposal",
            "eligibleForApproval": true,
            "eligibleForValidation": true,
            "summary": format!("Provider attempt {number} repaired the file"),
            "missingFiles": [],
            "recovery": {"action": "validateThenReview"}
        },
        "elapsedMs": u64::from(number) * 100,
        "usage": {
            "inputTokens": u64::from(number) * 1_000,
            "outputTokens": u64::from(number) * 100,
            "reasoningTokens": u64::from(number) * 10,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
            "costUsd": 0.01 * f64::from(number)
        },
        "summary": format!("Provider summary for attempt {number}"),
        "toolErrorCount": number,
        "noChanges": false,
        "sessionId": format!("provider-session-{number}"),
        "requestedModel": "codex/gpt-5.6-sol",
        "actualModel": format!("provider-native-{number}"),
        "changes": [{
            "path": task.file,
            "before": "def repair():\n    return 0\n",
            "beforeSha256": "a".repeat(64),
            "after": "def repair():\n    return 42\n",
            "sourceChanged": false,
            "beforeMode": 420,
            "afterMode": 420
        }],
        "checks": "Not run in the worker packet. The parent must validate the proposal in a disposable copy, review it, apply accepted changes, and run integration checks.",
        "integration": "proposalOnly"
    })
}

#[tokio::test]
async fn repairs_failed_validation_skips_blocker_and_continues_without_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut blocked = task("blocked");
    blocked.blocked_reason = Some("owner decision required".into());
    let mut mock = Mock::new();
    mock.first_invalid = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(
        store(dir.path(), vec![task("one"), blocked, task("two")]),
        &mock,
        cancel,
        None,
    )
    .await
    .unwrap();
    assert!(result.finished);
    assert_eq!(
        result.tasks.iter().map(|t| t.status).collect::<Vec<_>>(),
        vec![Status::Accepted, Status::Blocked, Status::Accepted]
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 4);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 2);
    assert!(mock.feedback.lock().unwrap()[1].contains("edge case failed"));
    assert_eq!(Store::read(dir.path()).unwrap().tasks[0].attempts.len(), 2);
}

#[tokio::test]
async fn passing_baseline_is_retained_when_task_is_blocked_without_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.baseline_passes = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    assert_eq!(result.tasks[0].status, Status::Blocked);
    assert_eq!(result.tasks[0].baseline, Some(json!({"passed":true})));
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        Store::read(dir.path()).unwrap().tasks[0].baseline,
        Some(json!({"passed":true}))
    );
}

#[tokio::test]
async fn baseline_infrastructure_failure_is_retained_before_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.baseline_error = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    let record = &result.tasks[0];
    assert_eq!(record.status, Status::Blocked);
    assert_eq!(record.detail, "baseline evaluator unavailable");
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(
        record.baseline,
        Some(json!({"passed":false,"error":"baseline evaluator unavailable"}))
    );
    assert_eq!(
        Store::read(dir.path()).unwrap().tasks[0].baseline,
        record.baseline
    );
}

#[tokio::test]
async fn malformed_baseline_result_is_retained_as_infrastructure_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.baseline_invalid = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    let record = &result.tasks[0];
    assert_eq!(record.status, Status::Blocked);
    assert_eq!(
        record.detail,
        "Baseline infrastructure failed; no provider dispatch"
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(
        record.baseline,
        Some(json!({
            "passed":false,
            "error":"Baseline result did not report boolean passed"
        }))
    );
    assert_eq!(
        Store::read(dir.path()).unwrap().tasks[0].baseline,
        record.baseline
    );
}

#[tokio::test]
async fn stop_interrupts_a_non_cooperative_baseline_without_provider_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    let baseline_started = Arc::new(Notify::new());
    mock.non_cooperative_baseline = true;
    mock.baseline_started = Some(baseline_started.clone());
    let (stop, cancel) = watch::channel(false);
    let drive = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None);
    tokio::pin!(drive);
    tokio::select! {
        () = baseline_started.notified() => stop.send(true).unwrap(),
        result = &mut drive => panic!("baseline unexpectedly finished: {result:?}"),
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), &mut drive)
        .await
        .expect("Stop should interrupt a non-cooperative baseline")
        .unwrap();
    assert!(result.finished);
    assert!(result.stopped);
    assert_eq!(result.tasks[0].status, Status::Stopped);
    assert!(result.tasks[0].attempts.is_empty());
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn validation_infrastructure_failure_is_retained_before_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.validation_error = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    let record = &result.tasks[0];
    assert_eq!(record.status, Status::Blocked);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(
        record.attempts[0].validation,
        Some(json!({"passed":false,"error":"evaluator transport unavailable"}))
    );
}

#[tokio::test]
async fn uncertain_provider_effect_is_retained_and_never_retried() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.unknown = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    assert_eq!(result.tasks[0].status, Status::Unknown);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert!(
        Store::create(
            dir.path(),
            dir.path().join("project"),
            "codex/test".into(),
            vec![task("one")]
        )
        .is_err()
    );
}

#[tokio::test]
async fn stop_interrupts_a_non_cooperative_provider_without_reclassifying_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let mut mock = Mock::new();
    mock.non_cooperative_proposal = true;
    mock.proposal_started = Some(Arc::clone(&started));
    let mock = Arc::new(mock);
    let (stop, cancel) = watch::channel(false);
    let worker = tokio::spawn({
        let mock = Arc::clone(&mock);
        async move {
            drive(
                store(dir.path(), vec![task("one"), task("two")]),
                mock.as_ref(),
                cancel,
                None,
            )
            .await
        }
    });
    started.notified().await;
    stop.send(true).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .expect("Stop should interrupt the provider wait")
        .unwrap()
        .unwrap();
    assert!(result.finished && result.stopped);
    assert_eq!(result.tasks[0].status, Status::Unknown);
    assert_eq!(result.tasks[1].status, Status::Stopped);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stop_during_validation_prevents_application_and_next_task() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, cancel) = watch::channel(false);
    let mut mock = Mock::new();
    mock.cancel = Some(tx);
    let result = drive(
        store(dir.path(), vec![task("one"), task("two")]),
        &mock,
        cancel,
        None,
    )
    .await
    .unwrap();
    assert!(result.stopped && result.finished);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert!(result.tasks.iter().all(|t| t.status == Status::Stopped));
}

#[tokio::test]
async fn preexisting_stop_never_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::new();
    let (_tx, cancel) = watch::channel(true);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    assert!(result.stopped);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn writer_lease_and_crash_recovery_preserve_unknown_without_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store(dir.path(), vec![task("one"), task("two")]);
    store
        .state(0, Status::Working, "dispatched".into(), None)
        .unwrap();
    assert!(Store::recover(dir.path()).is_err());
    drop(store);
    let recovered = Store::recover(dir.path()).unwrap();
    assert_eq!(recovered.tasks[0].status, Status::Unknown);
    assert_eq!(recovered.tasks[1].status, Status::Stopped);
    assert!(recovered.finished && recovered.stopped);
    assert_eq!(
        Store::recover(dir.path()).unwrap().revision,
        recovered.revision
    );
}

#[test]
fn stop_signal_is_durable_and_does_not_rewrite_run_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), vec![task("one")]);
    let revision = Store::read(dir.path()).unwrap().revision;
    Store::request_stop(dir.path()).unwrap();
    Store::request_stop(dir.path()).unwrap();
    assert!(Store::stop_requested(dir.path()).unwrap());
    assert_eq!(Store::read(dir.path()).unwrap().revision, revision);
    drop(store);
}

#[test]
fn evidence_hashes_and_accepted_receipts_are_required() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store(dir.path(), vec![task("one")]);
    store.run.tasks[0].status = Status::Accepted;
    assert!(validate(&store.run).is_err());
    store.run.tasks[0].status = Status::Validating;
    store.run.tasks[0].attempts.push(Attempt {
        number: 1,
        proposal: Some(json!({"candidate":"changed"})),
        proposal_sha256: Some("wrong".into()),
        validation: None,
        application: None,
    });
    assert!(validate(&store.run).is_err());
    store.run.version = 2;
    assert!(validate(&store.run).is_err());
}

#[tokio::test]
async fn repeated_failure_stops_at_attempt_limit_without_application() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.always_invalid = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    assert_eq!(result.tasks[0].status, Status::Rejected);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 3);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(Store::read(dir.path()).unwrap().tasks[0].attempts.len(), 3);
}

#[tokio::test]
async fn third_attempt_receives_all_prior_validation_feedback_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.always_invalid = true;
    mock.distinct_validation_feedback = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    assert_eq!(result.tasks[0].status, Status::Rejected);
    let feedback = mock.feedback.lock().unwrap();
    assert_eq!(feedback.len(), 3);
    let first = feedback[2].find("failure from attempt 1").unwrap();
    let second = feedback[2].find("failure from attempt 2").unwrap();
    assert!(first < second);
}

#[tokio::test]
async fn repeated_failing_proposal_stops_without_revalidation_or_another_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.always_invalid = true;
    mock.duplicate_proposal = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    let record = &result.tasks[0];
    assert_eq!(record.status, Status::Rejected);
    assert!(record.detail.contains("repeated a prior proposal"));
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    assert_eq!(mock.validations.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(record.attempts.len(), 2);
    assert_eq!(
        record.attempts[0].proposal_sha256,
        record.attempts[1].proposal_sha256
    );
    assert!(record.attempts[0].validation.is_some());
    assert!(record.attempts[1].validation.is_none());
}

#[tokio::test]
async fn matching_cli_candidate_with_volatile_metadata_stops_without_revalidation() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.always_invalid = true;
    mock.volatile_cli_metadata = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(store(dir.path(), vec![task("one")]), &mock, cancel, None)
        .await
        .unwrap();
    let record = &result.tasks[0];
    assert_eq!(record.status, Status::Rejected);
    assert!(record.detail.contains("repeated a prior proposal"));
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    assert_eq!(mock.validations.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(record.attempts.len(), 2);
    assert_ne!(
        record.attempts[0].proposal_sha256, record.attempts[1].proposal_sha256,
        "full envelopes must retain distinct integrity hashes"
    );
    assert!(record.attempts[0].validation.is_some());
    assert!(record.attempts[1].validation.is_none());
}

#[tokio::test]
async fn ambiguous_application_stops_the_project_before_the_next_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = Mock::new();
    mock.unknown_apply = true;
    let (_tx, cancel) = watch::channel(false);
    let result = drive(
        store(dir.path(), vec![task("one"), task("two")]),
        &mock,
        cancel,
        None,
    )
    .await
    .unwrap();
    assert!(result.stopped && result.finished);
    assert_eq!(result.tasks[0].status, Status::Unknown);
    assert_eq!(result.tasks[1].status, Status::Stopped);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 1);
}
