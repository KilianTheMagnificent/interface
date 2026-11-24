//! Switch provider inside a thread (T3 section 5, protocol 59) through the
//! thread host with fake ports: a fake guarded runtime that creates,
//! resumes and sends through the real send hook, and a journal the test
//! writes for each provider's stretch.

use std::sync::OnceLock;

use interface_contracts::{
    thread_journal::{JournalEntry, TurnOrigin},
    threads::{HandoffDelivery, RunOutcome},
};
use interface_settings::{
    thread_journal::ThreadJournal,
    thread_team::{THREAD_TEAM_FILE_NAME, ThreadTeamStore},
    threads::{HANDOFF_PREFIXES, THREADS_SWITCH_PLAIN_CODE},
};
use serde_json::{Value, json};

use super::*;
use crate::dispatcher::{SendHooks, SendSettlement, SessionAccessMemory};
use crate::threads::{
    THREADS_SWITCH_MODE_CODE, THREADS_SWITCH_NOT_READY_CODE,
    opinion::{THREADS_SAME_PROVIDER_CODE, THREADS_SWITCH_MISSING_CODE},
    terminal_bridge::{THREADS_IN_TERMINAL_CODE, THREADS_SESSION_BUSY_CODE},
};

const THREAD: &str = "claude#a";

/// The guarded runtime as a switch sees it: creates register a registry
/// row under their name, resumes open a session, sends run the real send
/// hook (turn, journal prompt, echo) and settle it.
#[derive(Default)]
struct FakeRuntime {
    host: OnceLock<ThreadHost>,
    sessions: OnceLock<Arc<FakeSessions>>,
    next: AtomicU64,
    creates: Mutex<Vec<Value>>,
    resumes: Mutex<Vec<String>>,
    /// Each send: key, text, whether the pane was shown it (echo).
    sends: Mutex<Vec<(String, String, bool)>>,
    create_fails: Mutex<Option<DispatchError>>,
    resume_fails: Mutex<Option<DispatchError>>,
    send_fails: Mutex<Option<DispatchError>>,
}

#[async_trait]
impl HostDispatcher for FakeRuntime {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let host = self.host.get().unwrap();
        let sessions = self.sessions.get().unwrap();
        if operation == CHANNELS.create_session {
            lock(&self.creates).push(payload.clone());
            if let Some(error) = lock(&self.create_fails).clone() {
                return Err(error);
            }
            let engine = payload["engine"].as_str().unwrap();
            let key = format!("{engine}#new-{}", self.next.fetch_add(1, Ordering::SeqCst));
            let mut summary = row(&key, SessionKind::Code);
            summary.cwd = payload["options"]["cwd"].as_str().unwrap().to_owned();
            if let Some(name) = payload["options"]["name"].as_str() {
                name.clone_into(&mut summary.title);
            }
            sessions.upsert(summary);
            lock(&sessions.live).insert(key.clone());
            return Ok(json!({"key": key}));
        }
        if operation == CHANNELS.resume_session {
            let key = payload["sessionKey"].as_str().unwrap().to_owned();
            lock(&self.resumes).push(key.clone());
            if let Some(error) = lock(&self.resume_fails).clone() {
                return Err(error);
            }
            lock(&sessions.live).insert(key.clone());
            return Ok(json!({"key": key}));
        }
        if operation == CHANNELS.send {
            let key = payload["sessionKey"].as_str().unwrap().to_owned();
            let text = payload["text"].as_str().unwrap().to_owned();
            let ticket = host.before_send(&key, &text).await?;
            let echo = ticket.as_ref().is_some_and(|ticket| ticket.echoes_prompt());
            let failed = lock(&self.send_fails).clone();
            if let Some(ticket) = ticket {
                ticket.settle(match &failed {
                    Some(error) if error.replay_safe => SendSettlement::Unsent,
                    _ => SendSettlement::Delivered,
                });
            }
            lock(&self.sends).push((key, text, echo));
            return failed.map_or(Ok(Value::Null), Err);
        }
        if operation == CHANNELS.probe_engines {
            let ready = |engine: &str| {
                json!({"engine": engine, "ok": true, "version": "1.0.0", "info": {
                    "binaryPath": format!("/usr/local/bin/{engine}"), "version": "1.0.0",
                    "loginIdentity": null,
                    "capabilities": {"streaming": true, "approvals": true, "resume": true,
                        "fork": true, "cloudSessions": false, "models": [],
                        "effortLevels": [], "permissionModes": []}}})
            };
            return Ok(json!([
                ready("claude"),
                ready("codex"),
                ready("acp:grok"),
                {"engine": "acp:qwen", "ok": false, "error": "Qwen's model server is not running."}
            ]));
        }
        Err(DispatchError::new(
            "operation-not-migrated",
            operation,
            false,
            true,
        ))
    }
}

