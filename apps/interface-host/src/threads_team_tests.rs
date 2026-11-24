//! Team tools (T3 section 2, protocol 57) through the thread host with fake
//! ports: a fake guarded runtime that runs the real send hook, a journal the
//! test writes for the started threads, and the real socket server.

use std::{
    collections::HashMap,
    sync::{OnceLock, atomic::AtomicU64},
};

use interface_contracts::{
    engine::{
        CollaborationMcpSessionRequest, EngineEventKind, SessionKind, SessionStatus,
        TEAM_TOOL_NAMES, TeamToolsFactory,
    },
    thread_journal::{JournalEntry, TurnOrigin},
    threads::{LineageRelation, RunOutcome},
};
use interface_settings::{
    thread_journal::ThreadJournal,
    thread_team::{DeliveryHow, THREAD_TEAM_FILE_NAME, ThreadTeamStore},
};
use serde_json::{Value, json};

use super::*;
use crate::dispatcher::{SendHooks, SendSettlement, SessionAccessMemory};
use crate::threads::team::NoticeStep;

const CALLER: &str = "claude#caller";

/// The guarded runtime as team tools see it: creates register a registry
/// row, sends run the real send hook (turn, journal prompt, echo) and
/// settle it, interrupts tell the run tap.
#[derive(Default)]
struct FakeTeamRuntime {
    host: OnceLock<ThreadHost>,
    sessions: OnceLock<Arc<FakeSessions>>,
    next: AtomicU64,
    creates: Mutex<Vec<Value>>,
    /// Each send: key, text, whether the pane was shown it (echo).
    sends: Mutex<Vec<(String, String, bool)>>,
    interrupts: Mutex<Vec<String>>,
    create_fails: Mutex<Option<DispatchError>>,
    send_fails: Mutex<Option<DispatchError>>,
}

