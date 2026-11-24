use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

fn private_temp() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn config() -> Config {
    Config {
        version: 1,
        project: PathBuf::from("/project"),
        objective: "Repair the utility".into(),
        files: vec!["value.py".into()],
        context_files: vec!["test.py".into()],
        validation: ValidationPlan {
            version: 1,
            files: vec!["test.py".into()],
            protected_files: vec!["test.py".into()],
            checks: vec![vec!["python3".into(), "test.py".into()]],
        },
        worker_model: "codex/model".into(),
        review_model: "claude/model".into(),
        duration_seconds: 60,
        max_iterations: 2,
    }
}
struct Mock {
    calls: AtomicUsize,
    reviews: AtomicUsize,
    applied: AtomicUsize,
    wait_phase: Option<Phase>,
    entered: Arc<Notify>,
    root: PathBuf,
    hold: bool,
    reject_first: bool,
    stale_review: bool,
    feedback_in_review: bool,
    omit_ack: bool,
    fail_first_check: bool,
    fail_second_check: bool,
    repeat_candidate: bool,
    finding_target: FindingTarget,
}
impl Mock {
    fn new(root: &std::path::Path) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            reviews: AtomicUsize::new(0),
            applied: AtomicUsize::new(0),
            wait_phase: None,
            entered: Arc::new(Notify::new()),
            root: root.into(),
            hold: false,
            reject_first: false,
            stale_review: false,
            feedback_in_review: false,
            omit_ack: false,
            fail_first_check: false,
            fail_second_check: false,
            repeat_candidate: false,
            finding_target: FindingTarget::Candidate,
        }
    }
    async fn wait(&self, phase: Phase) {
        if self.wait_phase == Some(phase) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
    }
}
#[async_trait]
impl Executor for Mock {
    async fn propose(
        &self,
        number: u8,
        feedback: &Value,
        _: watch::Receiver<bool>,
    ) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.wait(Phase::Proposing).await;
        if number > 1 && self.reject_first {
            assert_eq!(feedback["findings"][0]["id"], "edge-case");
        }
        if number == 2 && self.fail_first_check {
            assert_eq!(feedback["latestValidation"]["passed"], false);
        }
        let ack = json!({"feedbackRevision":if self.omit_ack {999} else {feedback["revision"].as_u64().unwrap()},
            "addressedFeedbackIds":feedback["messages"].as_array().unwrap().iter().map(|f|f["id"].clone()).collect::<Vec<_>>(),
            "addressedReviewIds":feedback["findings"].as_array().unwrap().iter().map(|f|f["id"].clone()).collect::<Vec<_>>(),"summary":"Repaired demonstrated edge case"});
        let candidate_number = if self.repeat_candidate { 1 } else { number };
        Ok(
            json!({"summary":json!({"outcome":"changes","summary":ack.to_string()}).to_string(),
            "elapsedMs":number,"changes":[{"path":"value.py","after":format!("candidate {candidate_number}")}] }),
        )
    }
    async fn validate(&self, number: u8, _: &Value) -> Result<Value, String> {
        self.wait(Phase::Validating).await;
        Ok(
            json!({"passed":!((number==1 && self.fail_first_check)||(number==2 && self.fail_second_check)),"inputs":[{"path":"value.py","sha256":"source"}],"checks":[{"stdout":"real counterexample"}]}),
        )
    }
    async fn review(
        &self,
        number: u8,
        _: &Value,
        _: &Value,
        candidate: &str,
        context: &Value,
        _: watch::Receiver<bool>,
    ) -> Result<Review, String> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        self.wait(Phase::Reviewing).await;
        if self.feedback_in_review && number == 1 {
            Store::feedback(&self.root, "late", "Test Unicode inputs")?;
        }
        let reject = self.reject_first && number == 1;
        Ok(Review {
            version: 1,
            candidate_sha256: if self.stale_review {
                "different".into()
            } else {
                candidate.into()
            },
            feedback_revision: context["revision"].as_u64().unwrap(),
            approved: !reject,
            summary: "Checked concrete edge cases".into(),
            findings: if reject {
                vec![Finding {
                    id: "edge-case".into(),
                    target: self.finding_target,
                    evidence: "Empty input returns null".into(),
                    requested_change: "Return an empty list".into(),
                }]
            } else {
                vec![]
            },
        })
    }
    async fn apply(&self, _: &Value, _: &Value) -> Result<Value, String> {
        self.applied.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"state":if self.hold {"held"} else {"applied"}}))
    }
}