#[derive(Default)]
struct FakeAccess(Mutex<HashMap<String, String>>);

impl SessionAccessMemory for FakeAccess {
    fn mode(&self, session_key: &str) -> Option<String> {
        lock(&self.0).get(session_key).cloned()
    }

    fn remember(&self, session_key: &str, mode: Option<&str>) {
        match mode {
            Some(mode) => lock(&self.0).insert(session_key.to_owned(), mode.to_owned()),
            None => lock(&self.0).remove(session_key),
        };
    }
}

use std::collections::HashMap;

struct Switch {
    harness: Harness,
    runtime: Arc<FakeRuntime>,
    access: Arc<FakeAccess>,
    root: String,
    journal: ThreadJournal,
    at: u64,
}

/// A journalled host with a guarded runtime and one idle, pinned Local
/// thread on Claude in `root`, whose first turn fixed the login and whose
/// session is live.
async fn switch() -> Switch {
    let mut harness = journal_harness();
    harness.git.moves.store(true, Ordering::SeqCst);
    let root = harness.dir("atlas");
    harness
        .git_thread(THREAD, &root, None, "fix the login")
        .await;
    end_run(&harness, THREAD).await;
    harness
        .command(json!({"kind": "pin", "key": THREAD}))
        .await
        .unwrap();
    lock(&harness.sessions.live).insert(THREAD.to_owned());
    let runtime = Arc::new(FakeRuntime::default());
    let _ = runtime.host.set(harness.host.clone());
    let _ = runtime.sessions.set(harness.sessions.clone());
    let access = Arc::new(FakeAccess::default());
    access.remember(THREAD, Some("acceptEdits"));
    let store = ThreadTeamStore::open(&harness.state.path().join(THREAD_TEAM_FILE_NAME))
        .map_err(|error| error.to_string());
    harness.host.attach_team(ThreadTeamConfig {
        runtime: runtime.clone(),
        access: Some(access.clone()),
        store,
    });
    let journal = ThreadJournal::open(harness.state.path()).unwrap();
    let mut switch = Switch {
        harness,
        runtime,
        access,
        root,
        journal,
        at: NOW - 10_000,
    };
    switch.turn(
        THREAD,
        TurnOrigin::Launch,
        "fix the login",
        "Retries now back off.",
    );
    switch
}

async fn end_run(harness: &Harness, key: &str) {
    let target = key.to_owned();
    harness
        .host
        .edit_quiet(move |document, now| {
            document.run_ended(&target, now, RunOutcome::Completed);
            Ok(())
        })
        .await
        .unwrap();
    harness.host.close_run(key);
}

impl Switch {
    /// One whole turn in `key`'s journal: its prompt, answer and end.
    fn turn(&mut self, key: &str, origin: TurnOrigin, prompt: &str, answer: &str) {
        for entry in [
            JournalEntry::TurnStarted {
                origin,
                prompt: prompt.into(),
                digest: prompt_digest(prompt),
                cut: false,
            },
            JournalEntry::Message {
                text: answer.into(),
                agent_key: None,
                cut: false,
            },
            JournalEntry::TurnEnded {
                outcome: RunOutcome::Completed,
                reason: None,
            },
        ] {
            self.at += 1;
            self.journal.append(key, self.at, entry).unwrap();
        }
    }

    async fn to(&self, key: &str, engine: &str) -> Result<ThreadControlResponse, DispatchError> {
        self.harness
            .command(json!({"kind": "switchProvider", "key": key, "engine": engine}))
            .await
    }