#[async_trait]
impl HostDispatcher for FakeTeamRuntime {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let host = self.host.get().unwrap();
        if operation == CHANNELS.create_session {
            lock(&self.creates).push(payload.clone());
            if let Some(error) = lock(&self.create_fails).clone() {
                return Err(error);
            }
            let engine = payload["engine"].as_str().unwrap();
            let key = format!(
                "{engine}#child-{}",
                self.next.fetch_add(1, Ordering::SeqCst)
            );
            let mut summary = row(&key, SessionKind::Code);
            summary.cwd = payload["options"]["cwd"].as_str().unwrap().to_owned();
            // The runtime registers a launch under its name.
            if let Some(name) = payload["options"]["name"].as_str() {
                name.clone_into(&mut summary.title);
            }
            self.sessions.get().unwrap().upsert(summary);
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
        if operation == CHANNELS.interrupt {
            let key = payload["sessionKey"].as_str().unwrap().to_owned();
            host.interrupt_requested(&key);
            lock(&self.interrupts).push(key);
            return Ok(Value::Null);
        }
        if operation == CHANNELS.probe_engines {
            return Ok(json!([
                {"engine": "claude", "ok": true, "version": "2.1.0", "info": {
                    "binaryPath": "/usr/local/bin/claude", "version": "2.1.0", "loginIdentity": null,
                    "capabilities": {"streaming": true, "approvals": true, "resume": true,
                        "fork": true, "cloudSessions": false, "models": ["opus", "sonnet"],
                        "effortLevels": [], "permissionModes": ["default", "plan"]},
                    "modelCatalog": [
                        {"value": "sonnet", "displayName": "Sonnet", "supportsEffort": false, "effortLevels": []},
                        {"value": "opus", "displayName": "Opus", "supportsEffort": false, "effortLevels": [], "isDefault": true}
                    ]}},
                {"engine": "codex", "ok": false, "error": "Codex is not installed."}
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

struct Team {
    harness: Harness,
    runtime: Arc<FakeTeamRuntime>,
    access: Arc<FakeAccess>,
    root: String,
    journal: ThreadJournal,
    at: u64,
}

/// A journalled host with team tools attached and one idle caller thread,
/// Local in `root`.
async fn team() -> Team {
    let mut harness = journal_harness();
    let root = harness.dir("atlas");
    harness
        .git_thread(CALLER, &root, None, "lead the work")
        .await;
    end_record_run(&harness, CALLER).await;
    let runtime = Arc::new(FakeTeamRuntime::default());
    let _ = runtime.host.set(harness.host.clone());
    let _ = runtime.sessions.set(harness.sessions.clone());
    let access = Arc::new(FakeAccess::default());
    let store = ThreadTeamStore::open(&harness.state.path().join(THREAD_TEAM_FILE_NAME))
        .map_err(|error| error.to_string());
    harness.host.attach_team(ThreadTeamConfig {
        runtime: runtime.clone(),
        access: Some(access.clone()),
        store,
    });
    let journal = ThreadJournal::open(harness.state.path()).unwrap();
    Team {
        harness,
        runtime,
        access,
        root,
        journal,
        at: NOW,
    }
}

async fn end_record_run(harness: &Harness, key: &str) {
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

impl Team {
    async fn call(&self, caller: &str, name: &str, input: Value) -> Result<Value, Value> {
        self.harness
            .host
            .team_call(caller, name, input)
            .await
            .map(|text| serde_json::from_str(&text).unwrap())
            .map_err(|text| serde_json::from_str(&text).unwrap())
    }

    async fn start(&self, input: Value) -> String {
        let started = self.call(CALLER, "start_thread", input).await.unwrap();
        started["thread"].as_str().unwrap().to_owned()
    }

    fn write(&mut self, key: &str, entry: JournalEntry) {
        self.at += 1;
        self.journal.append(key, self.at, entry).unwrap();
    }

    /// The child's run ends as `outcome` with `answer`: in the tap, the
    /// store and the journal.
    async fn finish(&mut self, key: &str, answer: &str, outcome: RunOutcome) {
        self.harness.host.observe_run(
            key,
            &EngineEventKind::MessageDelta {
                text: answer.into(),
                agent_key: None,
            },
        );
        self.harness.host.observe_run(
            key,
            &EngineEventKind::StatusChanged {
                status: if outcome == RunOutcome::Failed {
                    SessionStatus::Failed
                } else {
                    SessionStatus::Idle
                },
            },
        );
        let target = key.to_owned();
        self.harness
            .host
            .edit_quiet(move |document, now| {
                document.run_ended(&target, now, outcome);
                Ok(())
            })
            .await
            .unwrap();
        self.write(
            key,
            JournalEntry::TurnStarted {
                origin: TurnOrigin::Host,
                prompt: "task".into(),
                digest: "d".into(),
                cut: false,
            },
        );
        if outcome == RunOutcome::Failed {
            self.write(
                key,
                JournalEntry::Error {
                    message: "Not logged in".into(),
                    retryable: false,
                    cause: None,
                },
            );
        } else {
            self.write(
                key,
                JournalEntry::Message {
                    text: answer.into(),
                    agent_key: None,
                    cut: false,
                },
            );
        }
        self.write(
            key,
            JournalEntry::TurnEnded {
                outcome,
                reason: None,
            },
        );
    }

    async fn delivered(&self, child: &str) -> Option<(u32, DeliveryHow)> {
        let owned = child.to_owned();
        self.harness
            .host
            .team_store(move |store| {
                store
                    .started(&owned)
                    .and_then(|entry| entry.delivered)
                    .map(|delivered| (delivered.turn, delivered.how))
            })
            .await
            .unwrap()
    }

    fn sends_to(&self, key: &str) -> Vec<(String, bool)> {
        lock(&self.runtime.sends)
            .iter()
            .filter(|(to, _, _)| to == key)
            .map(|(_, text, echo)| (text.clone(), *echo))
            .collect()
    }
}

/// The server's exact tool list, with closed schemas and honest
/// annotations, over the real socket: the caller is the socket it connected
/// to, unbound until the provider names its session, and a socket answers
/// only for its own session.
#[tokio::test]
#[allow(clippy::too_many_lines)] // The catalogue, then one socket's whole life.
async fn the_team_server_lists_exact_tools_and_answers_for_its_own_socket_only() {
    use tokio::io::{AsyncBufReadExt as _, BufReader};
    let team = team().await;
    let catalogue = crate::team_tools::catalogue();
    assert_eq!(
        catalogue
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        TEAM_TOOL_NAMES
    );
    let required = |name: &str| -> Vec<&str> {
        let tool = catalogue.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect()
    };
    assert_eq!(required("start_thread"), ["task"]);
    assert_eq!(required("send_to_thread"), ["thread", "message"]);
    assert!(required("team_info").is_empty() && required("list_threads").is_empty());
    for name in [
        "team_info",
        "wait_thread",
        "thread_status",
        "list_threads",
        "read_thread",
    ] {
        let tool = catalogue.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
    }

    let base = tempfile::tempdir().unwrap();
    let servers = crate::team_tools::TeamToolServers::with_base(
        team.harness.host.clone(),
        base.path().into(),
    );
    let request = |id: &str| CollaborationMcpSessionRequest {
        engine: "claude".parse().unwrap(),
        provisional_session_id: id.into(),
        resumed: false,
    };
    let binding = servers.create(request("caller")).await.unwrap().unwrap();
    // This test process connects below, so it plays the session's provider:
    // the socket answers only that process and the ones it started.
    binding.provider_process_started(std::process::id()).await;
    let descriptor = binding.descriptor();
    assert_eq!(descriptor.name, "iface_team");
    assert_eq!(descriptor.command, "/usr/bin/nc");
    assert!(descriptor.args[1].ends_with("/s.sock"));
    let stream = tokio::net::UnixStream::connect(&descriptor.args[1])
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let listed = crate::mcp_transport::test_support::exchange(
        &mut writer,
        &mut lines,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(listed["result"]["tools"], json!(catalogue));
    let info = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"team_info","arguments":{}}});
    let early =
        crate::mcp_transport::test_support::exchange(&mut writer, &mut lines, info.clone()).await;
    assert_eq!(early["result"]["isError"], true, "not bound yet");
    // Another engine's key, or a different one later, never binds it.
    assert!(
        binding
            .confirm_source_session_key("codex#caller".into())
            .await
            .is_err()
    );
    binding
        .confirm_source_session_key(CALLER.into())
        .await
        .unwrap();
    assert!(
        binding
            .confirm_source_session_key("claude#other".into())
            .await
            .is_err()
    );
    let answered =
        crate::mcp_transport::test_support::exchange(&mut writer, &mut lines, info).await;
    let text = answered["result"]["content"][0]["text"].as_str().unwrap();
    let info: Value = serde_json::from_str(text).unwrap();
    assert_eq!(info["you"]["thread"], CALLER);
    assert_eq!(info["you"]["project"], team.root);
    assert_eq!(info["you"]["workspace"], "local");
    assert_eq!(info["you"]["mode"], "default");
    assert_eq!(
        info["limits"],
        json!({"runningThreads": 4, "totalThreads": 16})
    );
    assert_eq!(
        info["providers"][0]["models"],
        json!(["opus", "sonnet"]),
        "default first"
    );
    assert_eq!(info["providers"][0]["label"], "Claude");
    assert_eq!(info["providers"][1]["ready"], false);
    assert_eq!(info["providers"][1]["reason"], "Codex is not installed.");
    // A tool input naming another caller is refused: identity is the socket.
    let spoofed = crate::mcp_transport::test_support::exchange(
        &mut writer,
        &mut lines,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"team_info","arguments":{"caller":"claude#other"}}}),
    )
    .await;
    assert_eq!(spoofed["result"]["isError"], true);
    binding.dispose().await.unwrap();
    assert!(!std::path::Path::new(&descriptor.args[1]).exists());

    // Off in Settings: no session is offered the server.
    let mut settings = team.harness.document().await.settings;
    settings.team_tools = false;
    team.harness
        .command(json!({"kind": "setSettings", "settings": settings}))
        .await
        .unwrap();
    assert!(servers.create(request("next")).await.unwrap().is_none());
}

/// Start: a real thread in the caller's folder with its `child` lineage,
/// created one hop down on the guarded runtime in the caller's permission
/// rung or a narrower one, its task sent through the send hook as turn 0
/// (shown in its pane); a caller on a rung the target has nothing at or
/// below is refused before anything is made.
#[tokio::test]
async fn start_thread_files_a_child_thread_sends_its_task_and_never_widens_its_mode() {
    let team = team().await;
    lock(&team.access.0).insert(CALLER.into(), "acceptEdits".into());
    let started = team
        .call(
            CALLER,
            "start_thread",
            json!({"task": "append child.txt hello from the child", "provider": "codex"}),
        )
        .await
        .unwrap();
    let child = started["thread"].as_str().unwrap();
    assert_eq!(started["title"], "append child.txt hello from the child");
    assert_eq!(started["provider"], "codex");
    assert_eq!(started["status"], "running");
    let create = lock(&team.runtime.creates)[0].clone();
    assert_eq!(create["engine"], "codex");
    assert_eq!(create["options"]["permissionMode"], "on-request");
    assert_eq!(create["options"]["collaborationDepth"], 1);
    assert_eq!(create["options"]["prompt"], "");
    assert_eq!(create["options"]["cwd"], team.root);
    assert_eq!(create["options"]["kind"], "code");
    let document = team.harness.document().await;
    let record = document.record(child).unwrap();
    let lineage = record.lineage.as_ref().unwrap();
    assert_eq!(
        (lineage.parent.as_str(), lineage.relation),
        (CALLER, LineageRelation::Child)
    );
    assert_eq!(record.turns.len(), 1, "turn 0 is the task");
    assert_eq!(
        record.turns[0].preview,
        "append child.txt hello from the child"
    );
    assert_eq!(record.workspace.as_ref().unwrap().project_root, team.root);
    assert_eq!(
        team.sends_to(child),
        [("append child.txt hello from the child".to_owned(), true)]
    );
    assert!(team.harness.host.is_team_child(child));
    // The started thread has no team tools of its own.
    let nested = team.call(child, "team_info", json!({})).await.unwrap_err();
    assert_eq!(nested["code"], "no-nesting");

    // A planning caller cannot start a Codex thread (Codex has no plan mode).
    lock(&team.access.0).insert(CALLER.into(), "plan".into());
    let refused = team
        .call(
            CALLER,
            "start_thread",
            json!({"task": "x", "provider": "codex"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], "mode-unavailable");
    assert_eq!(lock(&team.runtime.creates).len(), 1, "nothing was made");
    // On its own provider it runs planning too.
    team.start(json!({"task": "plan it"})).await;
    let create = lock(&team.runtime.creates)[1].clone();
    assert_eq!(create["options"]["permissionMode"], "plan");
    // A tool input that names a mode is refused.
    let moded = team
        .call(
            CALLER,
            "start_thread",
            json!({"task": "x", "permissionMode": "bypassPermissions"}),
        )
        .await
        .unwrap_err();
    assert_eq!(moded["code"], "invalid-input");
}

/// Wait: the journal's answer once the run and its record end; a wait that
/// times out stops nothing; a thread stopped on an approval or a question is
/// `needsYou`; reading the result means no notice repeats it.
#[tokio::test(start_paused = true)]
async fn wait_thread_answers_from_the_journal_times_out_and_reports_needs_you() {
    let mut team = team().await;
    let child = team.start(json!({"task": "count the tests"})).await;
    let timed_out = team
        .call(
            CALLER,
            "wait_thread",
            json!({"thread": child, "timeoutSec": 5}),
        )
        .await
        .unwrap();
    assert_eq!(timed_out["status"], "running");
    assert_eq!(timed_out["waitTimedOut"], true);
    assert!(
        lock(&team.runtime.interrupts).is_empty(),
        "a timeout cancels nothing"
    );

    team.harness.host.observe_run(
        &child,
        &EngineEventKind::ApprovalRequested {
            approval_id: "a1".into(),
            tool: "Bash".into(),
            summary: "rm -rf build".into(),
            input: json!({}),
        },
    );
    let waiting = team
        .call(CALLER, "wait_thread", json!({"thread": child}))
        .await
        .unwrap();
    assert_eq!(
        (waiting["status"].as_str(), waiting["needs"].as_str()),
        (Some("needsYou"), Some("approval"))
    );
    team.harness.host.observe_run(
        &child,
        &EngineEventKind::ApprovalRequested {
            approval_id: "a2".into(),
            tool: "AskUserQuestion".into(),
            summary: "Which file?".into(),
            input: json!({}),
        },
    );
    let asked = team
        .call(CALLER, "thread_status", json!({"thread": child}))
        .await
        .unwrap();
    assert_eq!(asked["needs"], "question");

    team.finish(&child, "There are 42 tests.", RunOutcome::Completed)
        .await;
    let done = team
        .call(
            CALLER,
            "wait_thread",
            json!({"thread": child, "timeoutSec": 600}),
        )
        .await
        .unwrap();
    assert_eq!(done["status"], "done");
    assert_eq!(done["answer"], "There are 42 tests.");
    assert_eq!(done["turn"], 0);
    assert_eq!(done["waitTimedOut"], false);
    assert_eq!(team.delivered(&child).await, Some((0, DeliveryHow::Read)));
    assert_eq!(
        team.harness.host.notice_step(&child).await,
        NoticeStep::Done
    );
    assert!(team.sends_to(CALLER).is_empty(), "read already: no notice");
}

/// Stop and send reach only threads the caller started; a send goes only to
/// an idle one, through the send hook (shown in its pane, a turn of its own).
#[tokio::test]
async fn stop_and_send_reach_only_own_started_threads_and_send_only_when_idle() {
    let mut team = team().await;
    let child = team.start(json!({"task": "draft the notes"})).await;
    let busy = team
        .call(
            CALLER,
            "send_to_thread",
            json!({"thread": child, "message": "also this"}),
        )
        .await
        .unwrap_err();
    assert_eq!(busy["code"], "thread-busy");
    let stopped = team
        .call(CALLER, "stop_thread", json!({"thread": child}))
        .await
        .unwrap();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(*lock(&team.runtime.interrupts), [child.clone()]);
    team.finish(&child, "Partial notes.", RunOutcome::Interrupted)
        .await;
    let idle = team
        .call(CALLER, "stop_thread", json!({"thread": child}))
        .await
        .unwrap_err();
    assert_eq!(idle["code"], "thread-idle");
    team.call(
        CALLER,
        "send_to_thread",
        json!({"thread": child, "message": "now finish them"}),
    )
    .await
    .unwrap();
    assert_eq!(
        team.sends_to(&child).last().cloned(),
        Some(("now finish them".to_owned(), true))
    );
    assert_eq!(
        team.harness
            .document()
            .await
            .record(&child)
            .unwrap()
            .turns
            .len(),
        2
    );

    // A thread of the same project the caller did not start.
    let mine = "claude#mine";
    let root = team.root.clone();
    team.harness.git_thread(mine, &root, None, "my own").await;
    end_record_run(&team.harness, mine).await;
    for (tool, input) in [
        ("send_to_thread", json!({"thread": mine, "message": "hi"})),
        ("stop_thread", json!({"thread": mine})),
    ] {
        let refused = team.call(CALLER, tool, input).await.unwrap_err();
        assert_eq!(refused["code"], "not-started-by-you", "{tool}");
    }
}

/// List and read see the caller's project only: never another project's
/// threads, hidden sessions or agents' sessions.
#[tokio::test]
async fn list_and_read_are_scoped_to_the_callers_project() {
    let mut team = team().await;
    let child = team.start(json!({"task": "read the logs"})).await;
    team.finish(&child, "The logs are clean.", RunOutcome::Completed)
        .await;
    let neighbour = "codex#neighbour";
    let root = team.root.clone();
    team.harness
        .git_thread(neighbour, &root, None, "neighbour work")
        .await;
    let other_root = team.harness.dir("other");
    team.harness
        .git_thread("codex#elsewhere", &other_root, None, "other project")
        .await;
    let hidden = "claude#hidden";
    team.harness
        .git_thread(hidden, &root, None, "a worker")
        .await;
    lock(&team.harness.hidden.0).insert(hidden.into());

    let listed = team.call(CALLER, "list_threads", json!({})).await.unwrap();
    let threads = listed["threads"].as_array().unwrap();
    let keys = threads
        .iter()
        .map(|thread| thread["thread"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(keys.contains(&child.as_str()) && keys.contains(&neighbour));
    assert!(
        !keys.contains(&"codex#elsewhere") && !keys.contains(&hidden) && !keys.contains(&CALLER)
    );
    let started = threads
        .iter()
        .find(|thread| thread["thread"] == child.as_str())
        .unwrap();
    assert_eq!(started["startedByYou"], true);
    assert_eq!(started["status"], "done");
    assert_eq!(started["section"], "active");

    let read = team
        .call(CALLER, "read_thread", json!({"thread": child}))
        .await
        .unwrap();
    let kinds = read["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["prompt", "answer", "turnEnded"]);
    assert_eq!(read["items"][1]["text"], "The logs are clean.");
    assert!(read.get("next").is_none());
    for thread in ["codex#elsewhere", hidden] {
        let refused = team
            .call(CALLER, "read_thread", json!({"thread": thread}))
            .await
            .unwrap_err();
        assert!(
            ["other-project", "not-found"].contains(&refused["code"].as_str().unwrap()),
            "{thread}: {refused}"
        );
    }
    // An agent's session is never readable.
    team.harness
        .sessions
        .edit(neighbour, |row| row.kind = Some(SessionKind::Agent));
    let agent = team
        .call(CALLER, "thread_status", json!({"thread": neighbour}))
        .await
        .unwrap_err();
    assert_eq!(agent["code"], "not-found");
}

/// A retried `requestId` answers the first call and starts nothing new; a
/// start whose outcome is unknown is reported so and never tried again.
#[tokio::test]
async fn a_retried_request_starts_nothing_new_and_an_unknown_start_is_never_retried() {
    let team = team().await;
    let input = json!({"task": "one thing", "requestId": "start-1"});
    let first = team
        .call(CALLER, "start_thread", input.clone())
        .await
        .unwrap();
    let again = team.call(CALLER, "start_thread", input).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(lock(&team.runtime.creates).len(), 1);
    let reused = team
        .call(
            CALLER,
            "start_thread",
            json!({"task": "another", "requestId": "start-1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(reused["code"], "request-reused");

    *lock(&team.runtime.create_fails) = Some(DispatchError::new(
        "provider-outcome-unknown",
        "The provider may have started.",
        false,
        false,
    ));
    let input = json!({"task": "risky", "requestId": "start-2"});
    let unknown = team
        .call(CALLER, "start_thread", input.clone())
        .await
        .unwrap_err();
    assert_eq!(unknown["code"], "outcome-unknown");
    *lock(&team.runtime.create_fails) = None;
    let retried = team.call(CALLER, "start_thread", input).await.unwrap_err();
    assert_eq!(retried["code"], "outcome-unknown");
    assert_eq!(lock(&team.runtime.creates).len(), 2, "never tried again");
    // A definite refusal is a plain refusal, and its retry answers it again.
    *lock(&team.runtime.create_fails) = Some(DispatchError::new(
        "provider-unavailable",
        "Codex is not installed.",
        false,
        true,
    ));
    let input = json!({"task": "x", "provider": "codex", "requestId": "start-3"});
    let definite = team
        .call(CALLER, "start_thread", input.clone())
        .await
        .unwrap_err();
    assert_eq!(definite["code"], "refused");
    assert_eq!(
        team.call(CALLER, "start_thread", input).await.unwrap_err(),
        definite
    );
    assert_eq!(lock(&team.runtime.creates).len(), 3);
}

/// Limits: at most 4 running and 16 in all per caller.
#[tokio::test]
async fn start_limits_count_running_and_total_started_threads() {
    let mut team = team().await;
    let mut children = Vec::new();
    for index in 0..4 {
        children.push(team.start(json!({"task": format!("job {index}")})).await);
    }
    let running = team
        .call(CALLER, "start_thread", json!({"task": "one more"}))
        .await
        .unwrap_err();
    assert_eq!(running["code"], "limit-running");
    for child in &children {
        team.finish(child, "ok", RunOutcome::Completed).await;
    }
    for index in 4..16 {
        let child = team.start(json!({"task": format!("job {index}")})).await;
        team.finish(&child, "ok", RunOutcome::Completed).await;
    }
    let total = team
        .call(CALLER, "start_thread", json!({"task": "seventeen"}))
        .await
        .unwrap_err();
    assert_eq!(total["code"], "limit-total");
}

/// The notice: sent once, shown in the caller's pane, when the caller is
/// idle; held while the caller's turn runs; dropped when the caller is
/// settled or Away holds it; never sent when the result was already read.
#[tokio::test]
async fn a_finished_thread_notices_its_idle_caller_once_and_never_otherwise() {
    let mut team = team().await;
    let child = team
        .start(json!({"task": "summarize", "title": "Summary"}))
        .await;
    team.finish(&child, "Three points.", RunOutcome::Completed)
        .await;
    // Held while the caller runs.
    team.harness.host.open_run(CALLER);
    assert_eq!(
        team.harness.host.notice_step(&child).await,
        NoticeStep::Wait
    );
    assert!(team.sends_to(CALLER).is_empty());
    team.harness.host.close_run(CALLER);
    // Through the worker's path once the caller is idle.
    team.harness.host.team_run_ended(&child);
    settle_tasks(&team.harness.host).await;
    assert_eq!(
        team.sends_to(CALLER),
        [(
            "[Interface] The thread \"Summary\" (Claude) you started finished: Three points."
                .to_owned(),
            true
        )]
    );
    assert_eq!(team.delivered(&child).await, Some((0, DeliveryHow::Sent)));
    let caller_turns = team
        .harness
        .document()
        .await
        .record(CALLER)
        .unwrap()
        .turns
        .len();
    assert_eq!(caller_turns, 2, "the notice is a turn of the caller's");
    // Once.
    end_record_run(&team.harness, CALLER).await;
    assert_eq!(
        team.harness.host.notice_step(&child).await,
        NoticeStep::Done
    );
    assert_eq!(team.sends_to(CALLER).len(), 1);

    // A failed run says so; a settled caller drops it.
    let failing = team
        .start(json!({"task": "deploy", "title": "Deploy"}))
        .await;
    team.finish(&failing, "", RunOutcome::Failed).await;
    team.harness
        .command(json!({"kind": "settle", "key": CALLER}))
        .await
        .unwrap();
    assert_eq!(
        team.harness.host.notice_step(&failing).await,
        NoticeStep::Done
    );
    assert_eq!(
        team.delivered(&failing).await,
        Some((0, DeliveryHow::Dropped))
    );
    team.harness
        .command(json!({"kind": "unsettle", "key": CALLER}))
        .await
        .unwrap();
    // Away holding the caller drops it too.
    let held = team.start(json!({"task": "lint", "title": "Lint"})).await;
    team.finish(&held, "Clean.", RunOutcome::Completed).await;
    lock(&team.harness.host.inner.away_holds)
        .insert(CALLER.into(), crate::threads::away::AwayHold::default());
    assert_eq!(team.harness.host.notice_step(&held).await, NoticeStep::Done);
    assert_eq!(team.delivered(&held).await, Some((0, DeliveryHow::Dropped)));
    lock(&team.harness.host.inner.away_holds).remove(CALLER);
    // A stopped run makes none; notify off makes none.
    let quiet = team.start(json!({"task": "quiet", "notify": false})).await;
    team.finish(&quiet, "Done.", RunOutcome::Completed).await;
    assert_eq!(
        team.harness.host.notice_step(&quiet).await,
        NoticeStep::Done
    );
    assert_eq!(team.delivered(&quiet).await, None);
    assert_eq!(team.sends_to(CALLER).len(), 1);
    // An unknown send outcome is recorded and never sent again.
    let lost = team.start(json!({"task": "lost", "title": "Lost"})).await;
    team.finish(&lost, "Gone.", RunOutcome::Completed).await;
    *lock(&team.runtime.send_fails) = Some(DispatchError::new(
        "provider-outcome-unknown",
        "The provider stopped answering.",
        false,
        false,
    ));
    assert_eq!(team.harness.host.notice_step(&lost).await, NoticeStep::Done);
    *lock(&team.runtime.send_fails) = None;
    assert_eq!(team.delivered(&lost).await, Some((0, DeliveryHow::Unknown)));
    end_record_run(&team.harness, CALLER).await;
    assert_eq!(team.harness.host.notice_step(&lost).await, NoticeStep::Done);
    assert_eq!(team.sends_to(CALLER).len(), 2, "one attempt");
}

/// T3 section 9: a session's team tools are known to the thread host while
/// its binding is confirmed, so a thread reference it sends mentions
/// `read_thread` only then.
#[tokio::test]
async fn a_confirmed_team_binding_marks_its_session_as_having_team_tools() {
    let team = team().await;
    let base = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/private/tmp")
        .unwrap();
    let servers = crate::team_tools::TeamToolServers::with_base(
        team.harness.host.clone(),
        base.path().into(),
    );
    let binding = servers
        .create(CollaborationMcpSessionRequest {
            engine: "claude".parse().unwrap(),
            provisional_session_id: "caller".into(),
            resumed: false,
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!team.harness.host.has_team_tools(CALLER));
    binding
        .confirm_source_session_key(CALLER.into())
        .await
        .unwrap();
    assert!(team.harness.host.has_team_tools(CALLER));
    binding.dispose().await.unwrap();
    assert!(!team.harness.host.has_team_tools(CALLER));
}

/// Visual replies (T3 section 8): the caller's thread with team tools
/// attached and the host's own journal worker running (the visual store is
/// the worker's).
async fn visual_team() -> (Harness, Arc<FakeTeamRuntime>) {
    let mut harness = journal_harness();
    let root = harness.dir("atlas");
    harness.git_thread(CALLER, &root, None, "chart it").await;
    end_record_run(&harness, CALLER).await;
    let runtime = Arc::new(FakeTeamRuntime::default());
    let _ = runtime.host.set(harness.host.clone());
    let _ = runtime.sessions.set(harness.sessions.clone());
    let store = ThreadTeamStore::open(&harness.state.path().join(THREAD_TEAM_FILE_NAME))
        .map_err(|error| error.to_string());
    harness.host.attach_team(ThreadTeamConfig {
        runtime: runtime.clone(),
        access: None,
        store,
    });
    harness.host.start_journal();
    (harness, runtime)
}

/// Every `visual.shown` session event pushed so far, as `(key, event)`.
fn shown_visuals(harness: &mut Harness) -> Vec<(String, Value)> {
    let mut shown = Vec::new();
    while let Ok(frame) = harness.pushes.try_recv() {
        if frame.operation != CHANNELS.event {
            continue;
        }
        let payload: Value = frame.payload.decode().unwrap();
        if payload["event"]["kind"] == "visual.shown" {
            shown.push((
                payload["key"].as_str().unwrap().to_owned(),
                payload["event"].clone(),
            ));
        }
    }
    shown
}

async fn read_visual(harness: &Harness, key: &str, id: &str) -> Option<String> {
    let answer = harness
        .host
        .dispatch(
            CHANNELS.thread_journal,
            json!({"kind": "visual", "key": key, "id": id}),
        )
        .await
        .unwrap();
    serde_json::from_value::<interface_contracts::thread_journal::ThreadJournalResponse>(answer)
        .unwrap()
        .visual
        .map(|visual| visual.html)
}

/// `show_visual` keeps the page, journals it and shows it once in the
/// caller's pane; `iface:threadJournal` `visual` reads it back for the
/// thread only, and the provider's history gains it by time. Refusals are
/// typed, keep nothing and show nothing; a started thread has no visuals
/// (it has no team tools at all); removing the session removes its pages.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One visual's whole life.
async fn show_visual_keeps_journals_and_shows_the_page_once_and_refuses_with_codes() {
    let (mut harness, _runtime) = visual_team().await;
    let page = "<!doctype html><html><body><canvas id=c></canvas><script>c.getContext('2d').fillRect(0,0,9,9)</script></body></html>";
    let host = harness.host.clone();
    let call = |name: &str, input: Value| {
        let host = host.clone();
        let name = name.to_owned();
        async move {
            host.team_call(CALLER, &name, input)
                .await
                .map(|text| serde_json::from_str::<Value>(&text).unwrap())
                .map_err(|text| serde_json::from_str::<Value>(&text).unwrap())
        }
    };
    let shown = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        call(
            "show_visual",
            json!({"title": "  Signups by week ", "html": page}),
        ),
    )
    .await
    .expect("show_visual answers within 30 s")
    .unwrap();
    let id = shown["id"].as_str().unwrap().to_owned();
    assert!(
        interface_contracts::thread_journal::is_visual_id(&id),
        "{id}"
    );
    assert_eq!(shown["bytes"], page.len());
    let pushed = shown_visuals(&mut harness);
    assert_eq!(pushed.len(), 1, "shown once: {pushed:?}");
    assert_eq!(pushed[0].0, CALLER);
    assert_eq!(pushed[0].1["id"], id.as_str());
    assert_eq!(pushed[0].1["title"], "Signups by week");
    assert_eq!(pushed[0].1["bytes"], page.len());
    assert!(pushed[0].1["at"].is_string());
    assert_eq!(
        read_visual(&harness, CALLER, &id).await.as_deref(),
        Some(page)
    );
    assert_eq!(read_visual(&harness, "claude#other", &id).await, None);

    // Typed refusals: nothing kept, nothing shown.
    for (input, code) in [
        (
            json!({"title": "Big", "html": format!("<p>{}</p>", "a".repeat(512 * 1024))}),
            "visual-too-large",
        ),
        (
            json!({"title": "Prose", "html": "Here is a chart: <svg></svg>"}),
            "visual-not-html",
        ),
        (json!({"title": "", "html": page}), "invalid-input"),
        (
            json!({"title": "x", "html": page, "url": "https://example.com"}),
            "invalid-input",
        ),
        (json!({"title": "x"}), "invalid-input"),
    ] {
        let refused = call("show_visual", input).await.unwrap_err();
        assert_eq!(refused["code"], code, "{refused}");
    }
    assert!(shown_visuals(&mut harness).is_empty());

    // A started thread never gets team tools, so it never shows a visual.
    let child = "claude#started";
    harness.host.mark_team_child(child);
    assert!(!harness.host.team_offered(Some(child)).await);
    let nested = harness
        .host
        .team_call(child, "show_visual", json!({"title": "x", "html": page}))
        .await
        .unwrap_err();
    assert!(nested.contains("no-nesting"), "{nested}");

    // The provider's own history gains the visual by time.
    let provider = interface_contracts::ipc::HistoryPage {
        availability: None,
        reason: None,
        events: vec![interface_contracts::engine::EngineEvent {
            at: Some("2000-01-01T00:00:00.000Z".into()),
            event: EngineEventKind::MessageDelta {
                text: "earlier".into(),
                agent_key: None,
            },
        }],
        messages: Some(1),
        older_messages: 0,
    };
    let merged = {
        use crate::dispatcher::HistoryFallback as _;
        // The worker answered show_visual once the entry was written.
        harness.host.with_visuals(CALLER, provider).await
    };
    assert_eq!(merged.events.len(), 2);
    assert!(matches!(
        &merged.events[1].event,
        EngineEventKind::VisualShown { id: shown_id, .. } if *shown_id == id
    ));

    // Deleting the session removes its pages with its journal.
    harness.host.journal_removed(CALLER);
    harness.host.drain_journal().await;
    assert_eq!(read_visual(&harness, CALLER, &id).await, None);
}

/// Every file under `root`, recursively, with its bytes.
fn every_file(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() {
                found.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    found
}

fn holds(bytes: &[u8], value: &str) -> bool {
    bytes
        .windows(value.len())
        .any(|window| window == value.as_bytes())
}

/// Waits (at most five seconds) until `key`'s view carries an open secret
/// request, and answers it.
async fn open_secret_request(
    host: &ThreadHost,
    key: &str,
) -> interface_contracts::thread_secrets::ThreadSecretRequest {
    for _ in 0..5_000 {
        let view = host.view().await.unwrap();
        if let Some(request) = view
            .threads
            .iter()
            .find(|thread| thread.key == key)
            .and_then(|thread| thread.secret_request.clone())
        {
            return request;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("no secret request opened on {key}");
}

/// The private secret card (protocol 67, T3 section 12): `request_secret`
/// opens one request on the thread's view and waits; the owner's Give
/// writes the value to one owner-only file and the tool answers only its
/// path. The value then exists in exactly one place: not in the tool's
/// result, the Give answer, any pushed frame (views and events), the
/// journal, any store file under the data root, or the host log (every
/// level, as this thread logs it). A second request while one waits is
/// refused; Decline answers `{declined: true}`; the thread's removal takes
/// its secrets.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One flow: ask, give, search everywhere, decline, remove.
async fn a_given_secret_exists_only_in_its_owner_only_file() {
    use std::os::unix::fs::PermissionsExt as _;
    const VALUE: &str = "sk_test_51HunterTwoValue_9xQ";
    // The host log, at every level, for everything logged on this thread
    // (the test runtime runs every task here).
    let logs = tempfile::tempdir().unwrap();
    let sink = crate::host_log::HostLogSink::default();
    let log = sink.attach(&logs.path().join("Interface")).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(sink.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    // Proves the capture: this line must reach the file.
    tracing::trace!("secret test: the host log is captured");

    let mut team = team().await;
    let data_root = team.harness.state.path().to_path_buf();
    team.harness.host.attach_secrets(&data_root);
    let host = team.harness.host.clone();
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                CALLER,
                "request_secret",
                json!({"name": "STRIPE_TEST_KEY", "reason": "To run the payment tests."}),
            )
            .await
        }
    });
    let request = open_secret_request(&host, CALLER).await;
    assert_eq!(
        (request.name.as_str(), request.agent.as_str()),
        ("STRIPE_TEST_KEY", "Claude")
    );
    assert_eq!(request.reason, "To run the payment tests.");
    // One open request per thread.
    let second = team
        .call(
            CALLER,
            "request_secret",
            json!({"name": "OTHER", "reason": "More."}),
        )
        .await
        .unwrap_err();
    assert_eq!(second["code"], crate::threads::SECRET_PENDING);
    for bad in [
        json!({"name": "../x", "reason": "r"}),
        json!({"name": "OK", "reason": "two\nlines"}),
    ] {
        let refused = team.call(CALLER, "request_secret", bad).await.unwrap_err();
        assert_eq!(refused["code"], "invalid-input");
    }

    // A Give for another request id changes nothing.
    let stale = host
        .dispatch(
            CHANNELS.thread_secret,
            json!({"kind": "give", "key": CALLER, "id": "secret-999", "value": VALUE}),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, crate::threads::THREAD_SECRET_REFUSED_CODE);
    assert!(stale.replay_safe && !stale.message.contains(VALUE));
    let given = host
        .dispatch(
            CHANNELS.thread_secret,
            json!({"kind": "give", "key": CALLER, "id": request.id, "value": VALUE}),
        )
        .await
        .unwrap();
    let result = asked.await.unwrap().unwrap();
    let answer: Value = serde_json::from_str(&result).unwrap();
    let path = std::path::PathBuf::from(answer["path"].as_str().unwrap());
    assert_eq!(answer["note"], crate::threads::SECRET_NOTE);
    assert_eq!(
        answer.as_object().unwrap().len(),
        2,
        "only the path and the note: {answer}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), VALUE);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(path.starts_with(&data_root));
    assert_eq!(given["secrets"][0]["name"], "STRIPE_TEST_KEY");
    assert_eq!(given["secrets"][0]["key"], CALLER);
    // The journal records the call and its result as the provider reports
    // them: the name, the reason and the path, never the value.
    team.write(
        CALLER,
        JournalEntry::ToolCall {
            call_id: "toolu_secret".into(),
            tool: "mcp__iface_team__request_secret".into(),
            summary: "STRIPE_TEST_KEY".into(),
            input: json!({"name": "STRIPE_TEST_KEY", "reason": "To run the payment tests."}),
            agent_key: None,
            cut: false,
        },
    );
    team.write(
        CALLER,
        JournalEntry::ToolResult {
            call_id: "toolu_secret".into(),
            ok: true,
            summary: result.clone(),
            agent_key: None,
        },
    );
    host.view().await.unwrap();

    // Exactly one place holds the value: its own file.
    assert!(!holds(result.as_bytes(), VALUE), "the tool's result");
    assert!(
        !holds(given.to_string().as_bytes(), VALUE),
        "the Give answer"
    );
    let mut frames = 0;
    while let Ok(frame) = team.harness.pushes.try_recv() {
        frames += 1;
        let encoded = serde_json::to_string(&crate::protocol::HostFrame::Push(frame)).unwrap();
        assert!(
            !holds(encoded.as_bytes(), VALUE),
            "a pushed frame: {encoded}"
        );
    }
    assert!(frames > 0, "the request reached the view as a push");
    let holders = every_file(&data_root)
        .into_iter()
        .filter(|(_, bytes)| holds(bytes, VALUE))
        .map(|(file, _)| file)
        .collect::<Vec<_>>();
    assert_eq!(holders, [path.clone()], "the data root's files");
    assert!(
        every_file(&data_root)
            .iter()
            .any(|(file, _)| file.starts_with(data_root.join("thread-journal"))),
        "the journal was searched"
    );
    let logged = std::fs::read(&log).unwrap();
    assert!(
        holds(&logged, "the host log is captured"),
        "the host log was written"
    );
    assert!(!holds(&logged, VALUE), "the host log");

    // Declined: no file, and the agent is told so.
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                CALLER,
                "request_secret",
                json!({"name": "DB_PASSWORD", "reason": "To migrate."}),
            )
            .await
        }
    });
    let request = open_secret_request(&host, CALLER).await;
    host.dispatch(
        CHANNELS.thread_secret,
        json!({"kind": "decline", "key": CALLER, "id": request.id}),
    )
    .await
    .unwrap();
    let declined: Value = serde_json::from_str(&asked.await.unwrap().unwrap()).unwrap();
    assert_eq!(declined, json!({"declined": true}));
    assert!(!path.with_file_name("DB_PASSWORD").exists());
    let view = host.view().await.unwrap();
    assert!(
        view.threads
            .iter()
            .all(|thread| thread.secret_request.is_none())
    );

    // Forget, and removal with the thread.
    let listed = host
        .dispatch(CHANNELS.thread_secret, json!({"kind": "list"}))
        .await
        .unwrap();
    assert_eq!(listed["secrets"].as_array().unwrap().len(), 1);
    host.secrets_removed(CALLER);
    assert!(!path.exists() && !path.parent().unwrap().exists());
    // A payload that does not decode is refused without echoing it.
    let malformed = host
        .dispatch(
            CHANNELS.thread_secret,
            json!({"kind": "give", "key": CALLER, "id": 7, "value": VALUE}),
        )
        .await
        .unwrap_err();
    assert!(!malformed.message.contains(VALUE));
    assert!(!holds(&std::fs::read(&log).unwrap(), VALUE), "the host log");
}

/// Waits (at most five seconds) until `key`'s view carries no open secret
/// request; answers whether it went.
async fn secret_request_gone(host: &ThreadHost, key: &str) -> bool {
    for _ in 0..5_000 {
        let view = host.view().await.unwrap();
        if view
            .threads
            .iter()
            .filter(|thread| thread.key == key)
            .all(|thread| thread.secret_request.is_none())
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    false
}

/// A Give for a withdrawn request is refused as a stale one is, and no file
/// is left for an agent that was told nothing.
async fn give_is_refused_and_writes_nothing(
    host: &ThreadHost,
    data_root: &std::path::Path,
    id: &str,
) {
    const VALUE: &str = "withdrawn_secret_value_7Qz";
    let refused = host
        .dispatch(
            CHANNELS.thread_secret,
            json!({"kind": "give", "key": CALLER, "id": id, "value": VALUE}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, crate::threads::THREAD_SECRET_REFUSED_CODE);
    assert!(
        every_file(data_root)
            .iter()
            .all(|(_, bytes)| !holds(bytes, VALUE)),
        "a file holds a secret nobody asked for"
    );
}

/// The thread may ask again at once: a new request opens (and is declined
/// here, to finish the call).
async fn a_new_secret_request_opens(team: &Team) {
    let host = team.harness.host.clone();
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                CALLER,
                "request_secret",
                json!({"name": "NEXT_KEY", "reason": "The next step."}),
            )
            .await
        }
    });
    let request = open_secret_request(&host, CALLER).await;
    assert_eq!(request.name, "NEXT_KEY");
    host.dispatch(
        CHANNELS.thread_secret,
        json!({"kind": "decline", "key": CALLER, "id": request.id}),
    )
    .await
    .unwrap();
    let declined: Value = serde_json::from_str(&asked.await.unwrap().unwrap()).unwrap();
    assert_eq!(declined, json!({"declined": true}));
}

/// The caller's team tools socket, bound and confirmed, and one client
/// connection to it that has asked for `SECRET_KEY` as request id 1.
async fn secret_asked_over_the_socket(
    team: &Team,
    base: &std::path::Path,
) -> (
    Arc<dyn interface_contracts::engine::CollaborationMcpSessionBinding>,
    tokio::net::unix::OwnedReadHalf,
    tokio::net::unix::OwnedWriteHalf,
    interface_contracts::thread_secrets::ThreadSecretRequest,
) {
    use tokio::io::AsyncWriteExt as _;
    let servers =
        crate::team_tools::TeamToolServers::with_base(team.harness.host.clone(), base.into());
    let binding = servers
        .create(CollaborationMcpSessionRequest {
            engine: "claude".parse().unwrap(),
            provisional_session_id: "caller".into(),
            resumed: false,
        })
        .await
        .unwrap()
        .unwrap();
    binding
        .confirm_source_session_key(CALLER.into())
        .await
        .unwrap();
    // This test process connects below, so it plays the session's provider.
    binding.provider_process_started(std::process::id()).await;
    let stream = tokio::net::UnixStream::connect(&binding.descriptor().args[1])
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "request_secret",
        "arguments": {"name": "SECRET_KEY", "reason": "To deploy."}}});
    writer
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    let request = open_secret_request(&team.harness.host, CALLER).await;
    assert_eq!(request.name, "SECRET_KEY");
    (binding, reader, writer, request)
}

/// An agent that asked for a secret and then went (its process ended, so
/// its team tools connection closed) no longer asks: the card goes, a Give
/// for it writes nothing, and the thread can ask again at once instead of
/// being told `secret-pending` for up to ten minutes.
#[tokio::test]
async fn a_pending_secret_is_withdrawn_when_its_connection_closes() {
    let team = team().await;
    let data_root = team.harness.state.path().to_path_buf();
    team.harness.host.attach_secrets(&data_root);
    let base = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/private/tmp")
        .unwrap();
    let (binding, reader, writer, request) = secret_asked_over_the_socket(&team, base.path()).await;
    drop((reader, writer));
    assert!(
        secret_request_gone(&team.harness.host, CALLER).await,
        "the card outlived the connection that asked"
    );
    give_is_refused_and_writes_nothing(&team.harness.host, &data_root, &request.id).await;
    a_new_secret_request_opens(&team).await;
    binding.dispose().await.unwrap();
}

/// `notifications/cancelled` for the request (an interrupted tool call)
/// withdraws it the same way, while the connection stays open; a call the
/// client did not cancel is untouched.
#[tokio::test]
async fn a_pending_secret_is_withdrawn_when_the_client_cancels_its_call() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let team = team().await;
    let data_root = team.harness.state.path().to_path_buf();
    team.harness.host.attach_secrets(&data_root);
    let base = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/private/tmp")
        .unwrap();
    let (binding, reader, mut writer, request) =
        secret_asked_over_the_socket(&team, base.path()).await;
    // Another request id's cancellation changes nothing.
    let other = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 2, "reason": "not this one"}});
    writer
        .write_all(format!("{other}\n").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        open_secret_request(&team.harness.host, CALLER).await.id,
        request.id
    );
    let cancelled = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 1, "reason": "The user interrupted."}});
    writer
        .write_all(format!("{cancelled}\n").as_bytes())
        .await
        .unwrap();
    assert!(
        secret_request_gone(&team.harness.host, CALLER).await,
        "the card outlived the call the client cancelled"
    );
    give_is_refused_and_writes_nothing(&team.harness.host, &data_root, &request.id).await;
    a_new_secret_request_opens(&team).await;
    // The connection still serves: the cancelled call answered declined.
    let mut lines = tokio::io::BufReader::new(reader).lines();
    let answered: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(answered["id"], 1);
    binding.dispose().await.unwrap();
}