#[test]
fn written_bytes_match_the_pinned_format() {
    let dir = private_temp();
    let file = dir.path().join("cycle-run.v1.json");
    let control = dir.path().join("cycle-control.v1.json");
    let mut store = Store::create(dir.path(), config()).unwrap();
    let started = store.run.started_at_ms.to_string();
    let deadline = store.run.deadline_at_ms.to_string();
    let masks = [
        (started.as_str(), "<STARTED>"),
        (deadline.as_str(), "<DEADLINE>"),
    ];
    crate::golden::assert_golden("cycle-run.created.json", dir.path(), &file, &masks);
    crate::golden::assert_golden("cycle-control.created.json", dir.path(), &control, &[]);
    store.phase(Phase::Proposing).unwrap();
    crate::golden::assert_golden("cycle-run.proposing.json", dir.path(), &file, &masks);
    drop(store);
    // Recovery marks the interrupted controller unknown and rewrites once.
    Store::recover(dir.path()).unwrap();
    crate::golden::assert_golden("cycle-run.recovered.json", dir.path(), &file, &masks);
    Store::stop(dir.path()).unwrap();
    crate::golden::assert_golden("cycle-control.stopped.json", dir.path(), &control, &[]);
    assert!(dir.path().join("cycle.lock").exists());
}

#[tokio::test]
async fn fresh_review_findings_feed_the_next_attempt_and_gate_application() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.reject_first = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert_eq!(run.status, Status::Completed);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 1);
    assert!(!run.iterations[0].review.as_ref().unwrap().approved);
    assert_eq!(Store::read(dir.path()).unwrap().iterations.len(), 2);
}

