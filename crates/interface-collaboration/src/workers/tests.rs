use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Fake {
    calls: AtomicUsize,
    release: tokio::sync::Semaphore,
}
#[async_trait]
impl Runner for Fake {
    fn capture(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
    ) -> Result<BTreeMap<String, String>, String> {
        Ok(BTreeMap::new())
    }
    async fn run(
        &self,
        _: Job,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<ResultData, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::select! { permit = self.release.acquire() => { permit.unwrap().forget(); }, _ = cancelled.changed() => return Err("cancelled; remote outcome unknown".into()) }
        Ok(ResultData { report: Some(Report::parse(r#"{"schemaVersion":1,"outcome":"report","summary":"Three items total 12.","findings":[{"claim":"Total is 12","evidence":"4 + 3 + 5 = 12","uncertainty":"none"}],"missingContext":[]}"#).unwrap()), original_output:"original".into(),reported_model:None,provider_session:None,usage:json!({}) })
    }
}
fn request(id: &str) -> Request {
    Request {
        schema_version: 1,
        request_id: id.into(),
        kind: "analysis".into(),
        title: "Total".into(),
        brief: "Add 4, 3 and 5".into(),
        acceptance_criteria: vec![],
        read_files: vec![],
        model: MODEL.into(),
        wall_seconds: 180,
        max_steps: 16,
    }
}
fn setup(grant: bool) -> (tempfile::TempDir, Arc<Service>, Arc<Fake>) {
    let root = tempfile::tempdir().unwrap();
    let runner = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let service = Service::open(
        &root.path().join("workers"),
        Policy {
            source: root.path().into(),
            files: BTreeSet::new(),
            contributor_data_allowed: grant,
        },
        runner.clone(),
    )
    .unwrap();
    (root, service, runner)
}
fn id(value: &Value) -> String {
    value["taskId"].as_str().unwrap().into()
}
#[tokio::test]
async fn grok_is_admitted_beside_muse_and_other_models_are_refused() {
    let (_root, service, _runner) = setup(true);
    let models = service
        .call("claude", "worker_models", json!({}))
        .await
        .unwrap();
    let listed: Vec<_> = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [MODEL, GROK_MODEL]);
    let mut grok = request("grok");
    grok.model = GROK_MODEL.into();
    service.start("claude", grok).await.unwrap();
    let mut other = request("other");
    other.model = "opencode-go/grok-4.5".into();
    assert!(
        service
            .start("claude", other)
            .await
            .unwrap_err()
            .starts_with("invalidRequest")
    );
}
#[tokio::test]
async fn sharing_and_scope_refusals_are_unsent() {
    let (_root, service, runner) = setup(false);
    assert!(
        service
            .start("claude", request("one"))
            .await
            .unwrap_err()
            .starts_with("approvalRequired")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    let (_root, service, runner) = setup(true);
    let mut r = request("scope");
    r.read_files.push("not-approved.rs".into());
    assert!(
        service
            .start("codex", r)
            .await
            .unwrap_err()
            .starts_with("scopeDenied")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn both_owners_share_capacity_but_never_results_and_retry_is_idempotent() {
    let (_root, service, runner) = setup(true);
    let a = service.start("claude", request("one")).await.unwrap();
    let replay = service.start("claude", request("one")).await.unwrap();
    assert_eq!(id(&a), id(&replay));
    let b = service.start("codex", request("one")).await.unwrap();
    assert_ne!(id(&a), id(&b));
    let c = service.start("claude", request("two")).await.unwrap();
    assert_eq!(service.state.lock().await.active.len(), 2);
    assert_eq!(
        service.state.lock().await.index.records[&id(&c)].state,
        Status::Queued
    );
    assert!(
        service
            .collect("codex", &[id(&a)], 0, 0, true)
            .await
            .unwrap_err()
            .starts_with("scopeDenied")
    );
    assert!(service.cancel("codex", "cancel", &id(&a)).await.is_err());
    let mut changed = request("one");
    changed.brief = "different".into();
    assert!(
        service
            .start("claude", changed)
            .await
            .unwrap_err()
            .starts_with("idempotencyConflict")
    );
    runner.release.add_permits(3);
    let result = service
        .collect("claude", &[id(&a)], 0, 2000, true)
        .await
        .unwrap();
    let hash = result["tasks"][0]["resultSha256"].as_str().unwrap();
    assert!(!result["tasks"][0]["received"].as_bool().unwrap());
    assert!(
        service
            .review(
                "codex",
                "review",
                &id(&a),
                hash,
                "accepted",
                "checked arithmetic"
            )
            .await
            .is_err()
    );
    assert!(
        service
            .review("claude", "review", &id(&a), "wrong", "accepted", "checked")
            .await
            .is_err()
    );
    let reviewed = service
        .review(
            "claude",
            "review",
            &id(&a),
            hash,
            "accepted",
            "checked arithmetic",
        )
        .await
        .unwrap();
    assert_eq!(reviewed["received"], true);
    assert_eq!(
        service
            .review(
                "claude",
                "review",
                &id(&a),
                hash,
                "accepted",
                "checked arithmetic"
            )
            .await
            .unwrap(),
        reviewed
    );
    service.shutdown().await;
}
#[tokio::test]
async fn queued_cancellation_never_dispatches_and_mutation_conflicts_refuse() {
    let (_root, service, runner) = setup(true);
    service.start("claude", request("a")).await.unwrap();
    service.start("claude", request("b")).await.unwrap();
    let c = service.start("codex", request("c")).await.unwrap();
    let cancelled = service.cancel("codex", "cancel-c", &id(&c)).await.unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["effect"], "unsent");
    let receipt = cancelled["receiptSha256"].as_str().unwrap();
    assert!(
        service
            .review("codex", "accept-c", &id(&c), receipt, "accepted", "wrong")
            .await
            .is_err()
    );
    let acknowledgment = service
        .review(
            "codex",
            "ack-c",
            &id(&c),
            receipt,
            "received",
            "Cancellation seen",
        )
        .await
        .unwrap();
    assert_eq!(acknowledgment["received"], true);
    assert_eq!(
        service
            .review(
                "codex",
                "ack-c",
                &id(&c),
                receipt,
                "received",
                "Cancellation seen"
            )
            .await
            .unwrap(),
        acknowledgment
    );
    assert_eq!(
        service.cancel("codex", "cancel-c", &id(&c)).await.unwrap(),
        cancelled
    );
    runner.release.add_permits(2);
    service
        .collect("claude", &[], 0, 2000, false)
        .await
        .unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    service.shutdown().await;
}
#[tokio::test]
async fn restart_retains_results_and_marks_ambiguous_work_without_replay() {
    let (root, service, runner) = setup(true);
    let a = service.start("claude", request("a")).await.unwrap();
    runner.release.add_permits(1);
    service
        .collect("claude", &[id(&a)], 0, 2000, true)
        .await
        .unwrap();
    // Persist an in-flight intent without spawning to exercise crash recovery.
    {
        let mut state = service.state.lock().await;
        let mut record = state.index.records[&id(&a)].clone();
        record.task_id = "uncertain".into();
        record.result = None;
        record.result_sha256 = None;
        record.state = Status::Running;
        record.effect = "unknown".into();
        state.index.records.insert(record.task_id.clone(), record);
        service.persist(&mut state).unwrap();
    }
    service.shutdown().await;
    drop(service);
    let reopened = Service::open(
        &root.path().join("workers"),
        Policy {
            source: root.path().into(),
            files: BTreeSet::new(),
            contributor_data_allowed: true,
        },
        runner.clone(),
    )
    .unwrap();
    let result = reopened
        .collect("claude", &["uncertain".into()], 0, 0, true)
        .await
        .unwrap();
    assert_eq!(result["tasks"][0]["state"], "interrupted");
    assert_eq!(result["tasks"][0]["effect"], "unknown");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        reopened
            .collect("claude", &[id(&a)], 0, 0, true)
            .await
            .unwrap()["tasks"][0]["state"],
        "completed"
    );
}
#[tokio::test]
async fn followup_keeps_prior_result_and_checks_owner_and_hash() {
    let (_root, service, runner) = setup(true);
    let a = service.start("codex", request("a")).await.unwrap();
    assert!(
        service
            .followup("codex", &id(&a), "wrong", request("b"))
            .await
            .is_err()
    );
    runner.release.add_permits(1);
    let result = service
        .collect("codex", &[id(&a)], 0, 2000, true)
        .await
        .unwrap();
    let hash = result["tasks"][0]["resultSha256"].as_str().unwrap();
    let b = service
        .followup("codex", &id(&a), hash, request("b"))
        .await
        .unwrap();
    assert_eq!(b["parentTaskId"], id(&a));
    assert_eq!(
        service
            .followup("codex", &id(&a), hash, request("b"))
            .await
            .unwrap()["taskId"],
        b["taskId"]
    );
    assert_eq!(
        service
            .collect("codex", &[id(&a)], 0, 0, true)
            .await
            .unwrap()["tasks"][0]["resultSha256"],
        hash
    );
    service.shutdown().await;
}
#[test]
fn tools_and_report_contract_are_bounded() {
    assert_eq!(catalogue().len(), 6);
    assert!(!catalogue().iter().any(|t| t["name"] == "worker_apply"));
    assert!(Report::parse("Here is the result: {}").is_err());
    let mut r = request("a");
    r.max_steps = 100;
    assert!(r.validate().is_err());
}

#[tokio::test]
async fn malformed_report_keeps_original_evidence_and_cannot_be_accepted() {
    struct Invalid;
    #[async_trait]
    impl Runner for Invalid {
        fn capture(
            &self,
            _: &Path,
            _: &Path,
            _: &[String],
        ) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::new())
        }
        async fn run(&self, _: Job, _: watch::Receiver<bool>) -> Result<ResultData, String> {
            Ok(ResultData {
                report: None,
                original_output: "not JSON, do not repair me".into(),
                reported_model: None,
                provider_session: None,
                usage: json!({}),
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(
        &root.path().join("workers"),
        Policy {
            source: root.path().into(),
            files: BTreeSet::new(),
            contributor_data_allowed: true,
        },
        Arc::new(Invalid),
    )
    .unwrap();
    let task = service.start("codex", request("invalid")).await.unwrap();
    let result = service
        .collect("codex", &[id(&task)], 0, 2000, true)
        .await
        .unwrap();
    let record = &result["tasks"][0];
    assert_eq!(record["state"], "failed");
    assert_eq!(
        record["result"]["originalOutput"],
        "not JSON, do not repair me"
    );
    assert!(
        service
            .review(
                "codex",
                "reject-invalid",
                &id(&task),
                record["resultSha256"].as_str().unwrap(),
                "accepted",
                "wrong"
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_result_commit_never_exposes_completion_or_allows_review() {
    let (root, service, runner) = setup(true);
    let task = service
        .start("claude", request("disk-failure"))
        .await
        .unwrap();
    // Change the expected target identity without changing the valid store contents.
    let index = root.path().join("workers/workers.v1.json");
    let mut bytes = std::fs::read(&index).unwrap();
    bytes.push(b'\n');
    std::fs::write(&index, bytes).unwrap();
    runner.release.add_permits(1);
    let result = service.collect("claude", &[id(&task)], 0, 2000, true).await;
    assert!(result.unwrap_err().starts_with("storageUncertain"));
    assert_eq!(
        service.state.lock().await.index.records[&id(&task)].state,
        Status::Running
    );
    assert!(
        service
            .review("claude", "accept", &id(&task), "anything", "accepted", "")
            .await
            .unwrap_err()
            .starts_with("storageUncertain")
    );
    assert!(
        service
            .cancel("claude", "cancel", &id(&task))
            .await
            .unwrap_err()
            .starts_with("storageUncertain")
    );
    assert!(
        service
            .start("claude", request("later"))
            .await
            .unwrap_err()
            .starts_with("storageUncertain")
    );
    assert!(
        service.notice("claude").await["message"]
            .as_str()
            .unwrap()
            .contains("storageUncertain")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn archived_history_exceeds_old_capacity_and_keeps_exact_retries_and_owner_scope() {
    let (_root, service, runner) = setup(true);
    let mut first = None;
    for n in 0..130 {
        let request_id = format!("report-{n}");
        let task = service.start("claude", request(&request_id)).await.unwrap();
        runner.release.add_permits(1);
        let result = service
            .collect("claude", &[id(&task)], 0, 2000, true)
            .await
            .unwrap();
        let hash = result["tasks"][0]["resultSha256"].as_str().unwrap();
        let review_id = format!("review-{n}");
        service
            .review(
                "claude",
                &review_id,
                &id(&task),
                hash,
                "accepted",
                "checked",
            )
            .await
            .unwrap();
        if n == 0 {
            first = Some((id(&task), hash.to_owned()));
        }
    }
    let (first, hash) = first.unwrap();
    assert!(
        !service
            .state
            .lock()
            .await
            .index
            .records
            .contains_key(&first)
    );
    let archived = service
        .collect("claude", std::slice::from_ref(&first), 0, 0, true)
        .await
        .unwrap();
    assert_eq!(archived["tasks"][0]["resultSha256"], hash);
    assert!(
        service
            .collect("codex", std::slice::from_ref(&first), 0, 0, true)
            .await
            .is_err()
    );
    assert_eq!(
        id(&service.start("claude", request("report-0")).await.unwrap()),
        first
    );
    service
        .review("claude", "review-0", &first, &hash, "accepted", "checked")
        .await
        .unwrap();
    let mut changed = request("report-0");
    changed.brief = "different".into();
    assert!(
        service
            .start("claude", changed)
            .await
            .unwrap_err()
            .starts_with("idempotencyConflict")
    );
    assert!(
        service
            .start("claude", request("review-0"))
            .await
            .unwrap_err()
            .starts_with("idempotencyConflict")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 130);
}

#[tokio::test]
async fn a_full_index_of_unreceived_results_refuses_without_rewriting_it() {
    use std::os::unix::fs::MetadataExt;

    let (root, service, runner) = setup(true);
    let task = service.start("claude", request("seed")).await.unwrap();
    runner.release.add_permits(1);
    service
        .collect("claude", &[id(&task)], 0, 2000, false)
        .await
        .unwrap();
    {
        let mut state = service.state.lock().await;
        let seed = state.index.records[&id(&task)].clone();
        assert!(seed.state.terminal() && !seed.received);
        for n in 1..MAX_RECORDS {
            let mut record = seed.clone();
            record.task_id = Uuid::new_v4().to_string();
            record.request.request_id = format!("filler-{n}");
            state.index.sequence += 1;
            record.sequence = state.index.sequence;
            state.index.records.insert(record.task_id.clone(), record);
        }
        service.persist(&mut state).unwrap();
    }
    let path = root.path().join("workers").join(INDEX);
    let snapshot = || {
        let metadata = std::fs::metadata(&path).unwrap();
        (
            metadata.ino(),
            metadata.modified().unwrap(),
            std::fs::read(&path).unwrap(),
        )
    };
    let before = snapshot();
    for attempt in 0..2 {
        assert!(
            service
                .start("claude", request(&format!("refused-{attempt}")))
                .await
                .unwrap_err()
                .starts_with("capacityExceeded")
        );
    }
    assert_eq!(snapshot(), before);
    assert_eq!(service.state.lock().await.index.records.len(), MAX_RECORDS);
    // Still healthy: no needless write could have closed admission.
    assert!(
        service.notice("claude").await["message"]
            .as_str()
            .unwrap()
            .contains("results are ready")
    );
}

#[tokio::test]
async fn receipt_pressure_does_not_block_cancellation_and_archived_receipts_still_deduplicate() {
    let (_root, service, _runner) = setup(true);
    let task = service.start("claude", request("running")).await.unwrap();
    let intent = json!({"cancel":id(&task)});
    {
        let mut state = service.state.lock().await;
        for n in 0..2048 {
            state.index.mutations.insert(
                format!("claude/old-{n:04}"),
                (digest(&intent), json!({"prior":n})),
            );
        }
        service.persist(&mut state).unwrap();
    }
    service
        .cancel("claude", "new-cancel", &id(&task))
        .await
        .unwrap();
    assert!(service.state.lock().await.index.mutations.len() < 2048);
    assert_eq!(
        service
            .cancel("claude", "old-0000", &id(&task))
            .await
            .unwrap(),
        json!({"prior":0})
    );
    assert!(
        service
            .review("claude", "old-0000", &id(&task), "x", "received", "")
            .await
            .unwrap_err()
            .starts_with("idempotencyConflict")
    );
}

#[test]
fn native_grants_are_exact_and_revocable_without_a_provider_call() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let broker = root.path().join("broker");
    assert!(
        !grants::read(&broker, &source)
            .unwrap()
            .contributor_data_allowed
    );
    grants::write(
        &broker,
        Policy {
            source: source.clone(),
            files: BTreeSet::from(["a.rs".into()]),
            contributor_data_allowed: true,
        },
    )
    .unwrap();
    assert_eq!(
        grants::read(&broker, &source).unwrap().files,
        BTreeSet::from(["a.rs".into()])
    );
    grants::write(
        &broker,
        Policy {
            source: source.clone(),
            files: BTreeSet::new(),
            contributor_data_allowed: false,
        },
    )
    .unwrap();
    assert!(
        !grants::read(&broker, &source)
            .unwrap()
            .contributor_data_allowed
    );
    assert!(
        grants::write(
            &broker,
            Policy {
                source,
                files: BTreeSet::from(["../outside".into()]),
                contributor_data_allowed: true
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn written_bytes_match_the_pinned_format() {
    let (root, service, runner) = setup(true);
    let index = root.path().join("workers/workers.v1.json");
    crate::golden::assert_golden("workers.opened.json", root.path(), &index, &[]);
    let task = service.start("claude", request("pinned")).await.unwrap();
    let task_id = id(&task);
    // The fake runner holds the dispatched task, so the index says running.
    let running = std::fs::read(&index).unwrap();
    let record: Value = serde_json::from_slice(&running).unwrap();
    let created = record["records"][&task_id]["createdAt"]
        .as_str()
        .unwrap()
        .to_owned();
    let masks = [
        (task_id.as_str(), "<TASK>"),
        (created.as_str(), "<CREATED>"),
    ];
    crate::golden::assert_golden("workers.running.json", root.path(), &index, &masks);
    service.shutdown().await;
    let service = tokio::time::timeout(Duration::from_secs(5), async move {
        let mut service = service;
        loop {
            match Arc::try_unwrap(service) {
                Ok(service) => return service,
                Err(shared) => service = shared,
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dispatched task settled");
    drop(service);
    // Restore the running index an interrupted broker left behind; reopening
    // interrupts the record without replay and rewrites the index once.
    std::fs::write(&index, &running).unwrap();
    let policy = Policy {
        source: root.path().into(),
        files: BTreeSet::new(),
        contributor_data_allowed: true,
    };
    let reopened = Service::open(&root.path().join("workers"), policy, runner.clone()).unwrap();
    crate::golden::assert_golden("workers.recovered.json", root.path(), &index, &masks);
    assert!(root.path().join("workers/workers.lock").exists());
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    reopened.shutdown().await;
}

#[tokio::test]
async fn v1_store_upgrade_fences_older_brokers_before_archive_admission() {
    let (root, service, runner) = setup(false);
    let index = root.path().join("workers/workers.v1.json");
    let mut prior: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    prior["schemaVersion"] = json!(1);
    service.shutdown().await;
    drop(service);
    std::fs::write(&index, serde_json::to_vec(&prior).unwrap()).unwrap();
    let policy = Policy {
        source: root.path().into(),
        files: BTreeSet::new(),
        contributor_data_allowed: false,
    };
    let reopened =
        Service::open(&root.path().join("workers"), policy.clone(), runner.clone()).unwrap();
    let upgraded: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    assert_eq!(
        upgraded["schemaVersion"], 2,
        "the original filename must fence the v1 reader"
    );
    assert_eq!(upgraded["records"], prior["records"]);
    assert_eq!(upgraded["mutations"], prior["mutations"]);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    reopened.shutdown().await;
    drop(reopened);
    let mut future = upgraded;
    future["schemaVersion"] = json!(3);
    std::fs::write(&index, serde_json::to_vec(&future).unwrap()).unwrap();
    assert!(Service::open(&root.path().join("workers"), policy, runner).is_err());
}

struct HeldCapture {
    fake: Fake,
    hold: std::sync::atomic::AtomicBool,
    revoked: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Semaphore,
    gate: (std::sync::Mutex<bool>, std::sync::Condvar),
}
impl HeldCapture {
    fn release_capture(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
    async fn wait_for_capture(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
#[async_trait]
impl Runner for HeldCapture {
    fn validate_policy(&self, _: &Path, _: &Policy) -> Result<(), String> {
        if self.revoked.load(Ordering::SeqCst) {
            Err("scopeDenied: revoked".into())
        } else {
            Ok(())
        }
    }
    fn capture(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
    ) -> Result<BTreeMap<String, String>, String> {
        if self.hold.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            let (_guard, timeout) = self
                .gate
                .1
                .wait_timeout_while(
                    self.gate.0.lock().unwrap(),
                    Duration::from_secs(5),
                    |open| !*open,
                )
                .unwrap();
            if timeout.timed_out() {
                return Err("fixture capture timeout".into());
            }
        }
        Ok(BTreeMap::new())
    }
    async fn run(&self, job: Job, cancelled: watch::Receiver<bool>) -> Result<ResultData, String> {
        self.fake.run(job, cancelled).await
    }
}
fn held_capture_setup() -> (tempfile::TempDir, Arc<Service>, Arc<HeldCapture>) {
    let root = tempfile::tempdir().unwrap();
    let runner = Arc::new(HeldCapture {
        fake: Fake {
            calls: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
        },
        hold: std::sync::atomic::AtomicBool::new(false),
        revoked: std::sync::atomic::AtomicBool::new(false),
        entered: tokio::sync::Semaphore::new(0),
        gate: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
    });
    let service = Service::open(
        &root.path().join("workers"),
        Policy {
            source: root.path().into(),
            files: BTreeSet::new(),
            contributor_data_allowed: true,
        },
        runner.clone(),
    )
    .unwrap();
    (root, service, runner)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_capture_does_not_block_another_owners_cancellation() {
    let (_root, service, runner) = held_capture_setup();
    let active = service.start("claude", request("active")).await.unwrap();
    runner.hold.store(true, Ordering::SeqCst);
    let other = service.clone();
    let capture = tokio::spawn(async move { other.start("codex", request("capture")).await });
    runner.wait_for_capture().await;
    // The capture is held until after this bound, so a cancel that waited for
    // it could never finish: the bound only has to outlast a durable cancel
    // on a loaded machine, not race the capture.
    let cancellation = tokio::time::timeout(
        Duration::from_secs(10),
        service.cancel("claude", "cancel-active", &id(&active)),
    )
    .await;
    runner.release_capture();
    assert!(
        cancellation
            .expect("cancel waited for unrelated capture")
            .is_ok()
    );
    capture.await.unwrap().unwrap();
    service.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_revalidates_revocation_and_shutdown_before_admission() {
    for revoke in [false, true] {
        let (_root, service, runner) = held_capture_setup();
        runner.hold.store(true, Ordering::SeqCst);
        let other = service.clone();
        let capture = tokio::spawn(async move { other.start("codex", request("capture")).await });
        runner.wait_for_capture().await;
        if revoke {
            runner.revoked.store(true, Ordering::SeqCst);
        } else {
            service.shutdown().await;
        }
        runner.release_capture();
        assert!(capture.await.unwrap().is_err());
        assert!(service.state.lock().await.index.records.is_empty());
        assert_eq!(runner.fake.calls.load(Ordering::SeqCst), 0);
        service.shutdown().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_capture_is_bounded_and_duplicate_admission_dispatches_once() {
    let (_root, service, runner) = held_capture_setup();
    runner.hold.store(true, Ordering::SeqCst);
    let a = service.clone();
    let a = tokio::spawn(async move { a.start("codex", request("same")).await });
    runner.wait_for_capture().await;
    let b = service.clone();
    let b = tokio::spawn(async move { b.start("codex", request("same")).await });
    runner.wait_for_capture().await;
    let refused = service.start("claude", request("third")).await.unwrap_err();
    runner.release_capture();
    assert!(refused.starts_with("capacityExceeded"));
    assert_eq!(
        id(&a.await.unwrap().unwrap()),
        id(&b.await.unwrap().unwrap())
    );
    assert_eq!(service.state.lock().await.index.records.len(), 1);
    assert_eq!(runner.fake.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
}

#[tokio::test]
async fn unreceived_owners_cover_every_nonempty_notice_and_only_those_owners() {
    let owners = |names: &[&str]| Some(names.iter().map(|&n| n.to_owned()).collect());
    let (_root, service, runner) = setup(true);
    assert_eq!(service.unreceived_owners().await, owners(&[]));
    assert_eq!(service.notice("claude").await, empty_notice());
    let task = service.start("claude", request("pending")).await.unwrap();
    assert_eq!(
        service.unreceived_owners().await,
        owners(&[]),
        "a running task has nothing to receive"
    );
    runner.release.add_permits(1);
    let result = service
        .collect("claude", &[id(&task)], 0, 2000, true)
        .await
        .unwrap();
    assert_eq!(service.unreceived_owners().await, owners(&["claude"]));
    assert_eq!(service.notice("codex").await, empty_notice());
    assert_ne!(service.notice("claude").await, empty_notice());
    // A cancelled, unreviewed result is announced to its own owner only.
    let cancelled = service.start("codex", request("cancelled")).await.unwrap();
    service
        .cancel("codex", "stop", &id(&cancelled))
        .await
        .unwrap();
    let stopped = service
        .collect("codex", &[id(&cancelled)], 0, 2000, false)
        .await
        .unwrap();
    assert_eq!(stopped["tasks"][0]["state"], "cancelled", "{stopped}");
    assert_eq!(
        service.unreceived_owners().await,
        owners(&["claude", "codex"])
    );
    assert_ne!(service.notice("codex").await, empty_notice());
    service
        .review(
            "claude",
            "seen",
            &id(&task),
            result["tasks"][0]["resultSha256"].as_str().unwrap(),
            "received",
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        service.unreceived_owners().await,
        owners(&["codex"]),
        "a received result is not announced again"
    );
    assert_eq!(service.notice("claude").await, empty_notice());
    service.state.lock().await.storage_error = Some("storageUncertain: test".into());
    assert_eq!(service.unreceived_owners().await, None);
}

#[tokio::test]
async fn an_unchanged_store_is_not_rewritten_on_open() {
    use std::os::unix::fs::MetadataExt;

    let (root, service, runner) = setup(true);
    let task = service.start("claude", request("done")).await.unwrap();
    runner.release.add_permits(1);
    service
        .collect("claude", &[id(&task)], 0, 2000, true)
        .await
        .unwrap();
    service.shutdown().await;
    drop(service);
    let index = root.path().join("workers/workers.v1.json");
    let written = std::fs::metadata(&index).unwrap().ino();
    let policy = Policy {
        source: root.path().into(),
        files: BTreeSet::new(),
        contributor_data_allowed: true,
    };
    let reopened =
        Service::open(&root.path().join("workers"), policy.clone(), runner.clone()).unwrap();
    assert_eq!(
        std::fs::metadata(&index).unwrap().ino(),
        written,
        "settled records need no rewrite"
    );
    // A record left running is interrupted, so that open does write.
    {
        let mut state = reopened.state.lock().await;
        let mut record = state.index.records[&id(&task)].clone();
        record.task_id = "uncertain".into();
        record.result = None;
        record.result_sha256 = None;
        record.state = Status::Running;
        state.index.records.insert(record.task_id.clone(), record);
        reopened.persist(&mut state).unwrap();
    }
    reopened.shutdown().await;
    drop(reopened);
    let written = std::fs::metadata(&index).unwrap().ino();
    let interrupted = Service::open(&root.path().join("workers"), policy, runner).unwrap();
    assert_ne!(std::fs::metadata(&index).unwrap().ino(), written);
    let stored: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    assert_eq!(stored["records"]["uncertain"]["state"], "interrupted");
    interrupted.shutdown().await;
}
