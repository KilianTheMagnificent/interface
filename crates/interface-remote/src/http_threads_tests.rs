use super::*;
use interface_contracts::remote_threads::{RemoteThreadStatus, RemoteThreadV1};
use interface_contracts::threads::ThreadSection;
use std::sync::Mutex as StdMutex;

/// A Mac with one terminal that needs you and one thread parked on approval
/// `a1`. Each answer reports what the agent would have seen.
struct Backend {
    terminal: interface_contracts::terminal::TerminalSummary,
    answers: StdMutex<Vec<(String, String, bool)>>,
    sends: StdMutex<Vec<(String, String)>>,
    outcome: StdMutex<Result<(), RemoteError>>,
}

fn listed() -> RemoteThreadV1 {
    RemoteThreadV1 {
        key: "claude#thread-1".into(),
        engine: "claude".into(),
        session_id: "thread-1".into(),
        title: "Migrate the store".into(),
        project: Some("/Projects/app".into()),
        status: RemoteThreadStatus::NeedsYou,
        section: ThreadSection::Active,
        unread: true,
        last_activity: "2026-10-14T09:00:00.000Z".into(),
    }
}

impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(TerminalSnapshot {
            revision: 1,
            terminals: vec![self.terminal.clone()],
        })
    }
    fn enqueue(&self, _: StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
        Ok(())
    }
    fn threads(&self) -> ThreadsFuture<'_> {
        Box::pin(async {
            Ok(RemoteThreadsV1 {
                version: 1,
                threads: vec![listed()],
            })
        })
    }
    fn inbox(&self) -> InboxFuture<'_> {
        Box::pin(async {
            Ok(serde_json::from_str(include_str!(
                "../../../electron/src/fixtures/remote-inbox-v2.json"
            ))
            .unwrap())
        })
    }
    fn thread_detail(&self, key: String) -> ThreadDetailFuture<'_> {
        Box::pin(async move {
            assert_eq!(key, "claude#thread-1");
            Ok(RemoteThreadDetailV1 {
                version: 1,
                thread: listed(),
                turns: Vec::new(),
                older_turns: 0,
                approval: None,
                unavailable: None,
            })
        })
    }
    fn answer_thread(
        &self,
        key: String,
        approval_id: String,
        allow: bool,
    ) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            self.answers.lock().unwrap().push((key, approval_id, allow));
            self.outcome.lock().unwrap().clone()
        })
    }
    fn send_to_thread(&self, key: String, text: String) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            self.sends.lock().unwrap().push((key, text));
            self.outcome.lock().unwrap().clone()
        })
    }
}