#[tokio::test]
async fn new_feedback_during_review_invalidates_acceptance_and_is_acknowledged() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.feedback_in_review = true;
    mock.repeat_candidate = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert!(run.iterations[0].application.is_none());
    assert_eq!(run.iterations[1].feedback_revision, 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_review_and_missing_acknowledgment_never_apply() {
    for stale in [false, true] {
        let dir = private_temp();
        let store = Store::create(dir.path(), config()).unwrap();
        let mut mock = Mock::new(dir.path());
        mock.stale_review = stale;
        mock.omit_ack = !stale;
        let (_tx, rx) = watch::channel(false);
        let run = drive(store, &mock, rx, None).await.unwrap();
        assert!(matches!(run.status, Status::Unknown | Status::Blocked));
        assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn failed_check_is_retained_and_skips_review_until_repaired() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.fail_first_check = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert_eq!(mock.reviews.load(Ordering::SeqCst), 1);
    assert!(run.iterations[0].review.is_none());
}

#[tokio::test]
async fn durable_stop_interrupts_non_cooperative_provider_check_and_review() {
    for phase in [Phase::Proposing, Phase::Validating, Phase::Reviewing] {
        let dir = private_temp();
        let store = Store::create(dir.path(), config()).unwrap();
        let mut mock = Mock::new(dir.path());
        mock.wait_phase = Some(phase);
        let (_tx, rx) = watch::channel(false);
        let work = drive(store, &mock, rx, None);
        tokio::pin!(work);
        tokio::select! {()=mock.entered.notified()=>{Store::stop(dir.path()).unwrap();},result=&mut work=>panic!("unexpected completion {result:?}")}
        // A liveness bound, not a latency claim: the stop is seen within one
        // 100 ms poll, but its durable writes can take seconds on a loaded disk.
        let run = tokio::time::timeout(Duration::from_secs(30), work)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, Status::Stopped);
        assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    }
}

// Paused time: the 60 s deadline passes in virtual time once the worker
// hangs, so a slow disk can neither expire the run before the worker is
// dispatched nor outlast the bound below.
#[tokio::test(start_paused = true)]
async fn deadline_stops_a_hung_worker_and_late_feedback_cannot_reopen_run() {
    let dir = private_temp();
    let cfg = config();
    let store = Store::create(dir.path(), cfg.clone()).unwrap();
    let original_deadline = store.run.deadline_at_ms;
    let mut mock = Mock::new(dir.path());
    mock.wait_phase = Some(Phase::Proposing);
    let (_tx, rx) = watch::channel(false);
    let run = tokio::time::timeout(Duration::from_secs(120), drive(store, &mock, rx, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, Status::Expired);
    Store::feedback(dir.path(), "delayed-review", "Keep improving").unwrap();
    let after = Store::read(dir.path()).unwrap();
    assert_eq!(after.deadline_at_ms, original_deadline);
    assert_eq!(after.status, Status::Expired);
    assert!(Store::create(dir.path(), cfg).is_err());
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn feedback_ids_are_idempotent_and_recovery_never_resumes() {
    let dir = private_temp();
    let mut store = Store::create(dir.path(), config()).unwrap();
    let a = Store::feedback(dir.path(), "review-1", "Fix the bug").unwrap();
    let b = Store::feedback(dir.path(), "review-1", "Fix the bug").unwrap();
    assert_eq!(a.revision, b.revision);
    assert!(Store::feedback(dir.path(), "review-1", "Different").is_err());
    store.phase(Phase::Proposing).unwrap();
    assert!(Store::recover(dir.path()).is_err());
    drop(store);
    assert_eq!(Store::recover(dir.path()).unwrap().status, Status::Unknown);
}

#[test]
fn newest_failure_remains_available_with_large_old_history() {
    let dir = private_temp();
    let mut store = Store::create(dir.path(), config()).unwrap();
    store.run.iterations.push(Iteration {
        number: 1,
        feedback_revision: 0,
        proposal: None,
        proposal_sha256: None,
        candidate_sha256: None,
        validation: Some(json!({"passed":false,"error":"LATEST"})),
        review: None,
        application: None,
        outcome: "x".repeat(4000),
    });
    let context = feedback(&store.run, &Store::control(dir.path()).unwrap());
    assert_eq!(context["latestValidation"]["error"], "LATEST");
}

#[tokio::test]
async fn unresolved_review_survives_an_intervening_failed_check() {
    let dir = private_temp();
    let mut cfg = config();
    cfg.max_iterations = 3;
    let store = Store::create(dir.path(), cfg).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.reject_first = true;
    mock.fail_second_check = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert!(run.iterations[1].review.is_none());
    assert_eq!(mock.applied.load(Ordering::SeqCst), 1);
    assert_eq!(mock.reviews.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn volatile_provider_metadata_does_not_hide_a_repeated_failed_candidate() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.reject_first = true;
    mock.repeat_candidate = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert_eq!(run.status, Status::Blocked);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
    assert_eq!(mock.reviews.load(Ordering::SeqCst), 1);
    assert_ne!(
        run.iterations[0].proposal_sha256,
        run.iterations[1].proposal_sha256
    );
}

#[tokio::test]
async fn validation_findings_park_instead_of_rewriting_unrelated_candidate_code() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.reject_first = true;
    mock.finding_target = FindingTarget::Validation;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert_eq!(run.status, Status::Blocked);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.applied.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn checked_pair_candidate_is_held_and_does_not_start_another_round() {
    let dir = private_temp();
    let store = Store::create(dir.path(), config()).unwrap();
    let mut mock = Mock::new(dir.path());
    mock.hold = true;
    let (_tx, rx) = watch::channel(false);
    let run = drive(store, &mock, rx, None).await.unwrap();
    assert_eq!(run.status, Status::Completed);
    assert_eq!(run.iterations.len(), 1);
    assert_eq!(
        run.iterations[0].application.as_ref().unwrap()["state"],
        "held"
    );
    assert!(run.detail.contains("no source files applied"));
    assert_eq!(mock.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(Store::read(dir.path()).unwrap().iterations.len(), 1);
}

#[test]
fn owner_stop_waits_out_a_brief_control_checkpoint() {
    let dir = private_temp();
    let _store = Store::create(dir.path(), config()).unwrap();
    // The controller holds control.lock across its apply checkpoint.
    let checkpoint = Store::control_lock(dir.path()).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(checkpoint);
    });
    let control = Store::stop(dir.path()).expect("Stop recorded once the lock was free");
    assert!(control.stop);
    release.join().unwrap();
}