    fn sends_to(&self, key: &str) -> Vec<(String, bool)> {
        lock(&self.runtime.sends)
            .iter()
            .filter(|(to, _, _)| to == key)
            .map(|(_, text, echo)| (text.clone(), *echo))
            .collect()
    }

    /// The new head's handoff ran and ended.
    async fn finish(&self, key: &str) {
        end_run(&self.harness, key).await;
    }
}

/// T3 section 5: switching to a provider the thread has not used starts a
/// session in the thread's own folder on the guarded runtime (kind code, the
/// thread's title, at its permission rung and never wider), makes it the
/// head in one commit (same row, same pin), closes the old head's session,
/// and sends it the handoff built from the journal as a host prompt: shown
/// folded in the pane, recorded as turn 0 on its own checkpoint.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One switch, then each of its effects.
async fn switching_to_a_new_provider_hands_the_same_thread_over_from_its_journal() {
    let switch = switch().await;
    let before = switch.harness.document().await;
    let response = switch.to(THREAD, "codex").await.unwrap();
    let head = response.key.clone().expect("the new head");
    assert_eq!(head, "codex#new-0");
    assert_eq!(response.handoff, Some(HandoffDelivery::Sent));

    let creates = lock(&switch.runtime.creates).clone();
    assert_eq!(creates.len(), 1);
    let options = &creates[0]["options"];
    assert_eq!(creates[0]["engine"], "codex");
    assert_eq!(
        options["cwd"],
        switch.root.as_str(),
        "the thread's own folder"
    );
    assert_eq!(options["kind"], "code");
    assert_eq!(
        options["name"], "Title a",
        "the thread's title, not the handoff"
    );
    assert_eq!(options["prompt"], "", "bound with no turn");
    assert_eq!(
        options["permissionMode"], "on-request",
        "Claude's acceptEdits is the edits rung; Codex's edits mode, never wider"
    );
    assert!(options.get("collaborationDepth").is_none());
    assert!(lock(&switch.runtime.resumes).is_empty());

    // One row, in the same place, with the same pin.
    let view = &response.view;
    let rows = view
        .threads
        .iter()
        .map(|thread| thread.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(rows, [head.as_str()]);
    let row = thread(view, &head);
    assert_eq!(row.section, ThreadSection::Pinned);
    let old = before.record(THREAD).unwrap();
    assert_eq!(row.pin_order, old.pin_order);
    assert_eq!(row.workspace, old.workspace);
    let segments = &row.chain.as_ref().unwrap().segments;
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.key.as_str(), segment.order))
            .collect::<Vec<_>>(),
        [(THREAD, 0), (head.as_str(), 1)]
    );

    // The old head's live session was closed, after the commit.
    assert_eq!(*lock(&switch.harness.sessions.closes), [THREAD]);

    // The handoff: the journal's whole thread, as a host prompt.
    let sends = switch.sends_to(&head);
    assert_eq!(sends.len(), 1);
    let (text, echo) = &sends[0];
    assert!(echo, "shown in the pane as Interface's own prompt");
    assert!(text.starts_with(
        "You are taking over a thread from Claude in Interface; the owner switched it to Codex.\n\nThread: Title a\n"
    ));
    assert!(text.starts_with(HANDOFF_PREFIXES[0]));
    assert!(text.contains("user: fix the login\n\nassistant: Retries now back off."));
    assert!(text.ends_with(
        "Reply with one or two sentences on where things stand, then wait for the owner."
    ));
    let first = turn_ref(THREAD, 0, false);
    assert!(
        switch.harness.git.called(&format!("diff {first} worktree")),
        "the changes so far, from the thread's first checkpoint"
    );
    let document = switch.harness.document().await;
    let turns = &document.record(&head).unwrap().turns;
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].start_ref.as_deref(),
        Some(turn_ref(&head, 0, false).as_str())
    );
    assert!(document.record(THREAD).unwrap().segment_of.as_deref() == Some(head.as_str()));
}