fn harness() -> (
    Arc<RemoteHub>,
    Arc<Backend>,
    Arc<HttpState>,
    HeaderMap,
    Uuid,
) {
    let hub = Arc::new(RemoteHub::default());
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let grant = hub.enroll(&hub.pairing_code().unwrap(), "Phone").unwrap();
    let device = hub.authenticate(&grant).unwrap();
    let mut terminal = crate::workspaces::tests::terminal();
    terminal.attention = Some(interface_contracts::terminal::TerminalAttentionV1 {
        version: 1,
        since: "2026-10-14T09:12:00.000Z".into(),
    });
    let backend = Arc::new(Backend {
        terminal,
        answers: StdMutex::new(Vec::new()),
        sends: StdMutex::new(Vec::new()),
        outcome: StdMutex::new(Ok(())),
    });
    let state = Arc::new(HttpState {
        hub: hub.clone(),
        backend: backend.clone(),
        assets: BTreeMap::new(),
        streams: Arc::new(Semaphore::new(8)),
        requests: Semaphore::new(32),
        histories: Semaphore::new(2),
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("interface_remote={grant}").parse().unwrap(),
    );
    (hub, backend, state, headers, device)
}

fn answer(decision: RemoteThreadDecisionV1) -> Json<RemoteThreadApprovalResponseV1> {
    Json(RemoteThreadApprovalResponseV1 {
        version: 1,
        approval_id: "a1".into(),
        decision,
    })
}

fn thread_path() -> Path<(String, String)> {
    Path(("claude".into(), "thread-1".into()))
}

#[tokio::test]
async fn shared_agent_inbox_requires_a_current_grant_and_keeps_sessionless_tasks_read_only() {
    let (hub, _, state, headers, device) = harness();
    let listed = inbox(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(listed.version, 2);
    assert!(listed.items[1].thread.is_none());
    assert!(!listed.items[2].can_send);
    assert_eq!(
        inbox(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    hub.revoke(device).unwrap();
    assert_eq!(
        inbox(State(state), headers).await.unwrap_err().code,
        "unauthorized"
    );
}

#[tokio::test]
async fn only_roster_v4_and_threads_say_what_needs_you_and_only_behind_the_grant() {
    let (_, _, state, headers, _) = harness();
    let v4 = workspaces_v4(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(v4.version, 4);
    assert!(v4.workspaces[0].terminals[0].attention.is_some());
    let v3 = workspaces_v3(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert!(v3.workspaces[0].terminals[0].attention.is_none());
    let v2 = workspaces(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert!(v2.workspaces[0].terminals[0].attention.is_none());
    let v1 = terminals(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert!(v1.terminals[0].attention.is_none());
    let list = threads(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(list.threads[0].status, RemoteThreadStatus::NeedsYou);
    assert!(
        workspaces_v4(State(state.clone()), HeaderMap::new())
            .await
            .is_err()
    );
    assert_eq!(
        threads(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(
        thread_detail(State(state.clone()), thread_path(), headers.clone())
            .await
            .is_ok()
    );
    for (engine, session) in [
        ("claude", "a.b"),
        ("Claude", "a"),
        ("claude", ""),
        ("", "a"),
        ("claude", &"x".repeat(129)),
    ] {
        let refused = thread_detail(
            State(state.clone()),
            Path((engine.to_owned(), session.to_owned())),
            headers.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, "invalid-thread", "{engine} {session}");
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One ordered lease and receipt lifecycle.
async fn an_approval_answer_needs_the_lease_runs_once_and_never_repeats_an_unknown() {
    let (hub, backend, state, mut headers, device) = harness();
    let send = |headers: HeaderMap, decision| {
        answer_thread(
            State(state.clone()),
            thread_path(),
            headers,
            answer(decision),
        )
    };
    // No lease, no command: refused before anything runs.
    assert!(
        send(headers.clone(), RemoteThreadDecisionV1::AllowOnce)
            .await
            .is_err()
    );
    headers.insert(
        "x-interface-control",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    assert_eq!(
        send(headers.clone(), RemoteThreadDecisionV1::AllowOnce)
            .await
            .unwrap_err()
            .code,
        "control-lost"
    );
    let lease = claim_thread(State(state.clone()), thread_path(), headers.clone())
        .await
        .unwrap()
        .0["lease"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        lease,
        hub.claim_thread(device, "claude#thread-1")
            .unwrap()
            .to_string()
    );
    headers.insert("x-interface-control", lease.parse().unwrap());
    assert_eq!(
        send(headers.clone(), RemoteThreadDecisionV1::AllowOnce)
            .await
            .unwrap(),
        StatusCode::ACCEPTED
    );
    // A repeat of the same command answers its receipt; nothing runs again.
    assert_eq!(
        send(headers.clone(), RemoteThreadDecisionV1::AllowOnce)
            .await
            .unwrap(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        backend.answers.lock().unwrap().as_slice(),
        [("claude#thread-1".to_owned(), "a1".to_owned(), true)]
    );
    assert_eq!(
        send(headers.clone(), RemoteThreadDecisionV1::Deny)
            .await
            .unwrap_err()
            .code,
        "command-conflict"
    );
    // An answer whose delivery is unknown is never run again under its id.
    *backend.outcome.lock().unwrap() = Err(RemoteError::new("outcome-unknown", "Unknown.", false));
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    let unknown = send(headers.clone(), RemoteThreadDecisionV1::Deny)
        .await
        .unwrap_err();
    assert!(!unknown.replay_safe);
    *backend.outcome.lock().unwrap() = Ok(());
    let repeat = send(headers.clone(), RemoteThreadDecisionV1::Deny)
        .await
        .unwrap_err();
    assert_eq!(repeat.code, "outcome-unknown");
    assert_eq!(backend.answers.lock().unwrap().len(), 2);
    // Releasing the lease ends this phone's control of the thread.
    release_thread(State(state.clone()), thread_path(), headers.clone())
        .await
        .unwrap();
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    assert_eq!(
        send(headers.clone(), RemoteThreadDecisionV1::AllowOnce)
            .await
            .unwrap_err()
            .code,
        "control-lost"
    );
    let mut outdated = answer(RemoteThreadDecisionV1::AllowOnce);
    outdated.version = 2;
    assert_eq!(
        answer_thread(State(state.clone()), thread_path(), headers, outdated)
            .await
            .unwrap_err()
            .code,
        "invalid-version"
    );
    assert_eq!(backend.answers.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_follow_up_is_sent_once_under_the_lease_and_never_mistaken_for_an_answer() {
    let (_, backend, state, mut headers, _) = harness();
    let message = |text: &str| {
        Json(RemoteThreadSendV1 {
            version: 1,
            text: text.into(),
        })
    };
    let send = |headers: HeaderMap, text: &str| {
        send_to_thread(State(state.clone()), thread_path(), headers, message(text))
    };
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    headers.insert(
        "x-interface-control",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    assert_eq!(
        send(headers.clone(), "Run the tests again.")
            .await
            .unwrap_err()
            .code,
        "control-lost"
    );
    let lease = claim_thread(State(state.clone()), thread_path(), headers.clone())
        .await
        .unwrap()
        .0["lease"]
        .as_str()
        .unwrap()
        .to_owned();
    headers.insert("x-interface-control", lease.parse().unwrap());
    for refused in ["", "   ", &"x".repeat(REMOTE_THREAD_SEND_MAX_BYTES + 1)] {
        assert_eq!(
            send(headers.clone(), refused).await.unwrap_err().code,
            "input-size"
        );
    }
    assert_eq!(
        send(headers.clone(), "Run the tests again.").await.unwrap(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(headers.clone(), "Run the tests again.").await.unwrap(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        backend.sends.lock().unwrap().as_slice(),
        [(
            "claude#thread-1".to_owned(),
            "Run the tests again.".to_owned()
        )]
    );
    // One command id is one effect: an answer cannot reuse a message's id.
    assert_eq!(
        answer_thread(
            State(state.clone()),
            thread_path(),
            headers.clone(),
            answer(RemoteThreadDecisionV1::AllowOnce)
        )
        .await
        .unwrap_err()
        .code,
        "command-conflict"
    );
    assert!(backend.answers.lock().unwrap().is_empty());
    let mut outdated = message("hello");
    outdated.version = 2;
    assert_eq!(
        send_to_thread(State(state.clone()), thread_path(), headers, outdated)
            .await
            .unwrap_err()
            .code,
        "invalid-version"
    );
}