/// The asking thread's turn ends (stopped, failed or finished) while its
/// request waits: nobody in that turn can take the answer, so it goes.
#[tokio::test]
async fn a_pending_secret_is_withdrawn_when_its_turn_ends() {
    let team = team().await;
    let data_root = team.harness.state.path().to_path_buf();
    team.harness.host.attach_secrets(&data_root);
    let host = team.harness.host.clone();
    host.observe_run(
        CALLER,
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                CALLER,
                "request_secret",
                json!({"name": "SECRET_KEY", "reason": "To deploy."}),
            )
            .await
        }
    });
    host.observe_run(CALLER, &secret_tool_call("toolu_1"));
    let request = open_secret_request(&host, CALLER).await;
    host.observe_run(
        CALLER,
        &EngineEventKind::MessageDelta {
            text: "stopping".into(),
            agent_key: None,
        },
    );
    host.observe_run(
        CALLER,
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), asked)
        .await
        .expect("the request still waits after its turn ended");
    let declined: Value = serde_json::from_str(&answered.unwrap().unwrap()).unwrap();
    assert_eq!(declined, json!({"declined": true}));
    assert!(secret_request_gone(&host, CALLER).await);
    give_is_refused_and_writes_nothing(&host, &data_root, &request.id).await;
    a_new_secret_request_opens(&team).await;
}