/// Switching back to a provider the thread used before resumes its earlier
/// session (no new one) and tells it only what the others did since it last
/// worked, under "While you were away".
#[tokio::test]
async fn switching_back_resumes_the_earlier_session_with_only_what_it_missed() {
    let mut switch = switch().await;
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.harness.advance(HOUR);
    switch.finish(&head).await;
    switch.at = NOW + 1_000;
    let handoff = switch.sends_to(&head)[0].0.clone();
    switch.turn(&head, TurnOrigin::Host, &handoff, "Login retries are done.");
    switch.turn(
        &head,
        TurnOrigin::Owner,
        "add a retry test",
        "Added login.test.ts.",
    );
    lock(&switch.harness.sessions.closes).clear();

    let response = switch.to(&head, "claude").await.unwrap();
    assert_eq!(response.key.as_deref(), Some(THREAD));
    assert_eq!(response.handoff, Some(HandoffDelivery::Sent));
    assert_eq!(
        *lock(&switch.runtime.resumes),
        [THREAD],
        "resumed, not created"
    );
    assert_eq!(
        lock(&switch.runtime.creates).len(),
        1,
        "only the first switch created"
    );
    let sends = switch.sends_to(THREAD);
    assert_eq!(sends.len(), 1);
    let (delta, echo) = &sends[0];
    assert!(echo);
    assert!(delta.starts_with(
        "The owner switched this thread back to you (Claude) in Interface. While you were away, Codex continued this thread:"
    ));
    assert!(delta.starts_with(HANDOFF_PREFIXES[1]));
    assert!(delta.contains("user: add a retry test\n\nassistant: Added login.test.ts."));
    assert!(delta.contains("assistant: Login retries are done."));
    assert!(!delta.contains("fix the login"), "nothing it already knew");
    assert!(
        !delta.contains("You are taking over"),
        "never an earlier handoff"
    );
    assert!(delta.ends_with("then wait for the owner."));
    assert_eq!(*lock(&switch.harness.sessions.closes), [head.as_str()]);

    let document = switch.harness.document().await;
    let parts = document.chain_parts(THREAD);
    assert_eq!(
        parts
            .iter()
            .map(|part| (part.key.as_str(), part.order, part.head))
            .collect::<Vec<_>>(),
        [(head.as_str(), 1, false), (THREAD, 2, true)]
    );
}

/// Fail closed: a create or resume that fails leaves the store, the old
/// head's session and the ledger as they were, and sends nothing.
#[tokio::test]
async fn a_switch_whose_target_cannot_open_changes_nothing() {
    let switch = switch().await;
    *lock(&switch.runtime.create_fails) = Some(DispatchError::new(
        DEFINITELY_UNSENT_CODE,
        "Codex did not start.",
        false,
        true,
    ));
    let before = switch.harness.document().await;
    let error = switch.to(THREAD, "codex").await.unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert_eq!(switch.harness.document().await, before);
    assert!(lock(&switch.harness.sessions.closes).is_empty());
    assert!(lock(&switch.runtime.sends).is_empty());

    *lock(&switch.runtime.create_fails) = None;
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&head).await;
    *lock(&switch.runtime.resume_fails) = Some(DispatchError::new(
        "runtime-outcome-unknown",
        "Claude may have opened.",
        false,
        false,
    ));
    let before = switch.harness.document().await;
    let sends = lock(&switch.runtime.sends).len();
    let error = switch.to(&head, "claude").await.unwrap_err();
    assert!(!error.replay_safe, "an unknown resume is never retried");
    assert_eq!(switch.harness.document().await, before, "still on Codex");
    assert_eq!(lock(&switch.runtime.sends).len(), sends);
}

/// A handoff whose send may or may not have arrived leaves the switch
/// standing, says so, and is sent once only; one that definitely did not go
/// out says that instead.
#[tokio::test]
async fn an_unknown_handoff_stands_and_is_never_retried() {
    for (error, delivery) in [
        (
            DispatchError::new("runtime-outcome-unknown", "No answer.", false, false),
            HandoffDelivery::Unknown,
        ),
        (
            DispatchError::new(DEFINITELY_UNSENT_CODE, "Not sent.", false, true),
            HandoffDelivery::Unsent,
        ),
    ] {
        let switch = switch().await;
        *lock(&switch.runtime.send_fails) = Some(error);
        let response = switch.to(THREAD, "codex").await.unwrap();
        let head = response.key.unwrap();
        assert_eq!(response.handoff, Some(delivery));
        assert_eq!(lock(&switch.runtime.sends).len(), 1, "one attempt");
        let document = switch.harness.document().await;
        assert_eq!(
            document.head_of(THREAD),
            Some(head.as_str()),
            "the switch stands"
        );
    }
}

