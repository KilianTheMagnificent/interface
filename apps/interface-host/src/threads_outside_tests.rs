//! Outside agents (T3 section 11, protocol 65) through the thread host with
//! the team tools' fakes, and the real outside socket.

use interface_contracts::threads::OutsideAgents;
use interface_settings::thread_team::{self as rules, StartedBy, StartedByKind, StartedThread};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use super::*;

const SESSION: &str = "0a1b2c3d4e5f";
const OTHER: &str = "ffeeddccbbaa";
const CLIENT: &str = "claude-code";

impl Team {
    async fn outside(&self, session: &str, name: &str, input: Value) -> Result<Value, Value> {
        self.harness
            .host
            .outside_call(session, CLIENT, name, input)
            .await
            .map(|text| serde_json::from_str(&text).unwrap())
            .map_err(|text| serde_json::from_str(&text).unwrap())
    }

    /// The owner chooses `level` in Settings, as the renderer sends it.
    async fn set_level(&self, level: OutsideAgents) -> ThreadsView {
        let mut settings = self.harness.document().await.settings.clone();
        settings.outside_agents = level;
        self.harness
            .command(json!({"kind": "setSettings", "settings": settings}))
            .await
            .unwrap()
            .view
    }
}

/// A team host with a second project, `beacon`, holding one more thread.
async fn outside_team() -> (Team, String) {
    let mut team = team().await;
    let beacon = team.harness.dir("beacon");
    team.harness
        .git_thread("claude#beacon", &beacon, None, "beacon work")
        .await;
    end_record_run(&team.harness, "claude#beacon").await;
    (team, beacon)
}