/// The `request_secret` tool call as the asking session's events show it.
pub(super) fn secret_tool_call(call_id: &str) -> EngineEventKind {
    EngineEventKind::ToolCall {
        call_id: call_id.into(),
        tool: "mcp__iface_team__request_secret".into(),
        summary: "SECRET_KEY".into(),
        input: json!({"name": "SECRET_KEY", "reason": "To deploy."}),
        agent_key: None,
    }
}

/// The ended turn's events can reach the host after its next turn's call
/// (they come down another pipe). A request the NEXT turn made survives the
/// previous turn's end arriving late, and its own turn's end withdraws it.
#[tokio::test]
async fn a_late_end_of_the_previous_turn_never_withdraws_the_next_turns_secret() {
    let team = team().await;
    let data_root = team.harness.state.path().to_path_buf();
    team.harness.host.attach_secrets(&data_root);
    let host = team.harness.host.clone();
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    // Turn N runs and finishes; its end has not reached the host yet.
    host.observe_run(
        CALLER,
        &EngineEventKind::MessageDelta {
            text: "turn N".into(),
            agent_key: None,
        },
    );
    // Turn N+1 asks for a secret: the call arrives first.
    let asked = tokio::spawn({
        let host = host.clone();
        async move {
            host.team_call(
                CALLER,
                "request_secret",
                json!({"name": "SECRET_KEY", "reason": "To deploy."}),
            )
            .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // Then the stream: N's end, then N+1's call.
    host.observe_run(CALLER, &idle);
    host.observe_run(CALLER, &secret_tool_call("toolu_next"));
    let request = open_secret_request(&host, CALLER).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        open_secret_request(&host, CALLER).await.id,
        request.id,
        "the previous turn's end withdrew the next turn's request"
    );
    assert!(!asked.is_finished());
    // Its own turn's end withdraws it.
    host.observe_run(CALLER, &idle);
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), asked)
        .await
        .expect("the request still waits after its own turn ended");
    let declined: Value = serde_json::from_str(&answered.unwrap().unwrap()).unwrap();
    assert_eq!(declined, json!({"declined": true}));
    assert!(secret_request_gone(&host, CALLER).await);
    give_is_refused_and_writes_nothing(&host, &data_root, &request.id).await;
}