/// Every refusal is definitely unsent and changes nothing: the thread's own
/// provider, a running turn, a Code pane holding the session, an Away loop
/// holding it, a provider that is not ready, a mode the target does not
/// have, a plain chat, a worktree that is gone and an earlier segment.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One refusal after another.
async fn every_switch_refusal_is_definitely_unsent_and_changes_nothing() {
    let mut switch = switch().await;
    let before = switch.harness.document().await;
    let mut refusals = Vec::new();

    refusals.push((
        switch.to(THREAD, "claude").await,
        THREADS_SAME_PROVIDER_CODE,
    ));
    switch
        .harness
        .sessions
        .edit(THREAD, |row| row.status = SessionStatus::Working);
    refusals.push((switch.to(THREAD, "codex").await, THREADS_SESSION_BUSY_CODE));
    switch
        .harness
        .sessions
        .edit(THREAD, |row| row.status = SessionStatus::Idle);
    switch.harness.claims.hand_to_terminal(THREAD);
    refusals.push((switch.to(THREAD, "codex").await, THREADS_IN_TERMINAL_CODE));
    switch.harness.claims.release_handoff(THREAD);
    refusals.push((
        switch.to(THREAD, "acp:qwen").await,
        THREADS_SWITCH_NOT_READY_CODE,
    ));
    switch.access.remember(THREAD, Some("plan"));
    refusals.push((switch.to(THREAD, "codex").await, THREADS_SWITCH_MODE_CODE));
    switch.access.remember(THREAD, Some("acceptEdits"));
    for (result, code) in refusals.drain(..) {
        let error = result.unwrap_err();
        assert_eq!(error.code, code);
        assert!(error.replay_safe, "{code}");
    }
    assert_eq!(switch.harness.document().await, before);

    // An Away loop holds its thread.
    let away = away_thread(&switch.harness, "codex#away").await;
    assert!(!away.is_empty());
    let away_held = switch.to("codex#away", "claude").await.unwrap_err();
    assert_eq!(away_held.code, crate::threads::THREADS_AWAY_RUNNING_CODE);
    switch.harness.host.away_release("codex#away");
    // Away's setup refused the folder to prove its prepare is vetted.
    lock(&switch.harness.authority.0).clear();

    // A plain chat has no folder.
    switch
        .harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    switch
        .harness
        .command(json!({"kind": "bind", "key": "claude#chat", "prompt": "Hello"}))
        .await
        .unwrap();
    let plain = switch.to("claude#chat", "codex").await.unwrap_err();
    assert_eq!(plain.code, THREADS_SWITCH_PLAIN_CODE);

    // A worktree thread whose worktree is gone.
    let worktree = switch.harness.dir("wt");
    let root = switch.root.clone();
    switch
        .harness
        .git_thread("claude#w", &root, Some(&worktree), "in a worktree")
        .await;
    end_run(&switch.harness, "claude#w").await;
    std::fs::remove_dir_all(&worktree).unwrap();
    let gone = switch.to("claude#w", "codex").await.unwrap_err();
    assert_eq!(gone.code, THREADS_SWITCH_MISSING_CODE);

    // An earlier segment is switched from its thread, not itself.
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&head).await;
    let segment = switch.to(THREAD, "acp:grok").await.unwrap_err();
    assert_eq!(segment.code, THREADS_INVALID_CODE);
    assert_eq!(
        lock(&switch.runtime.creates).len(),
        1,
        "only the switch that worked"
    );
    for error in [away_held, plain, gone, segment] {
        assert!(error.replay_safe, "{}", error.code);
    }
}