/// Off (the default) refuses every call; Read reads but changes nothing;
/// Full starts. The level is the owner's, from Settings, kept in the team
/// ledger and never in the Threads store; a view carries it.
#[tokio::test]
async fn outside_levels_off_read_and_full_gate_every_call() {
    let (team, _) = outside_team().await;
    assert_eq!(team.harness.host.outside_level(), OutsideAgents::Off);
    for (name, input) in [
        ("list_projects", json!({})),
        ("list_threads", json!({})),
        ("thread_status", json!({"thread": CALLER})),
        ("start_thread", json!({"project": "atlas", "task": "x"})),
    ] {
        let refused = team.outside(SESSION, name, input).await.unwrap_err();
        assert_eq!(refused["code"], rules::OUTSIDE_OFF, "{name}");
    }

    let view = team.set_level(OutsideAgents::Read).await;
    assert_eq!(view.settings.outside_agents, OutsideAgents::Read);
    let listed = team
        .outside(SESSION, "list_threads", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["threads"].as_array().unwrap().len(), 2);
    for (name, input) in [
        ("start_thread", json!({"project": "atlas", "task": "x"})),
        ("send_to_thread", json!({"thread": CALLER, "message": "hi"})),
        ("stop_thread", json!({"thread": CALLER})),
    ] {
        let refused = team.outside(SESSION, name, input).await.unwrap_err();
        assert_eq!(refused["code"], rules::READ_ONLY, "{name}");
    }
    assert!(
        lock(&team.runtime.creates).is_empty(),
        "read changes nothing"
    );
    assert!(lock(&team.runtime.sends).is_empty());
    assert!(lock(&team.runtime.interrupts).is_empty());

    team.set_level(OutsideAgents::Full).await;
    team.outside(
        SESSION,
        "start_thread",
        json!({"project": "atlas", "task": "count the tests"}),
    )
    .await
    .unwrap();
    // The Threads store never keeps the level; the ledger does.
    assert_eq!(
        team.harness.document().await.settings.outside_agents,
        OutsideAgents::Off
    );
    let threads_file = std::fs::read_to_string(
        team.harness
            .state
            .path()
            .join(interface_settings::threads::THREADS_FILE_NAME),
    )
    .unwrap();
    assert!(!threads_file.contains("outsideAgents"));
    let ledger: Value = serde_json::from_slice(
        &std::fs::read(team.harness.state.path().join(rules::THREAD_TEAM_FILE_NAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["outsideAgents"], "full", "{ledger}");
    // Turning it off takes effect at once, for a session already connected.
    team.set_level(OutsideAgents::Off).await;
    let refused = team
        .outside(SESSION, "list_threads", json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::OUTSIDE_OFF);
}

/// An outside session reads every project's threads (never one without a
/// project, a hidden worker or an agent's), filters by project, and sends to
/// or stops only threads it started itself.
#[tokio::test]
async fn an_outside_agent_reads_every_project_and_controls_only_what_it_started() {
    let (team, beacon) = outside_team().await;
    team.set_level(OutsideAgents::Full).await;
    let projects = team
        .outside(SESSION, "list_projects", json!({}))
        .await
        .unwrap();
    assert_eq!(
        projects["projects"],
        json!([
            {"name": "atlas", "project": team.root, "threads": 1},
            {"name": "beacon", "project": beacon, "threads": 1},
        ])
    );
    let listed = team
        .outside(SESSION, "list_threads", json!({"project": beacon}))
        .await
        .unwrap();
    let rows = listed["threads"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["thread"], "claude#beacon");
    assert_eq!(rows[0]["project"], beacon);
    assert_eq!(rows[0]["startedByYou"], false);
    // A plain chat (no project) is nobody's to read.
    team.harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Code));
    team.harness.host.reconcile().await;
    let refused = team
        .outside(SESSION, "read_thread", json!({"thread": "claude#chat"}))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::NOT_FOUND);
    let read = team
        .outside(SESSION, "read_thread", json!({"thread": CALLER}))
        .await
        .unwrap();
    assert_eq!(read["thread"], CALLER);
    // Control is for threads this session started only.
    let refused = team
        .outside(
            SESSION,
            "send_to_thread",
            json!({"thread": CALLER, "message": "hi"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::NOT_STARTED_BY_YOU);
    let started = team
        .outside(
            SESSION,
            "start_thread",
            json!({"project": "beacon", "task": "look around"}),
        )
        .await
        .unwrap();
    let child = started["thread"].as_str().unwrap().to_owned();
    let other = team
        .outside(OTHER, "stop_thread", json!({"thread": child}))
        .await
        .unwrap_err();
    assert_eq!(
        other["code"],
        rules::NOT_STARTED_BY_YOU,
        "another session's"
    );
    let mine = team
        .outside(SESSION, "list_threads", json!({}))
        .await
        .unwrap();
    assert!(
        mine["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["thread"] == child.as_str() && row["startedByYou"] == true)
    );
    team.outside(SESSION, "stop_thread", json!({"thread": child}))
        .await
        .unwrap();
    assert_eq!(*lock(&team.runtime.interrupts), [child]);
}

/// `list_threads`' `project` is "a root or name from list_projects", as
/// `start_thread`'s is: a project's name keeps that project's threads, and a
/// name that is no project says so rather than answering an empty list.
#[tokio::test]
async fn an_outside_list_filters_by_a_project_name_as_well_as_its_root() {
    let (team, beacon) = outside_team().await;
    team.set_level(OutsideAgents::Read).await;
    let by_root = team
        .outside(SESSION, "list_threads", json!({"project": beacon}))
        .await
        .unwrap();
    let by_name = team
        .outside(SESSION, "list_threads", json!({"project": "beacon"}))
        .await
        .unwrap();
    assert_eq!(by_name, by_root, "the name lists what the root lists");
    assert_eq!(by_name["threads"][0]["thread"], "claude#beacon");
    let refused = team
        .outside(SESSION, "list_threads", json!({"project": "nowhere"}))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::UNKNOWN_PROJECT, "{refused}");
}

/// A start names an existing Interface project and nothing else; the thread
/// runs at the target's asking rung, at collaboration depth 1 (no team
/// tools of its own), on the default account, with no lineage; the ledger
/// and the view say an outside agent started it and which client.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One start's whole contract.
async fn an_outside_start_runs_in_a_known_project_at_the_asking_rung_without_team_tools() {
    let (team, beacon) = outside_team().await;
    team.set_level(OutsideAgents::Full).await;
    for project in [
        "/tmp".to_owned(),
        format!("{}/src", team.root),
        team.harness.dir("unknown"),
        "atlas/..".into(),
    ] {
        let refused = team
            .outside(
                SESSION,
                "start_thread",
                json!({"project": project, "task": "x"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused["code"], rules::UNKNOWN_PROJECT, "{project}");
    }
    // No input names a mode, a caller or a folder of its own.
    for extra in [
        json!({"permissionMode": "bypassPermissions"}),
        json!({"cwd": "/tmp"}),
        json!({"caller": "claude#a"}),
        json!({"workspace": "same"}),
        json!({"notify": true}),
    ] {
        let mut input = json!({"project": "atlas", "task": "x"});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let refused = team
            .outside(SESSION, "start_thread", input)
            .await
            .unwrap_err();
        assert_eq!(refused["code"], rules::INVALID_INPUT, "{extra}");
    }
    assert!(lock(&team.runtime.creates).is_empty(), "nothing was made");

    let started = team
        .outside(
            SESSION,
            "start_thread",
            json!({"project": team.root, "task": "append notes.txt hi", "provider": "codex"}),
        )
        .await
        .unwrap();
    let child = started["thread"].as_str().unwrap().to_owned();
    assert_eq!(started["status"], "running");
    let create = lock(&team.runtime.creates)[0].clone();
    assert_eq!(create["engine"], "codex");
    assert_eq!(
        create["options"]["permissionMode"], "untrusted",
        "Codex asks"
    );
    assert_eq!(create["options"]["collaborationDepth"], 1);
    assert_eq!(create["options"]["kind"], "code");
    assert!(
        create["options"].get("account").is_none(),
        "default account"
    );
    assert!(
        team.harness
            .git
            .calls()
            .contains(&format!("prepare {}", team.root)),
        "{:?}",
        team.harness.git.calls()
    );
    let document = team.harness.document().await;
    let record = document.record(&child).unwrap();
    assert!(record.lineage.is_none());
    assert_eq!(record.turns.len(), 1, "turn 0 is the task");
    assert_eq!(
        team.sends_to(&child),
        [("append notes.txt hi".to_owned(), true)]
    );
    assert!(
        team.harness.host.is_team_child(&child),
        "resumes one hop down"
    );
    let nested = team.call(&child, "team_info", json!({})).await.unwrap_err();
    assert_eq!(nested["code"], "no-nesting");
    let entry = team
        .harness
        .host
        .team_store({
            let child = child.clone();
            move |store| store.started(&child).cloned()
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.parent, rules::outside_caller(SESSION));
    assert!(!entry.notify, "nobody inside Interface waits for it");
    assert_eq!(
        entry.started_by,
        Some(StartedBy {
            kind: StartedByKind::Outside,
            client: CLIENT.into(),
        })
    );
    let view = team.harness.host.view().await.unwrap();
    let shown = view
        .threads
        .iter()
        .find(|thread| thread.key == child)
        .unwrap();
    assert_eq!(shown.started_from_outside.as_deref(), Some(CLIENT));
    assert!(
        view.threads
            .iter()
            .filter(|thread| thread.key != child)
            .all(|thread| thread.started_from_outside.is_none())
    );

    // Claude's asking mode is `default`; a project may be named.
    team.outside(
        SESSION,
        "start_thread",
        json!({"project": "beacon", "task": "x", "workspace": "worktree"}),
    )
    .await
    .unwrap();
    let create = lock(&team.runtime.creates)[1].clone();
    assert_eq!(create["engine"], "claude");
    assert_eq!(create["options"]["permissionMode"], "default");
    assert!(
        team.harness
            .git
            .calls()
            .contains(&format!("prepare {beacon}"))
    );
    // An engine with no asking mode is refused before anything is made.
    let refused = team
        .outside(
            SESSION,
            "start_thread",
            json!({"project": "atlas", "task": "x", "provider": "gemini"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::INVALID_INPUT);
    assert_eq!(lock(&team.runtime.creates).len(), 2);
}

/// Limits hold per session (4 running, 16 in all) and per day across
/// sessions (64); a retried start with the same requestId answers the first
/// start without making another.
#[tokio::test]
async fn outside_limits_and_receipts_hold() {
    let (mut team, _) = outside_team().await;
    team.set_level(OutsideAgents::Full).await;
    let input = json!({"project": "atlas", "task": "job", "requestId": "start-1"});
    let first = team
        .outside(SESSION, "start_thread", input.clone())
        .await
        .unwrap();
    assert_eq!(
        team.outside(SESSION, "start_thread", input).await.unwrap(),
        first
    );
    assert_eq!(lock(&team.runtime.creates).len(), 1, "one start");
    let mut children = vec![first["thread"].as_str().unwrap().to_owned()];
    for index in 1..4 {
        let started = team
            .outside(
                SESSION,
                "start_thread",
                json!({"project": "atlas", "task": format!("job {index}")}),
            )
            .await
            .unwrap();
        children.push(started["thread"].as_str().unwrap().to_owned());
    }
    let running = team
        .outside(
            SESSION,
            "start_thread",
            json!({"project": "atlas", "task": "five"}),
        )
        .await
        .unwrap_err();
    assert_eq!(running["code"], rules::LIMIT_RUNNING);
    // Another session has its own four.
    team.outside(
        OTHER,
        "start_thread",
        json!({"project": "atlas", "task": "other"}),
    )
    .await
    .unwrap();
    for child in &children {
        team.finish(child, "ok", RunOutcome::Completed).await;
    }
    for index in 4..16 {
        let started = team
            .outside(
                SESSION,
                "start_thread",
                json!({"project": "atlas", "task": format!("job {index}")}),
            )
            .await
            .unwrap();
        let child = started["thread"].as_str().unwrap().to_owned();
        team.finish(&child, "ok", RunOutcome::Completed).await;
    }
    let total = team
        .outside(
            SESSION,
            "start_thread",
            json!({"project": "atlas", "task": "seventeen"}),
        )
        .await
        .unwrap_err();
    assert_eq!(total["code"], rules::LIMIT_TOTAL);
    // The day's limit counts every session: 17 so far, fill to 64.
    let now = crate::threads::now_ms();
    team.harness
        .host
        .team_store(move |store| {
            for index in 0..(rules::MAX_OUTSIDE_STARTED_PER_DAY - 17) {
                store
                    .record_start(StartedThread {
                        parent: rules::outside_caller("dddddddddddd"),
                        child: format!("claude#filled-{index}"),
                        started_at: now,
                        notify: false,
                        delivered: None,
                        started_by: Some(StartedBy {
                            kind: StartedByKind::Outside,
                            client: "filler".into(),
                        }),
                    })
                    .unwrap();
            }
        })
        .await
        .unwrap();
    let day = team
        .outside(
            "123456789abc",
            "start_thread",
            json!({"project": "atlas", "task": "x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(day["code"], rules::LIMIT_DAY);
}

/// A start's `requestId` holds across outside sessions: after a lost
/// connection (`outcome-unknown`, which tells the agent to retry with the
/// same id) the retry comes from a new session and answers the thread
/// already started instead of starting another. Control stays the first
/// session's, and the answer says so; a replay is no new start for any
/// limit; the same id for a different call is still `request-reused`.
#[tokio::test]
async fn an_outside_start_retried_from_a_new_session_answers_the_first_thread() {
    let (team, _) = outside_team().await;
    team.set_level(OutsideAgents::Full).await;
    let input = json!({"project": "atlas", "task": "job", "requestId": "again-1"});
    let first = team
        .outside(SESSION, "start_thread", input.clone())
        .await
        .unwrap();
    let child = first["thread"].as_str().unwrap().to_owned();
    // The first session's connection is gone; the retry is a new session.
    let retried = team
        .outside(OTHER, "start_thread", input.clone())
        .await
        .unwrap();
    assert_eq!(lock(&team.runtime.creates).len(), 1, "one thread, not two");
    assert_eq!(retried["thread"], child.as_str(), "{retried}");
    assert_eq!(retried["startedByYou"], false, "{retried}");
    let refused = team
        .outside(OTHER, "stop_thread", json!({"thread": child}))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], rules::NOT_STARTED_BY_YOU);
    let other_call = team
        .outside(
            OTHER,
            "start_thread",
            json!({"project": "atlas", "task": "another job", "requestId": "again-1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(other_call["code"], rules::REQUEST_REUSED, "{other_call}");
    // The replay started nothing: the day counts one start, the new
    // session none of its own.
    let (day, others) = team
        .harness
        .host
        .team_store(|store| {
            (
                store.outside_started_since(0),
                store.children(&rules::outside_caller(OTHER)).len(),
            )
        })
        .await
        .unwrap();
    assert_eq!((day, others), (1, 0));
    // The first session's own retry is unchanged.
    assert_eq!(
        team.outside(SESSION, "start_thread", input).await.unwrap(),
        first
    );
}

/// The real outside socket: owner-only (0700 directory, 0600 socket), one
/// session per connection named by the host, `hello` before any call, the
/// client's name used for display only; refused peers are tested by
/// `peer_allowed`.
#[tokio::test]
async fn the_outside_socket_serves_hello_then_calls_for_its_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let (team, _) = outside_team().await;
    team.set_level(OutsideAgents::Read).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("team").join("outside.sock");
    let uid = interface_common::unix::effective_uid().unwrap();
    let server =
        crate::outside_agents::OutsideAgentsServer::start_at(team.harness.host.clone(), &path, uid)
            .unwrap();
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(crate::outside_agents::peer_allowed(Some(uid), uid));
    assert!(!crate::outside_agents::peer_allowed(Some(uid + 1), uid));
    assert!(!crate::outside_agents::peer_allowed(None, uid));

    let stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let limit = std::time::Duration::from_secs(30);
    let mut exchange = async |request: Value| {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(limit, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let early = exchange(json!({"jsonrpc": "2.0", "id": 1, "method": "call",
        "params": {"tool": "list_threads", "input": {}}}))
    .await;
    assert_eq!(early["error"]["code"], -32_002, "hello first");
    let hello = exchange(json!({"jsonrpc": "2.0", "id": 2, "method": "hello",
        "params": {"client": "codex\nmcp"}}))
    .await;
    assert_eq!(hello["result"], json!({"protocol": 1, "level": "read"}));
    let again = exchange(json!({"jsonrpc": "2.0", "id": 3, "method": "hello",
        "params": {"client": "someone else"}}))
    .await;
    assert!(again["error"].is_object(), "one hello per session");
    let listed = exchange(json!({"jsonrpc": "2.0", "id": 4, "method": "call",
        "params": {"tool": "list_threads", "input": {}}}))
    .await;
    assert_eq!(listed["result"]["isError"], false);
    let text: Value = serde_json::from_str(listed["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["threads"].as_array().unwrap().len(), 2);
    let refused = exchange(json!({"jsonrpc": "2.0", "id": 5, "method": "call",
        "params": {"tool": "start_thread", "input": {"project": "atlas", "task": "x"}}}))
    .await;
    assert_eq!(refused["result"]["isError"], true);
    assert!(
        refused["result"]["text"]
            .as_str()
            .unwrap()
            .contains("read-only")
    );
    let unknown = exchange(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"})).await;
    assert_eq!(unknown["error"]["code"], -32_601, "a narrow protocol");
    drop(server);
    assert!(!path.exists(), "removed when the host stops serving");
}

/// A side question's fork (protocol 67) is a hidden session, not a thread:
/// team tools' and outside agents' `list_threads` never show it and their
/// `read_thread` refuses it, and it cannot hold a schedule, a stash or be
/// referred to as a thread.
#[tokio::test]
async fn a_side_questions_fork_is_invisible_to_team_tools_and_outside_agents() {
    use interface_prompt_context::PromptContextThreads as _;
    const SIDE: &str = "claude#side-hidden";
    let (team, _) = outside_team().await;
    team.set_level(OutsideAgents::Full).await;
    let host = team.harness.host.clone();
    host.attach_sides(ThreadHost::open_side_store(team.harness.state.path()));
    host.attach_stash(ThreadHost::open_stash_store(team.harness.state.path()));
    host.attach_schedules(ThreadHost::open_schedule_store(team.harness.state.path()));
    *lock(&team.harness.sessions.next_fork) = Some(SIDE.into());
    team.harness
        .command(json!({"kind": "sideAsk", "key": CALLER, "question": "Which test?"}))
        .await
        .unwrap();
    host.reconcile().await;

    let listed = team.call(CALLER, "list_threads", json!({})).await.unwrap();
    assert!(!listed.to_string().contains(SIDE), "{listed}");
    assert!(
        team.call(CALLER, "read_thread", json!({"thread": SIDE}))
            .await
            .is_err()
    );
    let outside = team
        .outside(SESSION, "list_threads", json!({}))
        .await
        .unwrap();
    assert!(!outside.to_string().contains(SIDE), "{outside}");
    assert!(
        team.outside(SESSION, "read_thread", json!({"thread": SIDE}))
            .await
            .is_err()
    );

    // Not a thread for schedules, stashes or thread references.
    let when = json!({"kind": "interval", "minutes": 60});
    let scheduled = team
        .harness
        .command(json!({"kind": "scheduleSet", "key": SIDE, "prompt": "p", "when": when}))
        .await
        .unwrap_err();
    assert_ne!(
        scheduled.code,
        crate::threads::THREADS_SCHEDULES_UNAVAILABLE_CODE
    );
    let stashed = host
        .dispatch(
            CHANNELS.draft_stash,
            json!({"kind": "push", "key": SIDE, "draft": {"text": "x", "items": []}}),
        )
        .await
        .unwrap_err();
    assert_ne!(stashed.code, crate::threads::DRAFT_STASH_UNAVAILABLE_CODE);
    let reference = host.clone();
    let referred = tokio::task::spawn_blocking(move || reference.thread_reference(CALLER, SIDE))
        .await
        .unwrap();
    assert!(referred.is_err());
}