/// Side questions (protocol 67, T3 section 12): `sideAsk` forks the thread
/// at its latest turn into a side thread that no inbox lists (no record,
/// hidden from Chat), runs it read-only (Claude's plan mode) with no team
/// tools, and sends it the question through the guarded runtime; one at a
/// time per thread; closing removes the side thread, and so does removing
/// its thread. A provider that cannot fork refuses.
#[tokio::test]
async fn a_side_question_forks_the_thread_hidden_and_read_only_and_goes_when_closed() {
    use crate::dispatcher::TeamChildren;
    use crate::session_management::RemovalHooks as _;
    const SIDE: &str = "claude#side-1";
    const QUESTION: &str = "Which test covers the login timeout?";
    let team = team().await;
    let host = team.harness.host.clone();
    host.attach_sides(ThreadHost::open_side_store(team.harness.state.path()));
    *lock(&team.harness.sessions.next_fork) = Some(SIDE.into());
    let asked = team
        .harness
        .command(json!({"kind": "sideAsk", "key": CALLER, "question": QUESTION}))
        .await
        .unwrap();
    assert_eq!(asked.key.as_deref(), Some(SIDE));
    // Forked at the thread's latest turn, and asked through the guarded runtime.
    assert_eq!(lock(&team.harness.sessions.forks).last().unwrap().0, CALLER);
    assert!(
        lock(&team.runtime.sends)
            .iter()
            .any(|(key, text, _)| key == SIDE && text == QUESTION)
    );
    // Read-only, no team tools: plan mode, and it resumes one hop down.
    assert_eq!(team.access.mode(SIDE).as_deref(), Some("plan"));
    assert!(TeamChildren::is_team_child(&host, SIDE));
    assert!(!host.team_offered(Some(SIDE)).await);
    // Not a thread: no record, hidden from Chat, even after a reconcile.
    host.reconcile().await;
    let view = host.view().await.unwrap();
    assert!(view.threads.iter().all(|thread| thread.key != SIDE));
    assert!(view.hidden_keys.contains(&SIDE.to_owned()));
    let side = view
        .threads
        .iter()
        .find(|thread| thread.key == CALLER)
        .and_then(|thread| thread.side.clone())
        .unwrap();
    assert_eq!(
        (side.key.as_str(), side.question.as_str()),
        (SIDE, QUESTION)
    );
    // One at a time.
    *lock(&team.harness.sessions.next_fork) = Some("claude#side-x".into());
    let again = team
        .harness
        .command(json!({"kind": "sideAsk", "key": CALLER, "question": "And another?"}))
        .await
        .unwrap_err();
    assert_eq!(again.code, crate::threads::THREADS_SIDE_REFUSED_CODE);
    assert!(again.replay_safe);

    // Closing removes the side thread, its mode and its question.
    team.harness
        .command(json!({"kind": "sideClose", "key": CALLER}))
        .await
        .unwrap();
    assert!(
        lock(&team.harness.sessions.removals).contains(&(SIDE.to_owned(), true)),
        "the side thread's session is deleted"
    );
    assert_eq!(team.access.mode(SIDE), None);
    assert!(!TeamChildren::is_team_child(&host, SIDE));
    let view = host.view().await.unwrap();
    assert!(view.threads.iter().all(|thread| thread.side.is_none()));

    // Removed with its thread.
    *lock(&team.harness.sessions.next_fork) = Some("claude#side-2".into());
    team.harness
        .command(json!({"kind": "sideAsk", "key": CALLER, "question": QUESTION}))
        .await
        .unwrap();
    host.session_removed(CALLER);
    settle_tasks(&host).await;
    assert!(lock(&team.harness.sessions.removals).contains(&("claude#side-2".to_owned(), true)));

    // A provider that cannot fork a thread refuses before anything happens.
    let grok = team
        .harness
        .command(json!({"kind": "sideAsk", "key": "acp:grok#g1", "question": QUESTION}))
        .await
        .unwrap_err();
    assert_eq!(grok.code, crate::threads::THREADS_SIDE_REFUSED_CODE);
    assert!(grok.message.contains("Claude and Codex"));
}
#[path = "threads_outside_tests.rs"]
mod outside;

