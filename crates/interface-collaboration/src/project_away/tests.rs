use super::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
};
#[derive(Default)]
struct Fake {
    answers: Mutex<VecDeque<Result<String, Failure>>>,
    prompts: Mutex<Vec<String>>,
    creates: AtomicUsize,
    closes: AtomicUsize,
    /// Which sessions closed, in order.
    closed: Mutex<Vec<String>>,
    waiting: bool,
    /// Every session's folder, in creation order.
    cwds: Mutex<Vec<String>>,
    /// Each prepared worktree's token, then what became of it.
    prepared: AtomicUsize,
    discarded: Mutex<Vec<String>>,
    bound: Mutex<Vec<(String, String)>>,
    released: Mutex<Vec<String>>,
    /// The prompts each session was sent, by key.
    sent: Mutex<Vec<(String, String)>>,
    /// A refusal for the next worktree.
    refuse_prepare: Option<&'static str>,
}
impl Fake {
    fn new(outcomes: &[&str]) -> Arc<Self> {
        Arc::new(Self { answers: Mutex::new(outcomes.iter().map(|outcome| Ok(format!("AWAY_RESULT {{\"outcome\":\"{outcome}\",\"summary\":\"Evidence for {outcome}\",\"checks\":[\"fixture check passed\"]}}"))).collect()), ..Self::default() })
    }
    /// The thread each bind filed, by key.
    fn thread(&self) -> Option<String> {
        lock(&self.bound).first().map(|(key, _)| key.clone())
    }
}
#[async_trait]
impl Executor for Fake {
    async fn prepare(&self, _: &Run) -> Result<Workspace, Failure> {
        if let Some(refusal) = self.refuse_prepare {
            return Err(Failure::blocked(refusal));
        }
        let n = self.prepared.fetch_add(1, Ordering::SeqCst);
        Ok(Workspace {
            token: format!("prep-{n}"),
            cwd: "/worktrees/project-away".into(),
            branch: Some("iface/away-improve-1a2b3c".into()),
        })
    }
    async fn discard(&self, token: &str) {
        lock(&self.discarded).push(token.to_owned());
    }
    async fn create(&self, _: &Run, _: Role, _: u8, cwd: &str) -> Result<String, Failure> {
        lock(&self.cwds).push(cwd.to_owned());
        Ok(format!(
            "codex:{}",
            self.creates.fetch_add(1, Ordering::SeqCst)
        ))
    }
    async fn bind(&self, _: &Run, key: &str, token: &str) -> Result<(), Failure> {
        lock(&self.bound).push((key.to_owned(), token.to_owned()));
        Ok(())
    }
    async fn turn(
        &self,
        key: &str,
        prompt: String,
        _: watch::Receiver<bool>,
    ) -> Result<Answer, Failure> {
        lock(&self.prompts).push(prompt.clone());
        let ordinal = {
            let mut sent = lock(&self.sent);
            sent.push((key.to_owned(), prompt));
            sent.iter().filter(|(sent, _)| sent == key).count() - 1
        };
        if self.waiting {
            std::future::pending::<()>().await;
        }
        let thread_turn =
            (self.thread().as_deref() == Some(key)).then(|| u32::try_from(ordinal).unwrap());
        lock(&self.answers)
            .pop_front()
            .unwrap_or_else(|| Err(Failure::blocked("No fixture answer")))
            .map(|text| Answer { text, thread_turn })
    }
    async fn close(&self, key: &str) -> Result<(), Failure> {
        lock(&self.closed).push(key.to_owned());
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn release(&self, key: &str) {
        lock(&self.released).push(key.to_owned());
    }
}
/// A liveness bound, not a timing assertion: a run of several durably
/// written turns can take seconds on a loaded disk.
const SETTLE_BOUND: Duration = Duration::from_secs(30);

async fn settled(service: &Service) -> Run {
    tokio::time::timeout(SETTLE_BOUND, async {
        loop {
            let run = service.snapshot().await.unwrap().runs[0].clone();
            if !run.state.active() {
                return run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("run settled")
}
#[tokio::test]
async fn immediate_start_fresh_review_refinement_and_no_replay() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    // The worker's complete claim cannot override a review that finds more work.
    let fake = Fake::new(&[
        "continue", "revise", "complete", "continue", "continue", "complete",
    ]);
    service
        .start("run-1".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    let run = settled(&service).await;
    assert_eq!(run.state, State::Completed);
    assert_eq!(run.turns.len(), 6);
    // One work session (the thread) for three rounds, a fresh review each;
    // each review closes with its turn, the thread with the loop.
    assert_eq!(fake.creates.load(Ordering::SeqCst), 4);
    while lock(&fake.released).is_empty() {
        tokio::task::yield_now().await;
    }
    assert_eq!(fake.closes.load(Ordering::SeqCst), 4);
    assert!(lock(&fake.prompts)[2].contains("Evidence for revise"));
    assert!(
        run.turns
            .iter()
            .all(|turn| turn.session_key.is_some() && turn.state == "completed")
    );
    service
        .start("run-1".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    assert_eq!(fake.creates.load(Ordering::SeqCst), 4);
    assert!(
        service
            .start("run-1".into(), "/other".into(), None, fake.clone())
            .await
            .is_err()
    );
}
/// Review B3 (14 Oct 2026): a loop runs as one thread. Its first work round
/// prepares the loop's own worktree, files the work session as the thread,
/// and every later work round is a turn of that session; each review is a
/// fresh session in the same worktree, kept out of Threads. The thread is
/// released when the loop ends.
#[tokio::test]
async fn a_loop_runs_as_one_thread_in_its_own_worktree_with_fresh_reviews() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let mut revisions = service.subscribe();
    let fake = Fake::new(&["continue", "continue", "continue", "complete"]);
    service
        .start("thread".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    let run = settled(&service).await;
    assert_eq!(run.state, State::Completed);
    let thread = run.thread.clone().expect("the loop names its thread");
    assert_eq!(
        thread,
        RunThread {
            key: "codex:0".into(),
            worktree: "/worktrees/project-away".into(),
            branch: Some("iface/away-improve-1a2b3c".into()),
        }
    );
    assert_eq!(fake.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(
        *lock(&fake.bound),
        [("codex:0".to_owned(), "prep-0".to_owned())]
    );
    assert!(lock(&fake.discarded).is_empty());
    // One work session for both rounds, a fresh review each, all in the worktree.
    assert_eq!(fake.creates.load(Ordering::SeqCst), 3);
    assert!(
        lock(&fake.cwds)
            .iter()
            .all(|cwd| cwd == "/worktrees/project-away")
    );
    let sent = lock(&fake.sent)
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    assert_eq!(sent, ["codex:0", "codex:1", "codex:0", "codex:2"]);
    assert!(lock(&fake.prompts)[0].contains("/worktrees/project-away, a git worktree of /project"));
    let turns = run
        .turns
        .iter()
        .map(|turn| (turn.role, turn.session_key.as_deref(), turn.thread_turn))
        .collect::<Vec<_>>();
    assert_eq!(
        turns,
        [
            (Role::Work, Some("codex:0"), Some(0)),
            (Role::Review, Some("codex:1"), None),
            (Role::Work, Some("codex:0"), Some(1)),
            (Role::Review, Some("codex:2"), None),
        ]
    );
    while lock(&fake.released).is_empty() {
        tokio::task::yield_now().await;
    }
    assert_eq!(*lock(&fake.released), ["codex:0"]);
    // Two reviews ended with their turns; the thread's session with the loop.
    assert_eq!(fake.closes.load(Ordering::SeqCst), 3);
    assert_eq!(
        service.hidden_session_keys().await.unwrap(),
        ["codex:1", "codex:2"]
    );
    let views = service.thread_views().await.unwrap();
    assert_eq!(views.len(), 1);
    let (key, away) = &views[0];
    assert_eq!((key.as_str(), away.run_id.as_str()), ("codex:0", "thread"));
    assert_eq!(away.state, State::Completed);
    let verdicts = away
        .rounds
        .iter()
        .map(|round| round.verdict)
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts,
        [
            Some(Verdict::Continue),
            Some(Verdict::Continue),
            Some(Verdict::Continue),
            Some(Verdict::Complete)
        ]
    );
    assert_eq!(away.rounds[2].turn, Some(1));
    assert_eq!(away.rounds[3].session_key.as_deref(), Some("codex:2"));
    assert_eq!(
        away.rounds[3].summary.as_deref(),
        Some("Evidence for complete")
    );
    assert_eq!(away.rounds[3].checks, ["fixture check passed"]);
    assert!(revisions.has_changed().unwrap());
    assert_eq!(
        *revisions.borrow_and_update(),
        service.snapshot().await.unwrap().revision
    );
}
/// A project Away cannot give its own worktree (not a git repository, say)
/// blocks the loop before any session exists; nothing is left behind.
#[tokio::test]
async fn a_refused_worktree_blocks_the_loop_before_any_session() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Arc::new(Fake {
        refuse_prepare: Some(
            "Away works in its own worktree, so the project must be a git repository.",
        ),
        ..Arc::try_unwrap(Fake::new(&["complete"])).ok().unwrap()
    });
    service
        .start("plain".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    let run = settled(&service).await;
    assert_eq!(run.state, State::Blocked);
    assert!(run.detail.contains("git repository"), "{}", run.detail);
    assert!(run.thread.is_none());
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
    assert!(lock(&fake.sent).is_empty() && lock(&fake.released).is_empty());
}
/// What a thread shows of its loop is cut for display: the session keeps the
/// whole result.
#[test]
fn a_threads_rounds_cut_long_results_and_old_loops_keep_their_header() {
    let summary = "é".repeat(400);
    let checks = (0..9)
        .map(|n| format!("check {n} {}", "x".repeat(300)))
        .collect::<Vec<_>>();
    let report = format!(
        "AWAY_RESULT {}",
        serde_json::json!({"outcome": "revise", "summary": summary, "checks": checks})
    );
    let run = Run {
        id: "long".into(),
        cwd: "/project".into(),
        options: Options::default(),
        started_at_ms: 1,
        deadline_at_ms: 3_600_001,
        state: State::Blocked,
        detail: "d".repeat(900),
        turns: vec![Turn {
            iteration: 1,
            role: Role::Review,
            session_key: Some("codex:review".into()),
            state: "completed".into(),
            report,
            thread_turn: None,
        }],
        thread: Some(RunThread {
            key: "codex:work".into(),
            worktree: "/worktrees/project-away".into(),
            branch: None,
        }),
    };
    let (_, away) = thread_view(&run, true).unwrap();
    let round = &away.rounds[0];
    assert_eq!(round.verdict, Some(Verdict::Revise));
    assert_eq!(round.summary.as_ref().unwrap().len(), 512);
    assert_eq!(round.checks.len(), 4);
    assert!(round.checks.iter().all(|check| check.len() <= 200));
    assert_eq!(away.detail.len(), 512);
    assert!(thread_view(&run, false).unwrap().1.rounds.is_empty());
    let legacy = Run {
        thread: None,
        ..run
    };
    assert!(thread_view(&legacy, true).is_none());
}
#[tokio::test]
async fn stop_cancels_noncooperative_turn_and_closes_only_its_session() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Arc::new(Fake {
        waiting: true,
        ..Arc::try_unwrap(Fake::new(&[])).ok().unwrap()
    });
    service
        .start("stop".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    while lock(&fake.prompts).is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(
        service
            .start("duplicate".into(), "/project".into(), None, fake.clone())
            .await
            .is_err()
    );
    service.stop("stop").await.unwrap();
    let run = settled(&service).await;
    assert_eq!(run.state, State::Stopped);
    assert_eq!(run.turns[0].state, "cancelled");
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
    while lock(&fake.released).is_empty() {
        tokio::task::yield_now().await;
    }
    // Stop ended the thread's turn at once; the loop's end closed it again.
    let thread = run.thread.unwrap().key;
    assert_eq!(*lock(&fake.closed), [thread.clone(), thread.clone()]);
    assert_eq!(*lock(&fake.released), [thread]);
}
#[tokio::test]
async fn deadline_cancels_without_admitting_review() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Fake::new(&["continue"]);
    let run = Run {
        id: "expiry".into(),
        cwd: "/project".into(),
        options: Options::default(),
        started_at_ms: 1,
        deadline_at_ms: 3_600_001,
        state: State::Running,
        detail: "fixture".into(),
        turns: vec![],
        thread: None,
    };
    {
        let mut store = service.store.lock().await;
        let mut next = store.snapshot.clone();
        next.runs.push(run);
        store.commit(next).unwrap();
    }
    let (_, cancel) = watch::channel(false);
    service.drive("expiry", fake.clone(), cancel).await.unwrap();
    assert_eq!(
        service.snapshot().await.unwrap().runs[0].state,
        State::Expired
    );
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unknown_provider_and_missing_result_never_continue() {
    for unknown in [false, true] {
        let root = private_root();
        let service = Service::open(root.path()).unwrap();
        let fake = Fake::new(&[]);
        lock(&fake.answers).push_back(if unknown {
            Err(Failure {
                message: "Lost response".into(),
                unknown: true,
            })
        } else {
            Ok("Done!".into())
        });
        service
            .start("uncertain".into(), "/project".into(), None, fake.clone())
            .await
            .unwrap();
        assert_eq!(
            settled(&service).await.state,
            if unknown {
                State::Unknown
            } else {
                State::Blocked
            }
        );
        assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn recovery_preserves_project_presets_and_fences_started_effects() {
    let root = private_root();
    {
        let mut store = Store::open(root.path()).unwrap();
        let mut next = store.snapshot.clone();
        set_preset(&mut next, "/project".into(), Options::default());
        next.runs.push(Run {
            id: "interrupted".into(),
            cwd: "/project".into(),
            options: Options::default(),
            started_at_ms: 1,
            deadline_at_ms: 3_600_001,
            state: State::Running,
            detail: "Working".into(),
            turns: vec![Turn {
                iteration: 1,
                role: Role::Work,
                session_key: Some("codex:exact".into()),
                state: "started".into(),
                report: String::new(),
                thread_turn: None,
            }],
            thread: None,
        });
        store.commit(next).unwrap();
    }
    let service = Service::open(root.path()).unwrap();
    let snapshot = service.snapshot().await.unwrap();
    assert_eq!(snapshot.runs[0].state, State::Unknown);
    assert_eq!(snapshot.runs[0].turns[0].state, "unknown");
    assert_eq!(snapshot.presets.len(), 1);
}
#[test]
fn written_bytes_match_the_pinned_format() {
    let root = private_root();
    let file = root.path().join("project-away.v3.json");
    // Protocol 47: Claude reviews this Codex loop.
    let options = Options {
        reviewer: Some("claude".into()),
        ..Options::default()
    };
    {
        let mut store = Store::open(root.path()).unwrap();
        let mut next = store.snapshot.clone();
        set_preset(&mut next, "/project".into(), options.clone());
        next.runs.push(Run {
            id: "interrupted".into(),
            cwd: "/project".into(),
            options,
            started_at_ms: 1,
            deadline_at_ms: 3_600_001,
            state: State::Running,
            detail: "Working".into(),
            turns: vec![
                Turn {
                    iteration: 1,
                    role: Role::Work,
                    session_key: Some("codex:exact".into()),
                    state: "completed".into(),
                    report: "partial \"report\"".into(),
                    thread_turn: Some(0),
                },
                Turn {
                    iteration: 1,
                    role: Role::Review,
                    session_key: Some("codex:review".into()),
                    state: "started".into(),
                    report: String::new(),
                    thread_turn: None,
                },
            ],
            thread: Some(RunThread {
                key: "codex:exact".into(),
                worktree: "/worktrees/project-away".into(),
                branch: Some("iface/away-improve-1a2b3c".into()),
            }),
        });
        store.commit(next).unwrap();
        crate::golden::assert_golden("project-away.v3.committed.json", root.path(), &file, &[]);
    }
    // Reopening fences the running loop and its started turn, then rewrites.
    let store = Store::open(root.path()).unwrap();
    crate::golden::assert_golden("project-away.v3.recovered.json", root.path(), &file, &[]);
    assert!(root.path().join("project-away.lock").exists());
    assert!(!root.path().join("project-away.v1.json").exists());
    assert!(!root.path().join("project-away.v2.json").exists());
    drop(store);
}
/// The version 1 file on owners' disks (the pinned
/// `project-away.committed.json` bytes) is read once, carried over with every
/// run's thread and reviewer absent and its interrupted loop fenced, and never
/// changed, moved or read again (review B3; since protocol 47 into version 3).
#[test]
fn a_v1_store_is_carried_over_once_and_left_unchanged() {
    use std::os::unix::fs::PermissionsExt;
    let root = private_root();
    let legacy = root.path().join("project-away.v1.json");
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/project-away.committed.json"),
    )
    .unwrap();
    std::fs::write(&legacy, &bytes).unwrap();
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(root.path()).unwrap();
    assert_eq!(
        store.snapshot.version,
        interface_contracts::away::STORE_VERSION
    );
    let run = &store.snapshot.runs[0];
    assert_eq!((run.state, run.thread.as_ref()), (State::Unknown, None));
    assert_eq!(run.turns[0].state, "unknown");
    assert_eq!(store.snapshot.presets.len(), 1);
    let file = root.path().join("project-away.v3.json");
    crate::golden::assert_golden("project-away.v3.carried-v1.json", root.path(), &file, &[]);
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        bytes,
        "the v1 file is left as it was"
    );
    drop(store);
    // The current version exists now, so the old file is never consulted again.
    std::fs::write(&legacy, b"{}").unwrap();
    let again = Store::open(root.path()).unwrap();
    assert_eq!(again.snapshot.runs.len(), 1);
    crate::golden::assert_golden("project-away.v3.carried-v1.json", root.path(), &file, &[]);
}
/// Store version 3 (protocol 47): a version 2 file (the pinned
/// `project-away.v2.committed.json` bytes) is read once, before any version 1
/// file, and carried over unchanged but for its version: no loop names a
/// reviewer, so each keeps reviewing with its worker's provider.
#[test]
fn a_v2_store_is_carried_over_and_its_loops_review_as_before() {
    use std::os::unix::fs::PermissionsExt;
    let root = private_root();
    let golden = |name: &str| {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden")
                .join(name),
        )
        .unwrap()
    };
    for (name, bytes) in [
        (
            "project-away.v2.json",
            golden("project-away.v2.committed.json"),
        ),
        (
            "project-away.v1.json",
            golden("project-away.committed.json"),
        ),
    ] {
        let path = root.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let store = Store::open(root.path()).unwrap();
    let run = &store.snapshot.runs[0];
    assert!(
        run.thread.is_some(),
        "carried from version 2, not version 1"
    );
    assert_eq!(run.options.reviewer, None);
    assert_eq!(run.options.review_provider(), "codex");
    assert_eq!(super::session_provider(run, Role::Review), "codex");
    let file = root.path().join("project-away.v3.json");
    crate::golden::assert_golden("project-away.v3.carried-v2.json", root.path(), &file, &[]);
    assert_eq!(
        std::fs::read(root.path().join("project-away.v2.json")).unwrap(),
        golden("project-away.v2.committed.json"),
        "the v2 file is left as it was"
    );
    // A reviewer in a version 2 file is not version 2: refused, left alone.
    let other = private_root();
    let path = other.path().join("project-away.v2.json");
    let forged = String::from_utf8(golden("project-away.v2.committed.json"))
        .unwrap()
        .replace(
            "\"maxIterations\": 12\n",
            "\"maxIterations\": 12,\n        \"reviewer\": \"claude\"\n",
        );
    std::fs::write(&path, &forged).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Store::open(other.path()).is_err());
    assert!(!other.path().join("project-away.v3.json").exists());
}
/// An unreadable version 1 store keeps Away unavailable, as it always did:
/// its unknown loops are never dropped for an empty history.
#[test]
fn an_unreadable_v1_store_is_refused_and_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let root = private_root();
    let legacy = root.path().join("project-away.v1.json");
    std::fs::write(&legacy, b"{\"version\":1}").unwrap();
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Store::open(root.path()).is_err());
    assert_eq!(std::fs::read(&legacy).unwrap(), b"{\"version\":1}");
    assert!(!root.path().join("project-away.v3.json").exists());
}
#[tokio::test]
async fn iteration_limit_and_three_rejected_reviews_are_finite() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Fake::new(&[
        "continue", "revise", "continue", "revise", "continue", "revise",
    ]);
    service
        .start(
            "bounded".into(),
            "/project".into(),
            Some(Options {
                max_iterations: 1,
                ..Options::default()
            }),
            fake.clone(),
        )
        .await
        .unwrap();
    let run = settled(&service).await;
    // A rejected candidate at the limit is not reported as completed work.
    assert_eq!(run.turns.len(), 2);
    assert_eq!(fake.creates.load(Ordering::SeqCst), 2);
}
#[test]
fn result_contract_is_strict_and_unicode_safe() {
    assert_eq!(
        parse_reply(
            "Work finished.AWAY_RESULT {\"outcome\":\"complete\",\"summary\":\"ok\",\"checks\":[]}"
        )
        .unwrap()
        .outcome,
        Outcome::Complete
    );
    assert!(parse_reply("AWAY_RESULT {\"outcome\":\"complete\",\"summary\":\"ok\",\"checks\":[],\"extendDeadline\":true}").is_err());
    assert!(
        Options {
            duration_seconds: 0,
            ..Options::default()
        }
        .validate()
        .is_err()
    );
}

fn private_root() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}

#[tokio::test]
async fn stop_before_start_is_durable_and_cannot_be_overtaken() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    service.stop("late-start").await.unwrap();
    let fake = Fake::new(&["complete"]);
    service
        .start("late-start".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    assert_eq!(
        service.snapshot().await.unwrap().runs[0].state,
        State::Stopped
    );
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn view_keeps_recent_and_active_runs_and_cuts_reports_at_a_character() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let run = |id: &str, state: State, report: &str| Run {
        id: id.into(),
        cwd: "/project".into(),
        options: Options::default(),
        started_at_ms: 1,
        deadline_at_ms: 3_600_001,
        state,
        detail: "Detail".into(),
        turns: vec![Turn {
            iteration: 1,
            role: Role::Work,
            session_key: Some("codex:exact".into()),
            state: "completed".into(),
            report: report.into(),
            thread_turn: None,
        }],
        thread: None,
    };
    {
        let mut store = service.store.lock().await;
        let mut next = store.snapshot.clone();
        next.runs = vec![
            run("old-active", State::Running, "short"),
            run("old-done", State::Completed, "short"),
            run("recent", State::Completed, &"é".repeat(4)),
        ];
        store.commit(next).unwrap();
    }
    // "éééé" is 8 bytes; a 5-byte tail starts inside the second character.
    let view = service.view(1, 5).await.unwrap();
    let ids: Vec<_> = view.runs.iter().map(|run| run.id.as_str()).collect();
    assert_eq!(ids, ["old-active", "recent"]);
    assert_eq!(view.runs[0].turns[0].report, "short");
    assert_eq!(
        view.runs[1].turns[0].report,
        "(full result in session history) … éé"
    );
    // The store keeps every run and the complete report.
    let complete = service.snapshot().await.unwrap();
    assert_eq!(complete.runs.len(), 3);
    assert_eq!(complete.runs[2].turns[0].report, "éééé");
    let mut expected = complete;
    // The wire snapshot stays version 1; the store is version 2.
    expected.version = VERSION;
    expected.runs.remove(1);
    expected.runs[1].turns[0].report = view.runs[1].turns[0].report.clone();
    assert_eq!(view, expected);
}

#[tokio::test]
async fn full_history_refuses_without_fencing_storage_and_finishes_unknown() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    // The run and its receipts fit; the long report cannot.
    service.store.lock().await.set_file_bytes(8 * 1024);
    let fake = Fake::new(&[]);
    lock(&fake.answers).push_back(Ok(format!(
        "{}\nAWAY_RESULT {{\"outcome\":\"complete\",\"summary\":\"done\",\"checks\":[]}}",
        "x".repeat(16 * 1024)
    )));
    service
        .start("full".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    let run = settled(&service).await;
    // The refused report left storage trusted, so the finish still saved.
    assert_eq!(run.state, State::Unknown);
    assert_eq!(run.turns[0].state, "unknown");
    assert!(run.turns[0].report.is_empty());
    let snapshot = service.snapshot().await.unwrap();
    assert!(!snapshot.storage_uncertain);
    let on_disk = std::fs::read_to_string(root.path().join("project-away.v3.json")).unwrap();
    assert!(on_disk.contains("\"unknown\""));
    assert!(!on_disk.contains(&"x".repeat(64)));
    // New work stays fenced; no run is added and nothing launches.
    let error = service
        .start("next".into(), "/other".into(), None, fake.clone())
        .await
        .unwrap_err();
    assert!(error.contains("size limit"), "{error}");
    assert_eq!(service.snapshot().await.unwrap().runs.len(), 1);
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn a_write_that_failed_before_its_rename_leaves_storage_trusted() {
    let root = private_root();
    let file = root.path().join("project-away.v3.json");
    let mut store = Store::open(root.path()).unwrap();
    let mut next = store.snapshot.clone();
    set_preset(&mut next, "/project".into(), Options::default());
    store.commit(next.clone()).unwrap();
    let before = std::fs::read(&file).unwrap();
    set_preset(&mut next, "/other".into(), Options::default());
    {
        let _immutable = crate::snapshot_file::ImmutableDirectory::new(root.path());
        let error = store.commit(next.clone()).unwrap_err();
        assert!(error.contains("failed before rename"), "{error}");
    }
    assert!(!store.snapshot.storage_uncertain);
    assert!(!store.history_full);
    assert_eq!(std::fs::read(&file).unwrap(), before);
    store.commit(next).unwrap();
    assert_eq!(store.snapshot.presets.len(), 2);
}

#[tokio::test]
async fn tampered_or_oversize_target_still_fences_storage() {
    for replacement in [b"{}\n".to_vec(), vec![b' '; 16 * 1024]] {
        let root = private_root();
        let service = Service::open(root.path()).unwrap();
        service.store.lock().await.set_file_bytes(8 * 1024);
        service
            .save("/project".into(), Options::default())
            .await
            .unwrap();
        let path = root.path().join("project-away.v3.json");
        std::fs::write(&path, &replacement).unwrap();
        let error = service
            .save("/other".into(), Options::default())
            .await
            .unwrap_err();
        assert!(error.contains("uncertain"), "{error}");
        assert!(service.snapshot().await.unwrap().storage_uncertain);
        assert_eq!(std::fs::read(&path).unwrap(), replacement);
    }
}

#[tokio::test]
async fn a_long_turn_report_keeps_its_conclusion() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Fake::new(&["complete"]);
    let result =
        "AWAY_RESULT {\"outcome\":\"continue\",\"summary\":\"the conclusion\",\"checks\":[]}";
    lock(&fake.answers).push_front(Ok(format!("{}\n{result}", "narration ".repeat(5_000))));
    service
        .start("long".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    let run = settled(&service).await;
    assert_eq!(run.state, State::Completed);
    let report = &run.turns[0].report;
    assert!(report.len() <= 32_768);
    assert!(report.ends_with(result), "stored report lost its tail");
    let excerpt = &service.view(1, 4_096).await.unwrap().runs[0].turns[0].report;
    assert!(excerpt.starts_with("(full result in session history) … "));
    assert!(excerpt.ends_with(result));
}

#[tokio::test(flavor = "current_thread")]
async fn slow_storage_does_not_block_the_runtime_or_prompt_cancellation() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Arc::new(Fake {
        waiting: true,
        ..Arc::try_unwrap(Fake::new(&[])).ok().unwrap()
    });
    service
        .start("slow-stop".into(), "/project".into(), None, fake.clone())
        .await
        .unwrap();
    while lock(&fake.prompts).is_empty() {
        tokio::task::yield_now().await;
    }
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let writer = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .with_store(move |_| {
                    let _ = entered.send(());
                    held.recv_timeout(Duration::from_secs(2))
                        .expect("test released storage");
                    Ok(())
                })
                .await
        })
    };
    ready.await.unwrap();
    let stopping = {
        let service = service.clone();
        tokio::spawn(async move { service.stop("slow-stop").await })
    };
    // This single-thread runtime must deliver Stop and close the provider while
    // the blocking task still owns storage. No timing benchmark assertion.
    let cancelled = tokio::time::timeout(Duration::from_millis(500), async {
        while fake.closes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    release.send(()).unwrap();
    cancelled.expect("cancellation must not wait for storage");
    writer.await.unwrap().unwrap();
    stopping.await.unwrap().unwrap();
    assert_eq!(settled(&service).await.state, State::Stopped);
    service.shutdown().await;
}

#[tokio::test]
async fn cancelling_the_start_caller_cannot_strand_a_committed_run() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Fake::new(&["continue", "complete"]);
    let held = service.store.lock().await;
    let caller = {
        let service = service.clone();
        let fake = fake.clone();
        tokio::spawn(async move {
            service
                .start("orphan".into(), "/project".into(), None, fake)
                .await
        })
    };
    while service.tasks.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(held);
    tokio::time::timeout(SETTLE_BOUND, async {
        while service.snapshot().await.unwrap().runs.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled(&service).await.state, State::Completed);
    service.shutdown().await;
    assert_eq!(fake.creates.load(Ordering::SeqCst), 2);
    assert_eq!(fake.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shutdown_drains_pending_admission_without_launching() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    let fake = Fake::new(&[]);
    let held = service.store.lock().await;
    let caller = {
        let service = service.clone();
        let fake = fake.clone();
        tokio::spawn(async move {
            service
                .start("closing".into(), "/project".into(), None, fake)
                .await
        })
    };
    while service.tasks.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    let closing = {
        let service = service.clone();
        tokio::spawn(async move { service.shutdown().await })
    };
    while !service.closing.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
    drop(held);
    assert!(caller.await.unwrap().is_err());
    closing.await.unwrap();
    assert!(service.snapshot().await.unwrap().runs.is_empty());
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn interrupted_storage_fences_effects_and_unchanged_presets_skip_writes() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    service
        .save("/project".into(), Options::default())
        .await
        .unwrap();
    let revision = service.snapshot().await.unwrap().revision;
    service
        .save("/project".into(), Options::default())
        .await
        .unwrap();
    assert_eq!(service.snapshot().await.unwrap().revision, revision);
    assert!(
        service
            .with_store::<()>(|_| panic!("interrupted fixture transition"))
            .await
            .is_err()
    );
    assert!(service.snapshot().await.unwrap().storage_uncertain);
    assert!(
        service
            .save("/project".into(), Options::default())
            .await
            .is_err()
    );
    let fake = Fake::new(&[]);
    assert!(
        service
            .start("fenced".into(), "/other".into(), None, fake.clone())
            .await
            .is_err()
    );
    service.shutdown().await;
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn compact_status_reconciles_all_starts_without_transferring_reports() {
    let root = private_root();
    let service = Service::open(root.path()).unwrap();
    {
        let mut store = service.store.lock().await;
        let mut next = store.snapshot.clone();
        for n in 0..128 {
            next.runs.push(Run {
                id: format!("summary-{n}"),
                cwd: "/project".into(),
                options: Options::default(),
                started_at_ms: 1,
                deadline_at_ms: 3_600_001,
                state: if n == 0 {
                    State::Running
                } else {
                    State::Completed
                },
                detail: "fixture".into(),
                turns: vec![Turn {
                    iteration: 1,
                    role: Role::Work,
                    session_key: None,
                    state: "completed".into(),
                    report: "report body ".repeat(2000),
                    thread_turn: None,
                }],
                thread: None,
            });
        }
        next.cancelled_starts.push("cancelled".into());
        store.commit(next).unwrap();
    }
    let status = service.status().await.unwrap();
    let bytes = serde_json::to_vec(&status).unwrap();
    assert!(bytes.len() < 4096, "{} bytes", bytes.len());
    assert!(!String::from_utf8(bytes).unwrap().contains("report body"));
    assert_eq!(status.run_ids.len(), 128);
    assert_eq!(status.active_runs.len(), 1);
    assert_eq!(status.active_runs[0].id, "summary-0");
    assert_eq!(status.cancelled_starts, ["cancelled"]);
    let revision = status.revision;
    service.store.lock().await.snapshot.storage_uncertain = true;
    let uncertain = service.status().await.unwrap();
    assert_eq!(uncertain.revision, revision);
    assert!(uncertain.storage_uncertain);
}

// Reproducible synthetic audit workload; wall-clock timings are not test gates.
#[ignore = "manual release performance measurement"]
#[tokio::test(flavor = "current_thread")]
async fn benchmark_away_snapshot_cost() {
    for count in [0usize, 32, 128] {
        let root = private_root();
        let service = Service::open(root.path()).unwrap();
        {
            let mut store = service.store.lock().await;
            let mut next = store.snapshot.clone();
            for n in 0..count {
                next.runs.push(Run {
                    id: format!("audit-{n}"),
                    cwd: "/audit-fixture".into(),
                    options: Options::default(),
                    started_at_ms: 1,
                    deadline_at_ms: 3_600_001,
                    state: State::Completed,
                    detail: "synthetic".into(),
                    turns: (0..16)
                        .map(|i| Turn {
                            iteration: i / 2 + 1,
                            role: if i % 2 == 0 { Role::Work } else { Role::Review },
                            session_key: None,
                            state: "completed".into(),
                            report: "x".repeat(8192),
                            thread_turn: None,
                        })
                        .collect(),
                    thread: None,
                });
            }
            store.commit(next).unwrap();
        }
        let bytes = std::fs::metadata(root.path().join("project-away.v3.json"))
            .unwrap()
            .len();
        let mut timings = vec![];
        for n in 0..4 {
            let start = std::time::Instant::now();
            service
                .save(
                    "/audit-fixture".into(),
                    Options {
                        objective: format!("audit {n}"),
                        ..Options::default()
                    },
                )
                .await
                .unwrap();
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let start = std::time::Instant::now();
        let view = service.view(36, 4096).await.unwrap();
        let wire = serde_json::to_vec(&view).unwrap();
        let view_ms = start.elapsed().as_secs_f64() * 1000.0;
        let beat = tokio::spawn(async {
            let start = std::time::Instant::now();
            tokio::time::sleep(Duration::from_millis(5)).await;
            start.elapsed().as_secs_f64() * 1000.0
        });
        tokio::task::yield_now().await;
        service
            .save("/audit-fixture".into(), Options::default())
            .await
            .unwrap();
        let heartbeat_ms = beat.await.unwrap();
        let compact_bytes = serde_json::to_vec(&service.status().await.unwrap())
            .unwrap()
            .len();
        println!(
            "AUDIT_AWAY runs={count} file_bytes={bytes} save_ms={timings:?} view_wire_bytes={} compact_bytes={compact_bytes} view_and_encode_ms={view_ms:.3} five_ms_heartbeat_ms={heartbeat_ms:.3}",
            wire.len()
        );
    }
}
/// Protocol 47 (owner, 14 Oct 2026: "Other vendor by default"): a review
/// runs on the reviewer the loop names, at that provider's own default model
/// and with that provider's read-only restrictions; without one, the worker's
/// provider and model review, as before.
#[test]
fn a_review_runs_on_the_chosen_reviewer_with_its_own_restrictions() {
    use interface_contracts::engine::SandboxMode;
    let run = |provider: &str, reviewer: Option<&str>| Run {
        id: "reviewer".into(),
        cwd: "/project".into(),
        options: Options {
            provider: provider.into(),
            model: Some(format!("{provider}-model")),
            reviewer: reviewer.map(str::to_owned),
            ..Options::default()
        },
        started_at_ms: 1,
        deadline_at_ms: 3_600_001,
        state: State::Running,
        detail: "fixture".into(),
        turns: vec![],
        thread: None,
    };
    // Codex works, Claude reviews: the limited Claude, at its own default.
    let codex_claude = run("codex", Some("claude"));
    assert_eq!(session_provider(&codex_claude, Role::Work), "codex");
    assert_eq!(session_provider(&codex_claude, Role::Review), "claude");
    let work = launch_options(&codex_claude, Role::Work, 1);
    assert_eq!(
        (work.permission_mode.as_deref(), work.model.as_deref()),
        (Some("never"), Some("codex-model"))
    );
    let review = launch_options(&codex_claude, Role::Review, 1);
    assert_eq!(review.permission_mode.as_deref(), Some("acceptEdits"));
    assert_eq!(review.model, None);
    let denied = review.disallowed_tools.expect("Claude reviewer limits");
    for tool in ["Write", "Edit", "Bash", "WebFetch"] {
        assert!(denied.iter().any(|denied| denied == tool), "{tool}");
    }
    // Claude works, Codex reviews: read-only Codex.
    let claude_codex = run("claude", Some("codex"));
    let review = launch_options(&claude_codex, Role::Review, 1);
    assert_eq!(review.sandbox_mode, Some(SandboxMode::ReadOnly));
    assert_eq!(review.permission_mode.as_deref(), Some("never"));
    assert!(review.disallowed_tools.is_none() && review.model.is_none());
    // No reviewer named (every loop before protocol 47): the worker reviews.
    let same = run("codex", None);
    assert_eq!(session_provider(&same, Role::Review), "codex");
    let review = launch_options(&same, Role::Review, 1);
    assert_eq!(review.model.as_deref(), Some("codex-model"));
    assert_eq!(review.sandbox_mode, Some(SandboxMode::ReadOnly));
    // The thread names its reviewer.
    let threaded = Run {
        thread: Some(RunThread {
            key: "codex:work".into(),
            worktree: "/worktrees/project-away".into(),
            branch: None,
        }),
        ..codex_claude
    };
    assert_eq!(
        thread_view(&threaded, true).unwrap().1.reviewer.as_deref(),
        Some("claude")
    );
    let threaded_same = Run {
        thread: threaded.thread.clone(),
        ..same
    };
    assert_eq!(
        thread_view(&threaded_same, true)
            .unwrap()
            .1
            .reviewer
            .as_deref(),
        Some("codex")
    );
}
#[test]
fn claude_runs_sandboxed_without_bypass_and_codex_keeps_its_sandbox() {
    use interface_contracts::engine::SandboxMode;
    let run = |provider: &str| Run {
        id: "sandbox".into(),
        cwd: "/project".into(),
        options: Options {
            provider: provider.into(),
            ..Options::default()
        },
        started_at_ms: 1,
        deadline_at_ms: 2,
        state: State::Running,
        detail: "fixture".into(),
        turns: vec![],
        thread: None,
    };
    let claude = run("claude");
    for role in [Role::Work, Role::Review] {
        let options = launch_options(&claude, role, 1);
        assert_eq!(options.permission_mode.as_deref(), Some("acceptEdits"));
        assert_eq!(options.sandbox_mode, Some(SandboxMode::WorkspaceWrite));
        let disallowed = options.disallowed_tools.expect("Claude tool limits");
        assert!(disallowed.iter().any(|tool| tool == "WebFetch"));
    }
    let codex = run("codex");
    let work = launch_options(&codex, Role::Work, 1);
    assert_eq!(work.permission_mode.as_deref(), Some("never"));
    assert_eq!(work.sandbox_mode, Some(SandboxMode::WorkspaceWrite));
    assert_eq!(
        launch_options(&codex, Role::Review, 1).sandbox_mode,
        Some(SandboxMode::ReadOnly)
    );
}