/// A switched thread behaves as one thread: its history is every segment's
/// journal merged with "Switched to Codex" between; a search hit in the
/// earlier segment is the head's; its whole diff starts at the first
/// segment's first checkpoint; and Commit offers every segment's files.
#[tokio::test]
async fn a_switched_thread_reads_searches_diffs_and_ships_as_one_thread() {
    use crate::dispatcher::HistoryFallback;
    let mut switch = switch().await;
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&head).await;
    switch.at = NOW + 1_000;
    let handoff = switch.sends_to(&head)[0].0.clone();
    switch.turn(&head, TurnOrigin::Host, &handoff, "Where it stands.");
    switch.journal.sync_all().unwrap();

    let page = switch.harness.host.chain_history(&head, 500).await.unwrap();
    let lines = page
        .events
        .iter()
        .filter_map(|event| match &event.event {
            EngineEventKind::UserMessage { text, .. } => {
                Some(format!("user: {}", text.lines().next().unwrap()))
            }
            EngineEventKind::MessageDelta { text, .. } => Some(format!("assistant: {text}")),
            EngineEventKind::Notice { message, .. } => Some(format!("notice: {message}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "user: fix the login",
            "assistant: Retries now back off.",
            "notice: Switched to Codex",
            "user: You are taking over a thread from Claude in Interface; the owner switched it to Codex.",
            "assistant: Where it stands.",
        ]
    );
    assert!(
        switch
            .harness
            .host
            .chain_history(THREAD, 500)
            .await
            .is_none(),
        "a segment is never read as the thread"
    );

    let hits = switch
        .harness
        .host
        .dispatch(
            CHANNELS.thread_journal,
            json!({"kind": "search", "query": "back off"}),
        )
        .await
        .unwrap();
    assert_eq!(
        hits["hits"][0]["key"],
        head.as_str(),
        "the earlier segment's hit is the head's"
    );

    let diff = switch
        .harness
        .command(json!({"kind": "threadDiff", "key": head, "scope": "thread"}))
        .await
        .unwrap()
        .diff
        .unwrap();
    assert_eq!(
        diff.base,
        turn_ref(THREAD, 0, false),
        "from the first segment's first checkpoint"
    );

    let document = switch.harness.document().await;
    assert!(document.ship_scope(&head).unwrap().prompts == ["fix the login"]);
}

/// A thread switched back to its first provider is still diffed whole from
/// its earliest checkpoint: switching back makes the first segment the head
/// again (last in the chain's activation order), and the Changes panel's
/// Whole thread and the next handoff's "Changes so far" must still include
/// what that provider did before the first switch.
#[tokio::test]
async fn a_thread_switched_back_is_diffed_from_its_earliest_checkpoint() {
    let switch = switch().await;
    let codex = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&codex).await;
    let back = switch.to(&codex, "claude").await.unwrap().key.unwrap();
    assert_eq!(back, THREAD);
    switch.finish(THREAD).await;
    let diff = switch
        .harness
        .command(json!({"kind": "threadDiff", "key": THREAD, "scope": "thread"}))
        .await
        .unwrap()
        .diff
        .unwrap();
    assert_eq!(
        diff.base,
        turn_ref(THREAD, 0, false),
        "from Claude's first turn, before the first switch"
    );
}

/// Deleting or forgetting a switched thread's head takes its earlier
/// segments' sessions with it (their journals go through the same removal
/// as the head's); archiving or restoring the head archives or restores them.
#[tokio::test]
async fn removing_or_archiving_the_head_takes_its_segments_along() {
    use crate::session_management::RemovalHooks;
    let switch = switch().await;
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&head).await;

    switch
        .harness
        .sessions
        .edit(&head, |row| row.archived = true);
    switch.harness.host.reconcile().await;
    settle_tasks(&switch.harness.host).await;
    assert_eq!(
        *lock(&switch.harness.sessions.archives),
        [(THREAD.to_owned(), true)]
    );
    switch
        .harness
        .sessions
        .edit(&head, |row| row.archived = false);
    switch.harness.host.reconcile().await;
    settle_tasks(&switch.harness.host).await;
    assert_eq!(
        lock(&switch.harness.sessions.archives).last(),
        Some(&(THREAD.to_owned(), false))
    );

    switch.harness.host.session_removed_as(&head, false);
    settle_tasks(&switch.harness.host).await;
    assert_eq!(
        *lock(&switch.harness.sessions.removals),
        [(THREAD.to_owned(), false)],
        "forgotten as its head was"
    );
}