/// The launch of a new thread as the renderer makes it, up to the moment
/// before its bind: a prepared workspace in the project, then the session
/// (listed, and seeded by a reconcile without its workspace).
async fn launched_unbound(team: &Team, key: &str) -> String {
    let prepared = team
        .harness
        .command(json!({
            "kind": "prepareWorkspace", "projectRoot": team.root, "mode": "local", "hint": "fresh",
        }))
        .await
        .unwrap()
        .prepared
        .unwrap();
    let mut summary = row(key, SessionKind::Code);
    summary.cwd.clone_from(&prepared.cwd);
    team.harness.sessions.upsert(summary);
    team.harness.host.reconcile().await;
    let document = team.harness.document().await;
    assert!(
        document
            .record(key)
            .is_none_or(|record| record.workspace.is_none()),
        "not bound yet"
    );
    prepared.token
}

/// A new thread's first prompt can make its agent call a team tool before
/// the launch's bind has filed the thread with its workspace (the renderer
/// creates the session, then binds it). The call waits for that bind and
/// then answers as for any thread of the project.
#[tokio::test]
async fn a_team_call_before_its_threads_bind_waits_for_the_bind() {
    let team = team().await;
    let key = "claude#fresh";
    let token = launched_unbound(&team, key).await;
    let parked = Arc::clone(&team.harness.host.inner.bind_waits);
    let host = team.harness.host.clone();
    let mut call =
        tokio::spawn(async move { host.team_call(key, "list_threads", json!({})).await });
    tokio::select! {
        () = parked.notified() => {}
        answered = &mut call => panic!(
            "the call answered before its thread was bound: {:?}",
            answered.unwrap()
        ),
    }
    team.harness
        .command(json!({"kind": "bind", "key": key, "token": token, "prompt": "team start x"}))
        .await
        .unwrap();
    let listed: Value = serde_json::from_str(
        &tokio::time::timeout(std::time::Duration::from_secs(30), call)
            .await
            .expect("the call answers once the thread is bound")
            .unwrap()
            .expect("a bound thread's call succeeds"),
    )
    .unwrap();
    assert!(
        listed["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| thread["thread"] == CALLER),
        "{listed}"
    );
}

/// No bind comes: the call is refused as before, after the 10-second bound.
/// A session no launch is waiting to bind (no prepared workspace for its
/// folder, as for a plain chat) is refused at once.
#[tokio::test(start_paused = true)]
async fn a_team_call_whose_bind_never_comes_is_refused_after_the_bound() {
    let team = team().await;
    let key = "claude#orphan";
    launched_unbound(&team, key).await;
    let started = tokio::time::Instant::now();
    let refused: Value = serde_json::from_str(
        &team
            .harness
            .host
            .team_call(key, "start_thread", json!({"task": "x"}))
            .await
            .unwrap_err(),
    )
    .unwrap();
    assert_eq!(refused["code"], "no-project", "{refused}");
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(10),
        "it waited for the bind: {:?}",
        started.elapsed()
    );
    assert!(lock(&team.runtime.creates).is_empty());

    let chat = "claude#chat";
    let mut summary = row(chat, SessionKind::Code);
    summary.cwd = team.harness.dir("elsewhere");
    team.harness.sessions.upsert(summary);
    team.harness.host.reconcile().await;
    let started = tokio::time::Instant::now();
    let refused: Value = serde_json::from_str(
        &team
            .harness
            .host
            .team_call(chat, "start_thread", json!({"task": "x"}))
            .await
            .unwrap_err(),
    )
    .unwrap();
    assert_eq!(refused["code"], "no-project", "{refused}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "nothing waits for a session no launch will bind: {:?}",
        started.elapsed()
    );
}