/// Team tools follow the head: a started thread switched to another
/// provider is still its caller's (and still one hop down, so its new
/// session has no team tools), and the caller's wait for the old key
/// reaches the head.
#[tokio::test]
async fn the_team_ledger_follows_a_switched_thread_to_its_head() {
    let switch = switch().await;
    let caller = "codex#caller";
    switch
        .harness
        .sessions
        .upsert(row(caller, SessionKind::Code));
    let entry = interface_settings::thread_team::StartedThread {
        parent: caller.into(),
        child: THREAD.into(),
        started_at: NOW,
        notify: true,
        delivered: None,
        started_by: None,
    };
    switch
        .harness
        .host
        .team_store(move |store| store.record_start(entry))
        .await
        .unwrap()
        .unwrap();
    switch.harness.host.mark_team_child(THREAD);

    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    let creates = lock(&switch.runtime.creates).clone();
    assert_eq!(
        creates[0]["options"]["collaborationDepth"], 1,
        "still one hop down"
    );
    assert!(switch.harness.host.is_team_child(&head));
    assert!(!switch.harness.host.is_team_child(THREAD));
    let owned = head.clone();
    let parent = switch
        .harness
        .host
        .team_store(move |store| store.started(&owned).map(|entry| entry.parent.clone()))
        .await
        .unwrap();
    assert_eq!(parent.as_deref(), Some(caller));
    let owned = head.clone();
    let delivered = switch
        .harness
        .host
        .team_store(move |store| store.started(&owned).and_then(|entry| entry.delivered))
        .await
        .unwrap()
        .map(|delivered| (delivered.turn, delivered.how));
    assert_eq!(
        delivered,
        Some((0, interface_settings::thread_team::DeliveryHow::Dropped)),
        "the caller is not told about the handoff turn"
    );
    assert_eq!(switch.harness.host.head_key(THREAD).await, head);
}

/// An ACP agent refuses an empty launch, so a switch to one starts its
/// session with the handoff (turn 0, its run open) and answers it as the
/// prompt the pane shows; nothing is sent after.
#[tokio::test]
async fn a_switch_to_an_acp_provider_launches_with_its_handoff() {
    let switch = switch().await;
    let response = switch.to(THREAD, "acp:grok").await.unwrap();
    let head = response.key.clone().unwrap();
    let creates = lock(&switch.runtime.creates).clone();
    let launched = creates[0]["options"]["prompt"].as_str().unwrap().to_owned();
    assert!(launched.starts_with(HANDOFF_PREFIXES[0]));
    assert_eq!(
        creates[0]["options"]["permissionMode"], "default",
        "Grok has no edits rung: the nearest lower one"
    );
    assert_eq!(response.prompt.as_deref(), Some(launched.as_str()));
    assert_eq!(response.handoff, Some(HandoffDelivery::Sent));
    assert!(switch.sends_to(&head).is_empty(), "it rode the launch");
    let document = switch.harness.document().await;
    let record = document.record(&head).unwrap();
    assert_eq!(record.turns.len(), 1);
    assert!(record.running(), "its launch run is open");
    assert!(switch.harness.host.run_tracked(&head));
}