/// A project picked through a symlinked folder (on macOS, `/var` and
/// `/tmp` are links to `/private/...`): the workspace is prepared at the
/// path as picked, while the provider reports its real folder, so the
/// session's folder and the prepared workspace's differ in spelling only.
/// The new thread's first team tool call still waits for the bind.
#[tokio::test]
async fn a_team_call_before_its_bind_waits_when_the_project_path_is_a_link() {
    let team = team().await;
    let linked = team.harness.missing_dir("linked-atlas");
    std::os::unix::fs::symlink(&team.root, &linked).unwrap();
    let key = "claude#linked";
    let prepared = team
        .harness
        .command(json!({
            "kind": "prepareWorkspace", "projectRoot": linked, "mode": "local", "hint": "fresh",
        }))
        .await
        .unwrap()
        .prepared
        .unwrap();
    let mut summary = row(key, SessionKind::Code);
    // The provider runs in the folder's real path.
    summary.cwd = std::fs::canonicalize(&prepared.cwd)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_ne!(
        summary.cwd, prepared.cwd,
        "the session's folder is spelled another way"
    );
    team.harness.sessions.upsert(summary);
    team.harness.host.reconcile().await;
    let parked = Arc::clone(&team.harness.host.inner.bind_waits);
    let host = team.harness.host.clone();
    let mut call =
        tokio::spawn(async move { host.team_call(key, "list_threads", json!({})).await });
    tokio::select! {
        () = parked.notified() => {}
        answered = &mut call => panic!(
            "the call answered before its thread was bound: {:?}",
            answered.unwrap()
        ),
    }
    team.harness
        .command(
            json!({"kind": "bind", "key": key, "token": prepared.token, "prompt": "team start x"}),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), call)
        .await
        .expect("the call answers once the thread is bound")
        .unwrap()
        .expect("a bound thread's call succeeds");
}

/// Calls that arrive together (an agent can make several tool calls at
/// once) never pass the running limit together: of six starts at once, four
/// start and the rest are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starts_at_once_never_pass_the_running_limit_together() {
    let team = team().await;
    let calls = (0..6)
        .map(|index| {
            let host = team.harness.host.clone();
            tokio::spawn(async move {
                host.team_call(
                    CALLER,
                    "start_thread",
                    json!({"task": format!("job {index}")}),
                )
                .await
            })
        })
        .collect::<Vec<_>>();
    let mut started = 0;
    let mut refused = Vec::new();
    for call in calls {
        match call.await.unwrap() {
            Ok(_) => started += 1,
            Err(text) => refused.push(serde_json::from_str::<Value>(&text).unwrap()),
        }
    }
    assert_eq!(started, 4, "refused: {refused:?}");
    assert!(
        refused
            .iter()
            .all(|refusal| refusal["code"] == "limit-running"),
        "{refused:?}"
    );
    assert_eq!(lock(&team.runtime.creates).len(), 4);
}

/// Two started threads that finish together: the caller is idle when both
/// notices are decided, but the first notice starts the caller's turn, so
/// the second waits for that turn to end instead of going into it.
#[tokio::test]
async fn notices_of_threads_that_finish_together_go_one_turn_at_a_time() {
    let mut team = team().await;
    let first = team.start(json!({"task": "a", "title": "First"})).await;
    let second = team.start(json!({"task": "b", "title": "Second"})).await;
    team.finish(&first, "One.", RunOutcome::Completed).await;
    team.finish(&second, "Two.", RunOutcome::Completed).await;
    let host = team.harness.host.clone();
    let (one, two) = tokio::join!(host.notice_step(&first), host.notice_step(&second));
    assert_eq!(
        team.sends_to(CALLER).len(),
        1,
        "one notice while the caller's turn runs: {:?} ({one:?}, {two:?})",
        team.sends_to(CALLER)
    );
    // The caller's turn ends: the other notice goes now, as its own turn.
    end_record_run(&team.harness, CALLER).await;
    let waiting = if one == NoticeStep::Wait {
        &first
    } else {
        &second
    };
    assert_eq!(host.notice_step(waiting).await, NoticeStep::Done);
    assert_eq!(team.sends_to(CALLER).len(), 2);
    assert!(team.sends_to(CALLER).iter().all(|(_, echo)| *echo));
}

/// A real process talking to a team tools socket through `/usr/bin/nc -U`,
/// as a provider's MCP server does: requests go in on its stdin, answers
/// come out of its stdout.
struct SocketClient {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
}

impl SocketClient {
    /// `/bin/sh` running `nc` as its child (never exec'd into it), so the
    /// shell plays a provider and `nc` its MCP server. `nested` puts one
    /// more shell between them, as a provider's tool shell running a
    /// command does.
    fn shell(socket: &str, nested: bool) -> Self {
        let script = if nested {
            r#"/bin/sh -c '/usr/bin/nc -U "$0"; exit $?' "$0"; exit $?"#
        } else {
            r#"/usr/bin/nc -U "$0"; exit $?"#
        };
        Self::spawn(tokio::process::Command::new("/bin/sh").args(["-c", script, socket]))
    }

    /// `nc` alone, a child of this test process and of no provider.
    fn nc(socket: &str) -> Self {
        Self::spawn(tokio::process::Command::new("/usr/bin/nc").args(["-U", socket]))
    }

    fn spawn(command: &mut tokio::process::Command) -> Self {
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(
            child.stdout.take().unwrap(),
        ));
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id().unwrap()
    }

    /// One `tools/call`: whether the answer is an error, and its JSON.
    async fn call(&mut self, id: u64, name: &str, arguments: Value) -> (bool, Value) {
        use tokio::io::AsyncWriteExt as _;
        let call = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}});
        self.stdin
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(30), self.lines.next_line())
            .await
            .expect("the socket answers")
            .unwrap()
            .expect("an answer line");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        (
            reply["result"]["isError"] == true,
            serde_json::from_str(text).unwrap(),
        )
    }
}

/// The security review's finding (2026-11-23): a thread's team tools
/// socket answered any process of the owner's as that thread, so a command
/// running in another session (a prompt-injected Ask agent's sandboxed
/// `python3 -c` or `nc`) could start a thread in a Bypass thread's mode
/// with nobody approving anything. Now a socket answers only the provider
/// process its adapter named and the processes under it, by real
/// processes: the provider (a shell) and its MCP server (`nc`) are
/// answered; a command under another session's provider, and an `nc` under
/// no provider at all, are refused and start nothing; once the provider is
/// gone, the `nc` it left behind is refused too. The socket sits in a 0700
/// folder and is itself 0600.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Four callers, one socket.
async fn a_threads_team_tools_answer_only_its_own_provider_and_the_processes_it_started() {
    use std::os::unix::fs::PermissionsExt as _;
    let team = team().await;
    team.access.remember(CALLER, Some("bypassPermissions"));
    let base = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/private/tmp")
        .unwrap();
    let servers = crate::team_tools::TeamToolServers::with_base(
        team.harness.host.clone(),
        base.path().into(),
    );
    let request = |id: &str| CollaborationMcpSessionRequest {
        engine: "claude".parse().unwrap(),
        provisional_session_id: id.into(),
        resumed: false,
    };
    // The Bypass thread whose socket is the target.
    let binding = servers.create(request("caller")).await.unwrap().unwrap();
    binding
        .confirm_source_session_key(CALLER.into())
        .await
        .unwrap();
    let socket = binding.descriptor().args[1].clone();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(std::path::Path::new(&socket)), 0o600);
    assert_eq!(mode(std::path::Path::new(&socket).parent().unwrap()), 0o700);
    // Another session, whose provider runs a command that found the socket.
    let neighbour = servers.create(request("neighbour")).await.unwrap().unwrap();
    neighbour
        .confirm_source_session_key("claude#neighbour".into())
        .await
        .unwrap();

    let mut own = SocketClient::shell(&socket, false);
    binding.provider_process_started(own.pid()).await;
    let mut other_session = SocketClient::shell(&socket, true);
    neighbour
        .provider_process_started(other_session.pid())
        .await;
    let mut stranger = SocketClient::nc(&socket);

    let task = || json!({"task": "PROBE: run anything here"});
    for (who, client) in [
        (
            "a command under another session's provider",
            &mut other_session,
        ),
        ("a process under no provider", &mut stranger),
    ] {
        let (error, answer) = client.call(1, "start_thread", task()).await;
        assert!(error, "{who}: {answer}");
        assert_eq!(answer["code"], "refused", "{who}: {answer}");
        let (error, answer) = client.call(2, "team_info", json!({})).await;
        assert!(error && answer["code"] == "refused", "{who}: {answer}");
    }
    assert!(
        lock(&team.runtime.creates).is_empty(),
        "a refused caller started a thread"
    );

    // The thread's own provider's MCP server is answered as the thread.
    let (error, info) = own.call(1, "team_info", json!({})).await;
    assert!(!error, "{info}");
    assert_eq!(info["you"]["thread"], CALLER);
    let (error, started) = own.call(2, "start_thread", task()).await;
    assert!(!error, "{started}");
    let creates = lock(&team.runtime.creates).clone();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["options"]["permissionMode"], "bypassPermissions");

    // The provider goes; the `nc` it started lives on, orphaned, and is no
    // longer its session's.
    own.child.kill().await.unwrap();
    let (error, answer) = own.call(3, "team_info", json!({})).await;
    assert!(error && answer["code"] == "refused", "{answer}");

    binding.dispose().await.unwrap();
    neighbour.dispose().await.unwrap();
}

/// The security review's own proof, kept as an opt-in probe: `codex
/// sandbox` with an Ask agent's exact sandbox (workspace-write with network
/// on and shared /tmp excluded, as `gate_overrides` asks) runs a command
/// that finds a Bypass thread's team tools socket and asks it to start a
/// thread. The socket is reached and refuses. Needs the Codex CLI, so it is
/// ignored by default; it never runs a model turn and uses a scratch
/// `CODEX_HOME`:
/// `INTERFACE_CODEX_SANDBOX_PROBE=$(command -v codex) cargo test -p
/// interface-host --lib a_command_in_codexs_ask_sandbox -- --ignored`
#[tokio::test]
#[ignore = "needs the Codex CLI: set INTERFACE_CODEX_SANDBOX_PROBE to its path"]
async fn a_command_in_codexs_ask_sandbox_cannot_start_a_thread_through_another_threads_socket() {
    let Some(codex) = std::env::var_os("INTERFACE_CODEX_SANDBOX_PROBE") else {
        eprintln!("INTERFACE_CODEX_SANDBOX_PROBE is not set; nothing was probed");
        return;
    };
    let team = team().await;
    team.access.remember(CALLER, Some("bypassPermissions"));
    let base = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/private/tmp")
        .unwrap();
    let servers = crate::team_tools::TeamToolServers::with_base(
        team.harness.host.clone(),
        base.path().into(),
    );
    let binding = servers
        .create(CollaborationMcpSessionRequest {
            engine: "claude".parse().unwrap(),
            provisional_session_id: "caller".into(),
            resumed: false,
        })
        .await
        .unwrap()
        .unwrap();
    binding
        .confirm_source_session_key(CALLER.into())
        .await
        .unwrap();
    // The Bypass thread's provider is a process of its own: the sandbox
    // below is this test's child, not its.
    let mut provider = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    binding
        .provider_process_started(provider.id().unwrap())
        .await;
    let workspace = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(workspace.path()).unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"start_thread","arguments":{"task":"PROBE: run anything here"}}}"#;
    let script = format!(
        "for s in {base}/iface-tt-*/s.sock; do (printf '%s\\n' '{call}'; sleep 4) | /usr/bin/nc -U \"$s\"; echo; done",
        base = base.path().display()
    );
    let output = tokio::process::Command::new(codex)
        .args([
            "sandbox",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "-c",
            &format!(
                "sandbox_workspace_write.writable_roots=[\"{}\"]",
                workspace.display()
            ),
            "-c",
            "sandbox_workspace_write.network_access=true",
            "-c",
            "sandbox_workspace_write.exclude_slash_tmp=true",
            "--",
            "/bin/sh",
            "-c",
            &script,
        ])
        .current_dir(&workspace)
        .env("CODEX_HOME", codex_home.path())
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("sandboxed stdout:\n{stdout}");
    eprintln!(
        "sandboxed stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let creates = lock(&team.runtime.creates).clone();
    binding.dispose().await.unwrap();
    provider.kill().await.ok();
    assert!(
        creates.is_empty(),
        "a command inside the Ask agent sandbox started a thread: {creates:#?}"
    );
    assert!(
        stdout.contains(r#"\"code\":\"refused\""#),
        "the sandboxed command did not reach the socket to be refused: {stdout}"
    );
}