/// Protocol 69: the handoff an ACP agent takes at launch is measured like
/// any other turn. Its checkpoint is taken before the session starts (the
/// agent works on the handoff at once) and filed as turn 0's start, so the
/// turn's line counts join the thread's totals and the Changes panel's
/// Whole thread agrees with them.
#[tokio::test]
async fn an_acp_launch_handoff_starts_on_a_checkpoint_taken_before_the_agent() {
    let switch = switch().await;
    // A launch that fails changes nothing and leaves no checkpoint behind.
    *lock(&switch.runtime.create_fails) = Some(DispatchError::new(
        DEFINITELY_UNSENT_CODE,
        "Grok did not start.",
        false,
        true,
    ));
    let before = switch.harness.document().await;
    switch.to(THREAD, "acp:grok").await.unwrap_err();
    assert_eq!(switch.harness.document().await, before);
    let calls = switch.harness.git.calls();
    let pending = calls
        .iter()
        .find_map(|call| {
            call.strip_prefix("capture ")
                .filter(|name| name.contains("/pending/switch-"))
        })
        .expect("a checkpoint before the launch");
    assert!(switch.harness.git.called(&format!("delete {pending}")));
    *lock(&switch.runtime.create_fails) = None;
    let response = switch.to(THREAD, "acp:grok").await.unwrap();
    let head = response.key.clone().unwrap();
    let calls = switch.harness.git.calls();
    let captured = calls
        .iter()
        .position(|call| call.starts_with("capture ") && call.contains("/pending/switch-"))
        .expect("a checkpoint before the launch");
    let moved = calls
        .iter()
        .position(|call| call.starts_with("move ") && call.ends_with(&turn_ref(&head, 0, false)))
        .expect("filed under the new head's turn 0");
    assert!(captured < moved);
    let document = switch.harness.document().await;
    assert_eq!(
        document.record(&head).unwrap().turns[0]
            .start_ref
            .as_deref(),
        Some(turn_ref(&head, 0, false).as_str())
    );
}

/// T3 section 9: a thread's stashed drafts follow its head when its
/// provider is switched, so the composer that now shows the thread lists
/// them.
#[tokio::test]
async fn stashed_drafts_follow_a_switched_thread_to_its_head() {
    let switch = switch().await;
    let store = interface_settings::draft_stash::DraftStashStore::open(
        &switch
            .harness
            .state
            .path()
            .join(interface_settings::draft_stash::DRAFT_STASH_FILE_NAME),
    )
    .map_err(|error| error.to_string());
    switch.harness.host.attach_stash(store);
    switch
        .harness
        .host
        .dispatch(
            CHANNELS.draft_stash,
            json!({"command": "push", "key": THREAD, "draft": {"text": "parked before the switch"}}),
        )
        .await
        .unwrap();
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    let list = |key: &str| {
        let key = key.to_owned();
        let host = switch.harness.host.clone();
        async move {
            host.dispatch(CHANNELS.draft_stash, json!({"command": "list", "key": key}))
                .await
                .unwrap()
        }
    };
    let at_head = list(&head).await;
    assert_eq!(at_head["key"], head.as_str());
    assert_eq!(at_head["stashes"][0]["preview"], "parked before the switch");
    // The earlier segment's key reaches the head and its stashes.
    assert_eq!(list(THREAD).await, at_head);
}

/// Protocol 67: a switched thread keeps its secret request on its head's
/// row, whichever of its sessions asked. When the run of the session that
/// asked ends (here an earlier segment's, whose key is not the head's), that
/// request is withdrawn: answered declined, and the card goes.
#[tokio::test]
async fn a_switched_threads_secret_is_withdrawn_when_the_asking_sessions_run_ends() {
    let switch = switch().await;
    let host = switch.harness.host.clone();
    host.attach_secrets(switch.harness.state.path());
    let head = switch.to(THREAD, "codex").await.unwrap().key.unwrap();
    switch.finish(&head).await;
    assert_ne!(head, THREAD);
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                THREAD,
                "request_secret",
                json!({"name": "SECRET_KEY", "reason": "To deploy."}),
            )
            .await
        }
    });
    host.observe_run(THREAD, &super::team::secret_tool_call("toolu_segment"));
    let mut opened = false;
    for _ in 0..5_000 {
        let view = host.view().await.unwrap();
        if view
            .threads
            .iter()
            .any(|thread| thread.key == head && thread.secret_request.is_some())
        {
            opened = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(opened, "the request shows on the head's row");
    host.observe_run(
        THREAD,
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), asked)
        .await
        .expect("the request still waits after the asking session's run ended");
    let declined: Value = serde_json::from_str(&answered.unwrap().unwrap()).unwrap();
    assert_eq!(declined, json!({"declined": true}));
    let view = host.view().await.unwrap();
    assert!(
        view.threads
            .iter()
            .all(|thread| thread.secret_request.is_none())
    );
}
