use interface_engine::bounded_event_stream;
use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::Ordering};

use interface_contracts::engine::{
    CollaborationMcpStdioDescriptor, LaunchOptions as ContractLaunchOptions,
};
use interface_engine::EffectCertainty;
use interface_engine::test_support::{
    CollaborationTestFactory, CollaborationTestState, TeamToolsTestFactory,
};
use tempfile::tempdir;
use tokio::time::timeout;

use super::*;

fn fake_cli(script: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempdir().expect("temporary directory");
    let executable = directory.path().join("fake-claude");
    fs::write(&executable, script).expect("write fake CLI");
    let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("executable mode");
    (directory, executable)
}

fn capturing_script(args_path: &Path, stdin_path: &Path) -> String {
    format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{args}'
: > '{stdin}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{stdin}'
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"type":"control_request"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#,
        args = args_path.display(),
        stdin = stdin_path.display(),
    )
}

fn initialize_request(stdin_path: &Path) -> Value {
    for line in fs::read_to_string(stdin_path)
        .expect("captured stdin")
        .lines()
    {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value
            .get("request")
            .and_then(|request| request.get("subtype"))
            .and_then(Value::as_str)
            == Some("initialize")
        {
            return value
                .get("request")
                .cloned()
                .expect("initialize request payload");
        }
    }
    panic!("fake CLI did not receive an initialize request");
}

fn consecutive_args(arguments: &str, first: &str, second: &str) -> bool {
    arguments
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == [first, second])
}

fn interactive_script(delay_initialize: bool) -> String {
    let delay = if delay_initialize { "sleep 1\n" } else { "" };
    format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      {delay}      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{{"type":"control_request","request_id":"approval-1","request":{{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"tool-1","input":{{"questions":[]}}}}}}'
      ;;
    *'"type":"control_response"'*)
      printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":2}}}}'
      ;;
    *'"type":"control_request"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#
    )
}

/// Sibling of `interactive_script` that asks for a `Write` instead of an
/// `AskUserQuestion` and records every control response it is handed, so a
/// test can prove the host settled the request itself.
fn write_approval_script(response_path: &Path) -> String {
    format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{{"type":"control_request","request_id":"approval-1","request":{{"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool-1","input":{{"file_path":"probe.txt"}}}}}}'
      ;;
    *'"type":"control_response"'*)
      printf '%s\n' "$line" >> '{response}'
      printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":2}}}}'
      ;;
    *'"type":"control_request"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#,
        response = response_path.display(),
    )
}

/// The socket the shared collaboration fake's bindings name.
const COLLABORATION_SOCKET: &str = "/tmp/claude-collaboration-test.sock";

#[tokio::test]
async fn collaboration_binding_confirms_exact_source_and_adapter_dispose_revokes_it() {
    let (directory, executable) = fake_cli(&interactive_script(false));
    let state = Arc::new(CollaborationTestState::default());
    let factory = CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_collaboration_factory(factory);
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let ready = handle.ready().await.unwrap();
    assert_eq!(
        lock(&state.confirmed).as_slice(),
        [format!("claude#{}", ready.session_id)]
    );
    {
        let requests = lock(&state.requests);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].engine.as_str(), "claude");
        assert!(!requests[0].resumed);
    }

    adapter.dispose().await.unwrap();
    assert_eq!(state.disposed.load(Ordering::Acquire), 1);
    handle.close().await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(state.disposed.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn depth_one_claude_session_never_requests_a_broker_binding() {
    let (directory, executable) = fake_cli(&interactive_script(false));
    let state = Arc::new(CollaborationTestState::default());
    let factory = CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_collaboration_factory(factory);
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            collaboration_depth: Some(1),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    assert!(lock(&state.requests).is_empty());
    handle.close().await.unwrap();
}

struct TrustsOnly(PathBuf);

impl crate::FolderTrust for TrustsOnly {
    fn settings_allowed(&self, cwd: &Path) -> bool {
        cwd == self.0
    }
}

/// Headless, the CLI asks no trust question, so a folder nobody trusted must
/// add nothing of its own: whatever settings a caller asked for, a session
/// started or resumed there gets the user's own and no more, and the
/// folder's `.mcp.json` stays out. A trusted folder is launched as asked.
#[tokio::test]
async fn an_untrusted_folders_own_settings_never_load() {
    let (directory, executable) = fake_cli(&interactive_script(false));
    let trusted = directory.path().join("trusted");
    let hostile = directory.path().join("hostile");
    std::fs::create_dir_all(&trusted).unwrap();
    std::fs::create_dir_all(&hostile).unwrap();
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_folder_trust(Arc::new(TrustsOnly(trusted.clone())));
    let configurations = |folder: &Path, asked: Option<&[&str]>| {
        let cwd = folder.to_string_lossy().into_owned();
        let sources = asked.map(|asked| asked.iter().map(|&source| source.to_owned()).collect());
        let created = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: cwd.clone(),
                prompt: "hi".into(),
                setting_sources: sources.clone(),
                ..ContractLaunchOptions::default()
            })
            .unwrap();
        let resumed = adapter
            .resume_configuration(
                "existing".into(),
                cwd,
                ResumeOptions {
                    setting_sources: sources,
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        [created, resumed].map(|configuration| launch_arguments(&configuration.launch))
    };
    let sources_of = |arguments: &[String]| {
        arguments
            .iter()
            .find_map(|argument| argument.strip_prefix("--setting-sources="))
            .map(str::to_owned)
    };

    let every_way_to_ask: [Option<&[&str]>; 5] = [
        None,
        Some(&["user", "project", "local"]),
        Some(&["project"]),
        Some(&["local", "user"]),
        Some(&[]),
    ];
    for asked in every_way_to_ask {
        for arguments in configurations(&hostile, asked) {
            let sources = sources_of(&arguments).expect("the sources are always spelled out");
            assert!(
                sources.is_empty() || sources == "user",
                "{asked:?} loaded {sources}"
            );
            assert!(arguments.contains(&"--strict-mcp-config".to_owned()));
        }
    }

    for arguments in configurations(&trusted, None) {
        assert_eq!(sources_of(&arguments), None, "the CLI's own default");
    }
    for arguments in configurations(&trusted, Some(&["user", "project", "local"])) {
        assert_eq!(
            sources_of(&arguments).as_deref(),
            Some("user,project,local")
        );
    }
}

/// A gated agent never gets the collaboration MCP: a consult or handoff it starts would launch
/// an ungated target the owner was never told about.
#[tokio::test]
async fn gated_claude_session_never_attaches_the_collaboration_mcp() {
    let (directory, executable) = fake_cli(&interactive_script(false));
    let state = Arc::new(CollaborationTestState::default());
    let factory = CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_collaboration_factory(factory);
    let cwd = directory.path().to_string_lossy().into_owned();
    let created = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: cwd.clone(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            cwd.clone(),
            ResumeOptions {
                kind: Some(SessionKind::Agent),
                approval_gate: Some(true),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    for configuration in [&created, &resumed] {
        assert!(configuration.collaboration_mcp_factory.is_none());
        let arguments = launch_arguments(&configuration.launch);
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("iface_collaboration")),
            "{arguments:?}"
        );
    }
    let ungated = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: cwd.clone(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert!(ungated.collaboration_mcp_factory.is_some());

    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd,
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    assert!(lock(&state.requests).is_empty());
    assert!(lock(&state.confirmed).is_empty());
    handle.close().await.unwrap();
}

#[tokio::test]
async fn readiness_gates_send_then_fake_cli_round_trips_exactly_one_approval() {
    let (directory, executable) = fake_cli(&interactive_script(true));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert!(matches!(
        handle.send("too early".into()).await,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    assert!(handle.set_model(Some("sonnet".into())).await.is_confirmed());
    assert!(matches!(
        events.recv().await.unwrap().unwrap().event,
        EngineEventKind::ConfigChanged { ref model, .. } if model.as_deref() == Some("sonnet")
    ));
    assert!(matches!(
        events.recv().await.unwrap().unwrap().event,
        EngineEventKind::CommandsInvalidated
    ));
    assert!(handle.set_effort(Some("high".into())).await.is_confirmed());
    loop {
        if matches!(
            events.recv().await.unwrap().unwrap().event,
            EngineEventKind::ConfigChanged { ref effort, .. }
                if effort == &Some(Some("high".into()))
        ) {
            break;
        }
    }
    assert!(handle.set_fast_mode(true).await.is_confirmed());
    assert!(matches!(
        events.recv().await.unwrap().unwrap().event,
        EngineEventKind::FastmodeChanged {
            state: FastModeState::On,
            ..
        }
    ));
    assert!(
        handle
            .set_permission_mode("auto".into())
            .await
            .is_confirmed()
    );
    assert!(matches!(
        events.recv().await.unwrap().unwrap().event,
        EngineEventKind::AccessChanged { ref mode } if mode == "auto"
    ));
    assert!(matches!(
        handle.set_permission_mode("bypassPermissions".into()).await,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    assert!(handle.send("ask".into()).await.is_confirmed());

    let approval_id = loop {
        let next = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = next.event {
            break approval_id;
        }
    };
    let response = ApprovalResponse {
        answers: vec![("question-1".into(), "yes".into())],
        ..ApprovalResponse::default()
    };
    assert!(
        handle
            .respond_to_approval(
                approval_id.clone(),
                ApprovalVerdict::Allow,
                response.clone(),
            )
            .await
            .is_confirmed()
    );
    assert!(matches!(
        handle
            .respond_to_approval(approval_id, ApprovalVerdict::Allow, response)
            .await,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    handle.close().await.unwrap();
}

/// Once the owner answers a tool request, Claude goes back to work, but its
/// stream says nothing that leaves `Waiting`: `next_status` keeps it through
/// the turn's tool and message events. Without `Working` here the Dock badge
/// and quit prompt counted the session as waiting for the rest of the turn,
/// and keep-awake let the Mac sleep while the agent was still working.
#[tokio::test]
async fn confirmed_approval_puts_the_session_back_to_working() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"control_request","request_id":"approval-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"npm test"}}}'
      ;;
    *'"type":"control_response"'*)
      printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]},"uuid":"u2"}'
      printf '%s\n' '{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Tests pass, editing the next file"}]},"uuid":"u3"}'
      printf '%s\n' '{"type":"assistant","message":{"id":"m3","role":"assistant","content":[{"type":"tool_use","id":"tool-2","name":"Read","input":{"file_path":"a.txt"}}]},"uuid":"u4"}'
      ;;
    *'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#;
    let (directory, executable) = fake_cli(script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    assert!(handle.send("run the tests".into()).await.is_confirmed());
    let mut status = SessionStatus::Idle;
    let approval_id = loop {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = next.event {
            break approval_id;
        }
    };
    assert_eq!(status, SessionStatus::Waiting);
    assert!(
        handle
            .respond_to_approval(
                approval_id,
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    // The turn goes on: the tool result, more text and the next tool call.
    let mut saw_next_tool = false;
    while let Ok(next) = timeout(Duration::from_millis(500), events.recv()).await {
        let next = next.expect("bounded queue").expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        saw_next_tool |= matches!(next.event, EngineEventKind::ToolCall { .. });
    }
    assert!(saw_next_tool, "the fake CLI went on with its turn");
    assert_eq!(status, SessionStatus::Working);
    handle.close().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn concurrent_approvals_stay_waiting_until_last_request_is_settled_or_cancelled() {
    let script = r#"#!/bin/sh
turn=0
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      turn=$((turn + 1))
      if [ "$turn" -eq 1 ]; then
        printf '%s\n' '{"type":"control_request","request_id":"approval-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"npm test"}}}'
        printf '%s\n' '{"type":"control_request","request_id":"approval-2","request":{"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool-2","input":{"file_path":"a.txt","content":"hi"}}}'
      else
        printf '%s\n' '{"type":"control_request","request_id":"approval-3","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-3","input":{"command":"cargo test"}}}'
        printf '%s\n' '{"type":"control_request","request_id":"approval-4","request":{"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool-4","input":{"file_path":"b.txt","content":"yo"}}}'
      fi
      ;;
    *'"request_id":"approval-1"'*)
      printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]},"uuid":"u2"}'
      ;;
    *'"request_id":"approval-2"'*)
      printf '%s\n' '{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"both approved"}]},"uuid":"u3"}'
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"s1","result":"done"}'
      ;;
    *'"request_id":"approval-3"'*)
      printf '%s\n' '{"type":"control_cancel_request","request_id":"approval-4"}'
      printf '%s\n' '{"type":"assistant","message":{"id":"m3","role":"assistant","content":[{"type":"tool_use","id":"tool-5","name":"Read","input":{"file_path":"c.txt"}}]},"uuid":"u4"}'
      ;;
    *'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#;
    let (directory, executable) = fake_cli(script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();

    // Turn 1: two concurrent approvals must stay `Waiting` after the first is answered.
    assert!(handle.send("first turn".into()).await.is_confirmed());
    let mut status = SessionStatus::Idle;
    let mut seen_approvals = Vec::new();
    while seen_approvals.len() < 2 {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = next.event {
            seen_approvals.push(approval_id);
        }
    }
    assert_eq!(status, SessionStatus::Waiting);
    assert!(
        handle
            .respond_to_approval(
                seen_approvals[0].clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    let mut saw_tool_result = false;
    while let Ok(next) = timeout(Duration::from_millis(250), events.recv()).await {
        let next = next.expect("bounded queue").expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        saw_tool_result |= matches!(next.event, EngineEventKind::ToolResult { .. });
    }
    assert!(saw_tool_result);
    assert_eq!(
        status,
        SessionStatus::Waiting,
        "answering one of two concurrent approvals must not emit Working prematurely"
    );

    assert!(
        handle
            .respond_to_approval(
                seen_approvals[1].clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    let mut statuses = Vec::new();
    while status != SessionStatus::Idle {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        statuses.push(status);
    }
    assert!(
        statuses.contains(&SessionStatus::Working),
        "answering the last pending approval emits Working before turn settles: {statuses:?}"
    );

    // Turn 2: answering approval-3 while Claude cancels approval-4 via
    // `control_cancel_request` clears the last pending request and resumes Working.
    assert!(handle.send("second turn".into()).await.is_confirmed());
    seen_approvals.clear();
    while seen_approvals.len() < 2 {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = next.event {
            seen_approvals.push(approval_id);
        }
    }
    assert_eq!(status, SessionStatus::Waiting);
    assert!(
        handle
            .respond_to_approval(
                seen_approvals[0].clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    let mut saw_follow_on_tool = false;
    while let Ok(next) = timeout(Duration::from_millis(300), events.recv()).await {
        let next = next.expect("bounded queue").expect("event");
        status = interface_contracts::engine::next_status(status, &next);
        saw_follow_on_tool |= matches!(next.event, EngineEventKind::ToolCall { .. });
    }
    assert!(saw_follow_on_tool);
    assert_eq!(
        status,
        SessionStatus::Working,
        "HostRequestCancelled for the remaining request must transition Waiting back to Working"
    );
    handle.close().await.unwrap();
}

#[test]
fn host_permission_decision_matches_the_mode_policy() {
    let directory = tempdir().unwrap();
    let safe_edit = json!({"file_path": "src/lib.rs"});
    let table = [
        (
            PermissionMode::Auto,
            "Write",
            Some(&safe_edit),
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::Auto,
            "Bash",
            None,
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::Auto,
            "AskUserQuestion",
            None,
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::Auto,
            "Write",
            Some(&safe_edit),
            true,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::DontAsk,
            "Bash",
            None,
            false,
            HostPermissionDecision::Deny,
        ),
        (
            PermissionMode::AcceptEdits,
            "Edit",
            Some(&safe_edit),
            false,
            HostPermissionDecision::Allow,
        ),
        (
            PermissionMode::AcceptEdits,
            "Bash",
            None,
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::Default,
            "Write",
            Some(&safe_edit),
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::Plan,
            "Write",
            Some(&safe_edit),
            false,
            HostPermissionDecision::Ask,
        ),
        (
            PermissionMode::BypassPermissions,
            "Bash",
            None,
            false,
            HostPermissionDecision::Allow,
        ),
    ];
    for (mode, tool, input, forced_ask, expected) in table {
        assert_eq!(
            host_permission_decision(mode, tool, input, directory.path(), &[], forced_ask),
            expected,
            "mode {mode} tool {tool} forced_ask {forced_ask}"
        );
    }
}

#[tokio::test]
async fn auto_mode_allows_an_edit_without_parking_it_on_the_user() {
    let directory = tempdir().unwrap();
    let responses = directory.path().join("responses.txt");
    let (_fixture, executable) = fake_cli(&write_approval_script(&responses));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "write it".into(),
            permission_mode: Some("acceptEdits".into()),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    loop {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        assert!(
            !matches!(next.event, EngineEventKind::ApprovalRequested { .. }),
            "acceptEdits parked a contained workspace edit on the user"
        );
        if matches!(
            next.event,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            }
        ) {
            break;
        }
    }
    let recorded = fs::read_to_string(&responses).expect("recorded control responses");
    let settled = recorded
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["response"]["request_id"] == json!("approval-1"))
        .expect("a control response for approval-1");
    assert_eq!(settled["response"]["subtype"], json!("success"));
    assert_eq!(settled["response"]["response"]["behavior"], json!("allow"));
    handle.close().await.unwrap();
}

#[tokio::test]
async fn close_before_ready_never_promotes_the_handle() {
    let (directory, executable) = fake_cli(&interactive_script(true));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.close().await.unwrap();
    let error = handle.ready().await.unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
}

#[tokio::test]
async fn resume_drops_bypass_but_preserves_explicit_setting_isolation() {
    let directory = tempdir().unwrap();
    let arguments = directory.path().join("args.txt");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#,
        arguments.display()
    );
    let (_fixture, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    lock(&adapter.permission_history).insert("existing".into(), PermissionMode::BypassPermissions);
    let mut handle = adapter
        .resume_session(
            "existing".into(),
            directory.path().to_string_lossy().into_owned(),
            ResumeOptions {
                setting_sources: Some(Vec::new()),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    let first = events.recv().await.unwrap().unwrap();
    assert!(matches!(first.event, EngineEventKind::Notice { .. }));
    let second = events.recv().await.unwrap().unwrap();
    assert!(
        matches!(second.event, EngineEventKind::AccessChanged { ref mode } if mode == "default")
    );

    let rendered = fs::read_to_string(arguments).unwrap();
    assert!(rendered.lines().any(|line| line == "--permission-mode"));
    assert!(rendered.lines().any(|line| line == "default"));
    assert!(rendered.lines().any(|line| line == "--setting-sources="));
    assert!(rendered.lines().any(|line| line == "--strict-mcp-config"));
    assert!(!rendered.contains("--allow-dangerously-skip-permissions"));
    handle.close().await.unwrap();
}

/// 7 Oct 2026: a thread started in Auto came back in "default" after an app
/// restart and asked before every tool. The host keeps the mode its owner chose
/// and hands it to the resume, which launches in it with no dropped-mode notice;
/// a token Claude does not know still resumes in the default.
#[tokio::test]
async fn resume_applies_the_mode_the_owner_chose_for_the_session() {
    for (requested, expected) in [("auto", "auto"), ("not-a-mode", "default")] {
        let directory = tempdir().unwrap();
        let arguments = directory.path().join("args.txt");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#,
            arguments.display()
        );
        let (_fixture, executable) = fake_cli(&script);
        // A fresh adapter, as after a restart: no mode was seen this launch.
        let adapter = ClaudeAdapter::new(executable).unwrap();
        let mut handle = adapter
            .resume_session(
                "existing".into(),
                directory.path().to_string_lossy().into_owned(),
                ResumeOptions {
                    permission_mode: Some(requested.into()),
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        handle.ready().await.unwrap();
        let mut events = handle.take_events().unwrap();
        let first = events.recv().await.unwrap().unwrap();
        assert!(
            matches!(first.event, EngineEventKind::AccessChanged { ref mode } if mode == expected),
            "{requested}: {:?}",
            first.event
        );

        let rendered = fs::read_to_string(arguments).unwrap();
        let lines: Vec<&str> = rendered.lines().collect();
        let flag = lines
            .iter()
            .position(|line| *line == "--permission-mode")
            .unwrap();
        assert_eq!(lines[flag + 1], expected);
        handle.close().await.unwrap();
    }
}

#[tokio::test]
async fn command_discovery_answers_without_waiting_for_the_control_cli_to_exit() {
    // After stdin EOF this CLI lingers well past the graceful-shutdown window,
    // as a CLI tearing down MCP servers or plugins can.
    let (_fixture, executable) = fake_cli(
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[{"name":"review","description":"Review"}]}}}\n' "$id"
      ;;
  esac
done
sleep 4
"#,
    );
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let cwd = tempdir().unwrap();
    let started = std::time::Instant::now();
    let result = adapter
        .list_provider_commands(ProviderCommandRequest {
            cwd: cwd.path().to_string_lossy().into_owned(),
            model: None,
            setting_sources: Some(Vec::new()),
        })
        .await
        .unwrap();
    assert!(result.available);
    assert_eq!(result.commands[0].name, "review");
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "discovery waited {:?} for the CLI to exit",
        started.elapsed()
    );
}

#[tokio::test]
async fn fake_cli_chat_is_tool_less_isolated_and_refuses_permission_changes() {
    let directory = tempdir().unwrap();
    let arguments = directory.path().join("args.txt");
    let stdin = directory.path().join("stdin.txt");
    let (_fixture, executable) = fake_cli(&capturing_script(&arguments, &stdin));
    let state = Arc::new(CollaborationTestState::default());
    let factory = CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_collaboration_factory(factory);
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Chat),
            permission_mode: Some("acceptEdits".into()),
            setting_sources: Some(vec!["user".into()]),
            allowed_tools: Some(vec!["Read".into()]),
            disallowed_tools: Some(vec!["Bash".into()]),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let rendered = fs::read_to_string(&arguments).unwrap();
    assert!(consecutive_args(&rendered, "--tools", ""));
    assert!(!rendered.lines().any(|line| line == "--mcp-config"));
    assert!(consecutive_args(&rendered, "--permission-mode", "default"));
    assert!(rendered.lines().any(|line| line == "--setting-sources="));
    assert!(rendered.lines().any(|line| line == "--strict-mcp-config"));
    let initialized = initialize_request(&stdin);
    assert_eq!(initialized["systemPrompt"], json!([CHAT_SYSTEM_PROMPT]));
    assert!(initialized.get("appendSystemPrompt").is_none());
    assert!(lock(&state.requests).is_empty());
    let refused = handle.set_permission_mode("auto".into()).await;
    let DeliveryOutcome::DefinitelyUnsent(error) = refused else {
        panic!("chat SetPermissionMode reached the provider");
    };
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    assert_eq!(error.message, CHAT_PERMISSION_REFUSAL);
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    handle.close().await.unwrap();
}

/// The socket the team tools fake's bindings name.
const TEAM_SOCKET: &str = "/tmp/iface-tt-test/s.sock";

/// Team tools (protocol 57): an ordinary code session's CLI gets the
/// `iface_team` server, confirmed for its exact key before launch, and it is
/// revoked when the session ends. Chat, Voice, agents, gated sessions and
/// sessions one hop down (collaboration depth 1, as a started thread runs)
/// never ask for it; a session the host does not offer it to has none.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One launch, every refused shape, then the setting off.
async fn fake_cli_code_session_gets_team_tools_exactly_when_it_should() {
    let directory = tempdir().unwrap();
    let arguments = directory.path().join("args.txt");
    let stdin = directory.path().join("stdin.txt");
    let (_fixture, executable) = fake_cli(&capturing_script(&arguments, &stdin));
    let state = Arc::new(CollaborationTestState::default());
    let adapter = ClaudeAdapter::new(executable.clone())
        .unwrap()
        .with_team_tools(TeamToolsTestFactory::shared(&state, TEAM_SOCKET, true));
    let cwd = directory.path().to_string_lossy().into_owned();
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: cwd.clone(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let ready = handle.ready().await.unwrap();
    let rendered = fs::read_to_string(&arguments).unwrap();
    let config = rendered
        .lines()
        .skip_while(|line| *line != "--mcp-config")
        .nth(1)
        .expect("an --mcp-config");
    let config: Value = serde_json::from_str(config).unwrap();
    assert_eq!(
        config["mcpServers"]["iface_team"],
        json!({"type": "stdio", "command": "/usr/bin/nc", "args": ["-U", TEAM_SOCKET], "env": {}})
    );
    assert_eq!(
        lock(&state.confirmed).as_slice(),
        [format!("claude#{}", ready.session_id)]
    );
    assert!(!lock(&state.requests)[0].resumed);
    // The CLI's own process is named to the binding (the host answers only
    // it and the processes it starts).
    let providers = lock(&state.providers).clone();
    assert_eq!(providers.len(), 1, "{providers:?}");
    assert!(providers[0] != 0 && providers[0] != std::process::id());
    handle.close().await.unwrap();
    for _ in 0..200 {
        if state.disposed.load(Ordering::Acquire) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        state.disposed.load(Ordering::Acquire),
        1,
        "revoked at the end"
    );

    // Never asked for: anything but an ordinary code session one hop up.
    let refused = [
        ContractLaunchOptions {
            kind: Some(SessionKind::Chat),
            ..ContractLaunchOptions::default()
        },
        ContractLaunchOptions {
            kind: Some(SessionKind::Voice),
            ..ContractLaunchOptions::default()
        },
        ContractLaunchOptions {
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        },
        ContractLaunchOptions {
            kind: Some(SessionKind::Agent),
            agent_access: Some(AgentAccess::Auto),
            ..ContractLaunchOptions::default()
        },
        ContractLaunchOptions {
            collaboration_depth: Some(1),
            ..ContractLaunchOptions::default()
        },
    ];
    for options in refused {
        let configuration = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: cwd.clone(),
                prompt: "hi".into(),
                ..options
            })
            .unwrap();
        assert!(configuration.team_tools.is_none(), "{configuration:?}");
    }
    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            cwd.clone(),
            ResumeOptions {
                collaboration_depth: Some(1),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert!(
        resumed.team_tools.is_none(),
        "a started thread resumes without them"
    );
    let resumed = adapter
        .resume_configuration("existing".into(), cwd.clone(), ResumeOptions::default())
        .unwrap();
    assert!(resumed.team_tools.is_some(), "resume re-attaches them");

    // Offered to nobody (the owner's setting is off): no server, no notice.
    let state = Arc::new(CollaborationTestState::default());
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_team_tools(TeamToolsTestFactory::shared(&state, TEAM_SOCKET, false));
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd,
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let rendered = fs::read_to_string(&arguments).unwrap();
    assert!(!rendered.contains("iface_team"), "{rendered}");
    assert_eq!(lock(&state.requests).len(), 1);
    assert!(lock(&state.confirmed).is_empty());
    handle.close().await.unwrap();
}

#[tokio::test]
async fn fake_cli_agent_appends_the_autonomous_task_note() {
    let directory = tempdir().unwrap();
    let arguments = directory.path().join("args.txt");
    let stdin = directory.path().join("stdin.txt");
    let (_fixture, executable) = fake_cli(&capturing_script(&arguments, &stdin));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let rendered = fs::read_to_string(&arguments).unwrap();
    assert!(!rendered.lines().any(|line| line == "--tools"));
    let initialized = initialize_request(&stdin);
    assert_eq!(
        initialized["appendSystemPrompt"],
        json!(AGENT_PROMPT_APPEND)
    );
    assert!(initialized.get("systemPrompt").is_none());
    assert_eq!(initialized["excludeDynamicSections"], true);
    handle.close().await.unwrap();
}

/// Workspace-write is Claude's OS sandbox for Bash, with no network and no
/// unsandboxed retry, plus file tools that refuse outside reads. It joins the
/// other flag settings; the modes Claude cannot enforce stay refused unsent.
#[test]
fn workspace_write_launches_claude_in_its_os_sandbox() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let options = |sandbox_mode| ContractLaunchOptions {
        cwd: "/workspace".into(),
        prompt: "begin".into(),
        permission_mode: Some("acceptEdits".into()),
        fast_mode: Some(true),
        sandbox_mode,
        ..ContractLaunchOptions::default()
    };
    let configuration = adapter
        .create_configuration(options(Some(SandboxMode::WorkspaceWrite)))
        .unwrap();
    let settings = configuration.launch.settings.expect("flag settings");
    assert_eq!(
        settings["sandbox"],
        json!({
            "enabled": true,
            "failIfUnavailable": true,
            "allowUnsandboxedCommands": false,
            "autoAllowBashIfSandboxed": true,
            "network": {"strictAllowlist": true},
        })
    );
    assert_eq!(
        settings["permissions"],
        json!({"blockReadsOutsideWorkingDirectories": true})
    );
    assert_eq!(settings["fastMode"], Value::Bool(true));
    assert_eq!(
        configuration.launch.permission_mode,
        PermissionMode::AcceptEdits
    );
    let unsandboxed = adapter.create_configuration(options(None)).unwrap();
    let unsandboxed = unsandboxed.launch.settings.expect("flag settings");
    assert!(!unsandboxed.contains_key("sandbox"));
    assert!(!unsandboxed.contains_key("permissions"));
    for mode in [SandboxMode::ReadOnly, SandboxMode::DangerFullAccess] {
        let error = adapter
            .create_configuration(options(Some(mode)))
            .unwrap_err();
        assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    }
}

#[test]
fn bypass_is_launch_only_and_invalid_settings_are_definitely_unsent() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let error = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            setting_sources: Some(vec!["credentials".into()]),
            ..ContractLaunchOptions::default()
        })
        .unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
}

#[test]
fn launch_maps_ultracode_and_fast_mode_before_spawn_and_fences_app_commands() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let configuration = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "begin".into(),
            effort: Some("ultracode".into()),
            fast_mode: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_eq!(configuration.launch.effort.as_deref(), Some("xhigh"));
    assert_eq!(
        configuration.launch.settings,
        Some(Map::from_iter([
            ("ultracode".into(), Value::Bool(true)),
            ("fastMode".into(), Value::Bool(true)),
        ]))
    );
    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                effort: Some("ultracode".into()),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert_eq!(resumed.launch.effort.as_deref(), Some("xhigh"));
    assert_eq!(
        resumed
            .launch
            .settings
            .as_ref()
            .and_then(|settings| settings.get("ultracode")),
        Some(&Value::Bool(true))
    );

    let error = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "/model claude-opus".into(),
            ..ContractLaunchOptions::default()
        })
        .unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    assert!(error.message.contains("nothing was sent to the provider"));
}

#[test]
fn session_kind_presets_match_the_chat_agent_and_code_contracts() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let chat = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Chat),
            permission_mode: Some("acceptEdits".into()),
            setting_sources: Some(vec!["user".into()]),
            allowed_tools: Some(vec!["Read".into()]),
            disallowed_tools: Some(vec!["Bash".into()]),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_eq!(chat.kind, SessionKind::Chat);
    assert_eq!(chat.permission_mode, PermissionMode::Default);
    assert_eq!(chat.launch.permission_mode, PermissionMode::Default);
    assert_eq!(chat.launch.setting_sources, Some(Vec::new()));
    assert!(chat.launch.allowed_tools.is_empty());
    assert!(chat.launch.disallowed_tools.is_empty());
    assert_eq!(chat.launch.tools, Some(Vec::new()));
    assert_eq!(
        chat.launch.initialization.system_prompt.as_deref(),
        Some(&[CHAT_SYSTEM_PROMPT.to_owned()][..])
    );
    assert_eq!(chat.launch.initialization.append_system_prompt, None);
    assert!(chat.launch.initialization.exclude_dynamic_sections);

    let chat_resume = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                kind: Some(SessionKind::Chat),
                setting_sources: Some(vec!["user".into()]),
                disallowed_tools: Some(vec!["Bash".into()]),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert_eq!(chat_resume.kind, SessionKind::Chat);
    assert_eq!(chat_resume.launch.tools, Some(Vec::new()));
    assert_eq!(chat_resume.launch.setting_sources, Some(Vec::new()));
    assert!(chat_resume.launch.disallowed_tools.is_empty());
    assert_eq!(
        chat_resume.launch.initialization.system_prompt.as_deref(),
        Some(&[CHAT_SYSTEM_PROMPT.to_owned()][..])
    );

    let agent = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_eq!(agent.kind, SessionKind::Agent);
    assert_eq!(agent.launch.tools, None);
    assert_eq!(
        agent.launch.initialization.append_system_prompt.as_deref(),
        Some(AGENT_PROMPT_APPEND)
    );
    assert!(agent.launch.initialization.exclude_dynamic_sections);

    let code = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_eq!(code.kind, SessionKind::Code);
    assert_eq!(code.launch.tools, None);
    assert_eq!(code.launch.initialization.system_prompt, None);
    assert_eq!(code.launch.initialization.append_system_prompt, None);
}

struct FakeVoiceMcp {
    keys: Mutex<Vec<String>>,
}

impl VoiceMcpProvider for FakeVoiceMcp {
    fn descriptor_for(&self, session_key: &str, cwd: &Path) -> Option<McpServer> {
        assert_eq!(cwd, Path::new("/workspace"));
        lock(&self.keys).push(session_key.to_owned());
        Some(McpServer {
            command: PathBuf::from("/usr/bin/nc"),
            args: vec!["-U".into(), "/tmp/v.sock".into()],
        })
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn voice_kind_attaches_the_voice_mcp_and_spoken_prompt_but_chat_carries_neither() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let provider = Arc::new(FakeVoiceMcp {
        keys: Mutex::new(Vec::new()),
    });
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_voice_mcp(provider.clone());
    let voice = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            kind: Some(SessionKind::Voice),
            model: Some("sonnet".into()),
            effort: Some("low".into()),
            name: Some("Voice assistant".into()),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_eq!(voice.kind, SessionKind::Voice);
    assert!(
        voice
            .launch
            .initialization
            .append_system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("voice assistant"))
    );
    assert!(voice.launch.mcp_servers.contains_key(VOICE_MCP_SERVER_NAME));
    assert!(
        voice
            .launch
            .allowed_tools
            .contains(&"mcp__iface_voice__app_state".to_owned())
    );
    assert!(
        !voice
            .launch
            .allowed_tools
            .iter()
            .any(|tool| tool.ends_with("start_pane"))
    );
    let spec = voice.launch.build_spec().unwrap();
    let arguments = spec
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let config = arguments
        .iter()
        .position(|argument| argument == "--mcp-config")
        .map(|index| arguments[index + 1].as_str())
        .expect("voice launch carries --mcp-config");
    assert!(config.contains("\"iface_voice\"") && config.contains("/usr/bin/nc"));
    assert!(
        arguments
            .iter()
            .any(|argument| argument.starts_with("--allowedTools")
                || argument.contains("mcp__iface_voice__read_pane"))
    );
    assert_eq!(
        lock(&provider.keys).as_slice(),
        [format!("claude#{}", voice.session_id)]
    );

    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                kind: Some(SessionKind::Voice),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert!(
        resumed
            .launch
            .mcp_servers
            .contains_key(VOICE_MCP_SERVER_NAME)
    );
    assert_eq!(
        resumed
            .launch
            .initialization
            .append_system_prompt
            .as_deref(),
        Some(VOICE_PROMPT_APPEND)
    );
    assert!(
        resumed
            .launch
            .allowed_tools
            .contains(&"mcp__iface_voice__ui_snapshot".to_owned())
    );

    for kind in [Some(SessionKind::Chat), None] {
        let other = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: "/workspace".into(),
                prompt: "hi".into(),
                kind,
                ..ContractLaunchOptions::default()
            })
            .unwrap();
        assert!(other.launch.mcp_servers.is_empty());
        assert_eq!(other.launch.initialization.append_system_prompt, None);
        let spec = other.launch.build_spec().unwrap();
        assert!(
            !spec
                .arguments()
                .iter()
                .any(|argument| argument == "--mcp-config")
        );
    }
}

#[tokio::test]
async fn follow_up_app_command_is_refused_before_provider_input() {
    let (directory, executable) = fake_cli(&interactive_script(false));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let rejected = handle.send("/rename hidden-title".into()).await;
    let DeliveryOutcome::DefinitelyUnsent(error) = rejected else {
        panic!("reserved command reached the provider boundary");
    };
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    assert!(error.message.contains("Interface owns /rename"));
    handle.close().await.unwrap();
}

#[tokio::test]
async fn first_provider_event_best_effort_tags_only_a_new_session() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"provider-session","model":"opus","cwd":"/work/app"}'
      ;;
    *'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#;
    let (directory, executable) = fake_cli(script);
    let projects = directory.path().join("projects");
    let adapter = ClaudeAdapter::new_with_projects_root(executable, &projects).unwrap();
    let cwd = directory.path().to_string_lossy().into_owned();
    let configuration = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: cwd.clone(),
            prompt: "go".into(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let session_id = configuration.session_id.clone();
    let project = projects.join(store::project_dir_name(&cwd));
    fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{session_id}.jsonl"));
    fs::write(
        &transcript,
        format!("{{\"type\":\"user\",\"sessionId\":\"{session_id}\",\"cwd\":\"{cwd}\"}}\n"),
    )
    .unwrap();

    let mut handle = adapter.spawn_handle(configuration);
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    loop {
        let next = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("provider event deadline")
            .expect("bounded event queue")
            .expect("provider event");
        if matches!(next.event, EngineEventKind::SessionMeta { .. }) {
            break;
        }
    }
    let mut tagged = false;
    for _ in 0..100 {
        if fs::read_to_string(&transcript)
            .unwrap()
            .contains("\"type\":\"tag\"")
        {
            tagged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tagged,
        "new Claude session was not tagged after provider identity"
    );
    handle.close().await.unwrap();

    let resumed_transcript = project.join("existing.jsonl");
    fs::write(
        &resumed_transcript,
        format!("{{\"type\":\"user\",\"sessionId\":\"existing\",\"cwd\":\"{cwd}\"}}\n"),
    )
    .unwrap();
    let configuration = adapter
        .resume_configuration("existing".into(), cwd, ResumeOptions::default())
        .unwrap();
    let mut resumed = adapter.spawn_handle(configuration);
    resumed.ready().await.unwrap();
    let mut resumed_events = resumed.take_events().unwrap();
    assert!(resumed.send("go".into()).await.is_confirmed());
    loop {
        let next = timeout(Duration::from_secs(2), resumed_events.recv())
            .await
            .expect("resumed provider event deadline")
            .expect("bounded resumed event queue")
            .expect("resumed provider event");
        if matches!(next.event, EngineEventKind::SessionMeta { .. }) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !fs::read_to_string(resumed_transcript)
            .unwrap()
            .contains("\"type\":\"tag\"")
    );
    resumed.close().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_session_publishes_catalogues_agent_models_context_and_model_refresh() {
    let fixture = tempdir().unwrap();
    let context_calls = fixture.path().join("context-calls.txt");
    let script = format!(
        r#"#!/bin/sh
context_file='{}'
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"models":[{{"value":"claude-opus-test","displayName":"Opus","description":"Claude Opus 5","supportsEffort":true,"supportedEffortLevels":["xhigh"]}}],"commands":[{{"name":"/review","description":"Review work"}},{{"name":"/model","description":"app owned"}}],"agents":[{{"name":"Explore","description":"Search","model":"claude-haiku-test"}}]}}}}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{{"type":"system","subtype":"init","session_id":"provider-session","model":"opus","cwd":"/work/app"}}'
      printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"call-1","name":"Task","input":{{"subagent_type":"Explore","description":"Find it","model":"parent-override"}}}}]}}}}'
      printf '%s\n' '{{"type":"system","subtype":"compact_boundary","compact_metadata":{{"trigger":"auto","pre_tokens":100,"post_tokens":20}}}}'
      printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":2}}}}'
      ;;
    *'"subtype":"get_context_usage"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      if [ ! -f "$context_file" ]; then
        printf 'x\n' > "$context_file"
        printf '{{"type":"control_response","response":{{"subtype":"error","request_id":"%s","error":"context unavailable"}}}}\n' "$id"
      else
        printf 'x\n' >> "$context_file"
        printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"totalTokens":80,"maxTokens":200,"percentage":40,"categories":[{{"name":"Messages","tokens":80}}],"isAutoCompactEnabled":true,"autoCompactThreshold":150}}}}}}\n' "$id"
      fi
      ;;
    *'"subtype":"set_model"'*)
      printf '%s\n' '{{"type":"system","subtype":"commands_changed","commands":[{{"name":"/fresh","description":"Fresh command"}}]}}'
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"type":"control_request"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#,
        context_calls.display()
    );
    let (directory, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "go".into(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    handle.ready().await.unwrap();

    let mut saw_models = false;
    let mut saw_initial_commands = false;
    let mut saw_agent_model = false;
    let mut saw_context = false;
    for _ in 0..40 {
        let next = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("catalogue/context deadline")
            .expect("bounded event queue")
            .expect("catalogue/context event");
        match next.event {
            EngineEventKind::ModelsAvailable { models } => {
                saw_models = models.iter().any(|model| {
                    model.value == "claude-opus-test"
                        && model.effort_levels == ["xhigh", "ultracode"]
                });
            }
            EngineEventKind::CommandsAvailable { commands } => {
                saw_initial_commands = commands.len() == 1 && commands[0].name == "review";
            }
            EngineEventKind::SubagentStarted { model, .. } => {
                saw_agent_model = model.as_deref() == Some("claude-haiku-test");
            }
            EngineEventKind::ContextUsage { usage } => {
                saw_context = usage.used == Some(80)
                    && usage.size == Some(200)
                    && usage.percent == Some(40.0);
            }
            _ => {}
        }
        if saw_models && saw_initial_commands && saw_agent_model && saw_context {
            break;
        }
    }
    assert!(saw_models);
    assert!(saw_initial_commands);
    assert!(saw_agent_model);
    assert!(saw_context);

    assert!(
        handle
            .set_model(Some("claude-opus-next".into()))
            .await
            .is_confirmed()
    );
    let mut invalidated = false;
    let mut refreshed_after_invalidation = false;
    for _ in 0..20 {
        let next = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("command refresh deadline")
            .expect("bounded event queue")
            .expect("command refresh event");
        match next.event {
            EngineEventKind::CommandsInvalidated => invalidated = true,
            EngineEventKind::CommandsAvailable { commands }
                if invalidated && commands.iter().any(|command| command.name == "fresh") =>
            {
                refreshed_after_invalidation = true;
                break;
            }
            _ => {}
        }
    }
    assert!(refreshed_after_invalidation);

    let mut refreshes = 0;
    for _ in 0..100 {
        refreshes = fs::read_to_string(&context_calls)
            .map(|calls| calls.lines().count())
            .unwrap_or_default();
        if refreshes >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        refreshes >= 3,
        "expected init, compact, and idle context refreshes"
    );
    handle.close().await.unwrap();
}

#[tokio::test]
async fn probe_uses_exact_version_argv_without_a_shell() {
    let directory = tempdir().unwrap();
    let arguments = directory.path().join("probe-args.txt");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'Claude fake 1.0\\n'\n",
        arguments.display()
    );
    let (_fixture, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(&executable).unwrap();
    let info = adapter.probe().await.unwrap();
    assert_eq!(info.version, "Claude fake 1.0");
    assert_eq!(fs::read_to_string(arguments).unwrap(), "--version\n");
    assert_eq!(info.binary_path, executable.to_string_lossy());
    // The runtime keeps this probe only while this file is unchanged.
    assert_eq!(adapter.executable(), Some(executable.clone()));
}

/// The `--version` bound of tests that compare probe outcomes. On macOS std
/// creates a child's pipe and marks it close-on-exec in two steps, so a fake
/// CLI another test spawns in between inherits a probe's output and holds it
/// open until it exits; the command-discovery fake lingers 4 s after its
/// stdin closes. Under gate load that can push a probe past the production
/// 5 s bound (`Timeout` instead of the CLI's own failure), and the outcome
/// these tests compare must not depend on it.
const TEST_PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// A fake `claude` that answers `--version` (counting each ask in `STATE/asked`)
/// and completes a control session's handshake. `STATE/no-session` makes only
/// the control session fail; `STATE/broken` makes every invocation fail, while
/// the executable file itself stays the same.
fn versioned_cli(state: &Path, version: &str) -> (tempfile::TempDir, PathBuf) {
    const SCRIPT: &str = r#"#!/bin/sh
[ -e 'STATE/broken' ] && exit 1
if [ "$1" = --version ]; then
  printf x >> 'STATE/asked'
  printf 'VERSION\n'
  exit 0
fi
[ -e 'STATE/no-session' ] && exit 1
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*|*'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#;
    fake_cli(
        &SCRIPT
            .replace("STATE", &state.display().to_string())
            .replace("VERSION", version),
    )
}

#[tokio::test]
async fn probe_asks_the_version_once_per_executable_file() {
    let state = tempdir().unwrap();
    let asked = || fs::read_to_string(state.path().join("asked")).map_or(0, |log| log.len());
    let (fixture, executable) = versioned_cli(state.path(), "Claude fake 1.0");
    let adapter = ClaudeAdapter::new(&executable)
        .unwrap()
        .with_probe_timeout(TEST_PROBE_TIMEOUT);

    assert_eq!(adapter.probe().await.unwrap().version, "Claude fake 1.0");
    assert_eq!(adapter.probe().await.unwrap().version, "Claude fake 1.0");
    assert_eq!(asked(), 1, "an unchanged executable is not asked again");

    // An update installs a new file at the same path.
    let (_update, replacement) = versioned_cli(state.path(), "Claude fake 2.0");
    fs::copy(&replacement, fixture.path().join("next")).unwrap();
    fs::rename(fixture.path().join("next"), &executable).unwrap();
    assert_eq!(adapter.probe().await.unwrap().version, "Claude fake 2.0");
    assert_eq!(asked(), 2);
}

#[tokio::test]
async fn a_kept_version_never_hides_an_unchanged_executable_that_stopped_running() {
    let state = tempdir().unwrap();
    let asked = || fs::read_to_string(state.path().join("asked")).map_or(0, |log| log.len());
    let (_fixture, executable) = versioned_cli(state.path(), "Claude fake 1.0");
    let adapter = ClaudeAdapter::new(&executable)
        .unwrap()
        .with_probe_timeout(TEST_PROBE_TIMEOUT);
    assert_eq!(adapter.probe().await.unwrap().version, "Claude fake 1.0");
    assert_eq!(asked(), 1);

    // The control session fails for another reason, but the version still
    // answers: the probe is ready, as it was before versions were kept.
    fs::write(state.path().join("no-session"), "").unwrap();
    assert_eq!(adapter.probe().await.unwrap().version, "Claude fake 1.0");
    assert_eq!(
        asked(),
        2,
        "a probe that did not see the CLI start asks again"
    );

    // The same file no longer runs at all: unavailable, with the error an
    // adapter that never kept a version reports.
    fs::write(state.path().join("broken"), "").unwrap();
    let error = adapter.probe().await.unwrap_err();
    let uncached = ClaudeAdapter::new(&executable)
        .unwrap()
        .with_probe_timeout(TEST_PROBE_TIMEOUT)
        .probe()
        .await
        .unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Unavailable);
    assert_eq!(error.message, "Claude version probe exited unsuccessfully");
    assert_eq!(error, uncached);
}

#[tokio::test]
async fn store_task_join_failures_are_fail_closed() {
    let read = run_store(false, || -> () { panic!("read worker panic") })
        .await
        .unwrap_err();
    assert_eq!(read.message, "Claude store task failed");
    assert_eq!(read.kind, EngineErrorKind::Internal);
    assert_eq!(read.certainty, EffectCertainty::DefinitelyUnsent);
    assert!(read.replay_is_safe());
    let mutate = run_store(true, || -> () { panic!("mutation worker panic") })
        .await
        .unwrap_err();
    assert_eq!(mutate.message, "Claude store task failed");
    assert_eq!(mutate.certainty, EffectCertainty::OutcomeUnknown);
    assert!(!mutate.replay_is_safe());
}

#[test]
fn volatile_permission_history_omits_default() {
    let history = Mutex::new(HashMap::new());
    remember_volatile_permission(&history, "s", PermissionMode::Default);
    assert!(lock(&history).is_empty());
    remember_volatile_permission(&history, "s", PermissionMode::Auto);
    assert_eq!(lock(&history).get("s").copied(), Some(PermissionMode::Auto));
    remember_volatile_permission(&history, "s", PermissionMode::Default);
    assert!(lock(&history).is_empty());
}

#[test]
fn versioned_display_keeps_an_already_versioned_family_name() {
    assert_eq!(
        versioned_claude_display("Opus 4", Some("Claude Opus 4.6 for exact tests")),
        "Opus 4"
    );
    assert_eq!(
        versioned_claude_display("Sonnet", Some("Claude Sonnet 4.6")),
        "Sonnet 4.6"
    );
}

#[tokio::test]
async fn adapter_store_ops_run_on_the_blocking_pool() {
    let (directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let projects = directory.path().join("projects");
    let cwd = "/work/app";
    let project = projects.join(store::project_dir_name(cwd));
    fs::create_dir_all(&project).unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";
    fs::write(
        project.join(format!("{session_id}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"uuid\":\"22222222-2222-4222-8222-222222222222\",\"sessionId\":\"{session_id}\",\"cwd\":\"{cwd}\",\"message\":{{\"content\":\"hello\"}}}}\n\
                 {{\"type\":\"assistant\",\"uuid\":\"33333333-3333-4333-8333-333333333333\",\"sessionId\":\"{session_id}\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"world\"}}]}}}}\n"
        ),
    )
    .unwrap();
    let adapter = ClaudeAdapter::new_with_projects_root(executable, &projects).unwrap();
    let listed = adapter.list_sessions().await.unwrap();
    assert_eq!(listed.sessions.len(), 1);
    assert!(listed.complete);
    let page = adapter
        .read_history(
            session_id.into(),
            HistoryRequest {
                cwd: cwd.into(),
                limit: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.messages, Some(1));
    assert_eq!(page.older_messages, 1);
    adapter
        .rename_session(session_id.into(), "Renamed".into(), Some(cwd.into()))
        .await
        .unwrap();
    let listed = adapter.list_sessions().await.unwrap();
    assert_eq!(listed.sessions[0].title, "Renamed");
    let forked = adapter
        .fork_session(session_id.into(), Some(cwd.into()))
        .await
        .unwrap();
    adapter
        .adopt_session(forked.clone(), true, Some(cwd.into()))
        .await
        .unwrap();
    adapter
        .delete_session(forked, Some(cwd.into()))
        .await
        .unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn fork_at_a_turn_copies_the_transcript_through_that_turns_answer() {
    let (directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let projects = directory.path().join("projects");
    let cwd = "/work/app";
    let project = projects.join(store::project_dir_name(cwd));
    fs::create_dir_all(&project).unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";
    let line = |uuid: &str, parent: &str, kind: &str, content: &str| {
        format!(
            "{{\"type\":\"{kind}\",\"uuid\":\"{uuid}\",\"parentUuid\":{parent},\"sessionId\":\"{session_id}\",\"cwd\":\"{cwd}\",\"message\":{{\"role\":\"{kind}\",\"content\":[{{\"type\":\"text\",\"text\":\"{content}\"}}]}}}}\n"
        )
    };
    let transcript = [
        line(
            "22222222-2222-4222-8222-000000000001",
            "null",
            "user",
            "one",
        ),
        line(
            "22222222-2222-4222-8222-000000000002",
            "\"22222222-2222-4222-8222-000000000001\"",
            "assistant",
            "answer one",
        ),
        line(
            "22222222-2222-4222-8222-000000000003",
            "\"22222222-2222-4222-8222-000000000002\"",
            "user",
            "two",
        ),
        line(
            "22222222-2222-4222-8222-000000000004",
            "\"22222222-2222-4222-8222-000000000003\"",
            "assistant",
            "answer two",
        ),
        line(
            "22222222-2222-4222-8222-000000000005",
            "\"22222222-2222-4222-8222-000000000004\"",
            "user",
            "three",
        ),
    ]
    .concat();
    fs::write(project.join(format!("{session_id}.jsonl")), &transcript).unwrap();
    let adapter = ClaudeAdapter::new_with_projects_root(executable, &projects).unwrap();

    let at = |prompt: &str| interface_engine::ForkBoundary {
        prompt_digest: interface_engine::prompt_digest(prompt),
        later_same_digest: 0,
    };
    let forked = adapter
        .fork_session_at(session_id.into(), Some(cwd.into()), at("one"))
        .await
        .unwrap();
    assert_ne!(forked, session_id);
    let page = adapter
        .read_history(
            forked.clone(),
            HistoryRequest {
                cwd: cwd.into(),
                limit: 0,
            },
        )
        .await
        .unwrap();
    let texts = page
        .events
        .iter()
        .filter_map(|event| match &event.event {
            EngineEventKind::UserMessage { text, .. }
            | EngineEventKind::MessageDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["one", "answer one"]);
    let listed = adapter.list_sessions().await.unwrap();
    let copy = listed
        .sessions
        .iter()
        .find(|session| session.session_id == forked)
        .expect("the fork is listed");
    assert_eq!(copy.cwd, cwd);

    for (prompt, reason) in [
        ("three", "no answer yet"),
        ("four", "can't find that message"),
    ] {
        let error = adapter
            .fork_session_at(session_id.into(), Some(cwd.into()), at(prompt))
            .await
            .unwrap_err();
        assert_eq!(
            error.certainty,
            EffectCertainty::DefinitelyUnsent,
            "{error:?}"
        );
        assert!(!error.retryable);
        assert!(error.message.contains(reason), "{}", error.message);
    }
    assert_eq!(fs::read_dir(&project).unwrap().count(), 2);
    assert_eq!(
        fs::read_to_string(project.join(format!("{session_id}.jsonl"))).unwrap(),
        transcript
    );
}

#[test]
fn provider_store_ops_models_commands_and_usage_match_the_oracle() {
    let (directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter =
        ClaudeAdapter::new_with_projects_root(executable, directory.path().join("projects"))
            .unwrap();
    assert_eq!(
        adapter.supported_ops(),
        vec![
            EngineOp::RenameSession,
            EngineOp::DeleteSession,
            EngineOp::ForkSession,
            EngineOp::AdoptSession,
            EngineOp::ReadUsage,
        ]
    );

    let models = claude_model_catalog(&json!({"models":[{
        "value":"claude-opus-test", "displayName":"Opus",
        "description":"Claude Opus 4.6 for exact tests", "supportsEffort":true,
        "supportedEffortLevels":["low","xhigh"], "supportsFastMode":true
    }]}));
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].display_name, "Opus 4.6");
    assert_eq!(models[0].effort_levels, vec!["low", "xhigh", "ultracode"]);
    assert!(models[0].supports_fast_mode == Some(true));

    let commands = claude_commands(&json!({"commands":[{
        "name":"review", "description":"Review the work",
        "argumentHint":"[scope]", "aliases":["inspect"]
    }]}))
    .unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "review");
    assert_eq!(
        commands[0].aliases.as_deref(),
        Some(&["inspect".into()][..])
    );

    let claude = EngineId::new("claude").unwrap();
    let usage = map_claude_usage(
        &claude,
        &json!({
            "subscription_type":"max", "rate_limits_available":true,
            "rate_limits":{
                "five_hour":{"utilization":10,
                    "resets_at":"2026-08-13T04:50:00.083134+00:00"},
                "seven_day":{"utilization":null, "resets_at":"bad"},
                "seven_day_opus":{"utilization":99}
            }
        }),
    );
    assert!(usage.available);
    assert_eq!(usage.engine, claude);
    assert_eq!(usage.plan.as_deref(), Some("max"));
    assert_eq!(usage.windows.len(), 2);
    assert_eq!(usage.windows[0].label, "5-hour");
    assert_eq!(usage.windows[0].utilization, Some(10.0));
    assert_eq!(
        usage.windows[0].resets_at.as_deref(),
        Some("2026-08-13T04:50:00.083Z")
    );
    assert_eq!(usage.windows[1].utilization, None);
    assert_eq!(usage.windows[1].resets_at, None);

    let api_key = map_claude_usage(
        &claude,
        &json!({
            "subscription_type":"api", "rate_limits_available":false
        }),
    );
    assert!(!api_key.available);
    assert_eq!(api_key.engine, claude);
    assert_eq!(api_key.plan.as_deref(), Some("api"));
    assert!(api_key.windows.is_empty());
}

#[test]
fn normalize_utc_iso_rejects_non_ascii_without_panicking() {
    assert_eq!(
        normalize_utc_iso("2026-08-01T00:00:00Z").as_deref(),
        Some("2026-08-01T00:00:00.000Z")
    );
    assert_eq!(normalize_utc_iso("2026-08-01T00:00:0\u{e9}+00:00"), None);
}

#[test]
fn delivery_conversion_preserves_unknown_outcomes() {
    let outcome = delivery_result(Err(DeliveryError {
        certainty: DeliveryCertainty::OutcomeUnknown,
        message: "partial write".into(),
    }));
    assert!(matches!(outcome, DeliveryOutcome::OutcomeUnknown(_)));
}

#[tokio::test]
async fn legacy_mapped_event_overflow_keeps_the_newest_live_event() {
    let (sink, mut stream) = bounded_event_stream(1);
    let mapped = vec![
        event(EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        }),
        event(EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        }),
    ];
    assert!(emit_all(&sink, mapped));
    assert!(matches!(
        stream.recv().await.unwrap().unwrap().event,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle
        }
    ));
    sink.close();
    assert_eq!(stream.recv().await, Ok(None));
}

#[tokio::test]
async fn budgeted_control_overflow_stops_emission_with_an_explicit_error() {
    let (sink, mut stream) = bounded_event_stream_with_budget(1, 4096);
    let mapped = vec![
        event(EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        }),
        event(EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        }),
    ];
    assert!(!emit_all(&sink, mapped));
    assert!(matches!(
        stream.recv().await,
        Err(interface_engine::EventStreamError::Overflow { capacity: 1 })
    ));
}

#[tokio::test]
async fn stale_command_refresh_cannot_overwrite_a_newer_provider_revision() {
    let command = |name: &str| ProviderCommand {
        name: name.into(),
        description: "test".into(),
        argument_hint: None,
        aliases: None,
    };
    let (sink, mut stream) = bounded_event_stream(2);
    assert!(emit_advisory(
        &sink,
        AdvisoryResult::Commands {
            revision: 1,
            commands: Some(vec![command("old")]),
            report_failure: false,
        },
        2,
        0,
    ));
    assert!(emit_advisory(
        &sink,
        AdvisoryResult::Commands {
            revision: 2,
            commands: Some(vec![command("fresh")]),
            report_failure: false,
        },
        2,
        0,
    ));
    let next = stream.recv().await.unwrap().unwrap();
    assert!(matches!(
        next.event,
        EngineEventKind::CommandsAvailable { commands }
            if commands.len() == 1 && commands[0].name == "fresh"
    ));
}

#[tokio::test]
async fn stale_context_refresh_cannot_overwrite_a_newer_context_revision() {
    let usage = |used: u64| ContextUsage {
        used: Some(used),
        size: Some(1_000),
        percent: None,
        categories: None,
        auto_compact: None,
        auto_compact_threshold: None,
        compacting: None,
    };
    let (sink, mut stream) = bounded_event_stream(2);
    assert!(emit_advisory(
        &sink,
        AdvisoryResult::Context {
            revision: 1,
            usage: Some(usage(100)),
        },
        0,
        2,
    ));
    assert!(emit_advisory(
        &sink,
        AdvisoryResult::Context {
            revision: 2,
            usage: Some(usage(200)),
        },
        0,
        2,
    ));
    sink.close();
    let next = stream.recv().await.unwrap().unwrap();
    assert!(matches!(
        next.event,
        EngineEventKind::ContextUsage { usage } if usage.used == Some(200)
    ));
    assert_eq!(stream.recv().await, Ok(None));
}

fn launch_arguments(launch: &ClaudeLaunchOptions) -> Vec<String> {
    launch
        .build_spec()
        .expect("valid launch")
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

fn flag_value<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

const EXPECTED_GATE_OUTPUT: &str =
    r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}"#;

fn assert_gate_settings(configuration: &RunConfiguration) {
    let settings = Value::Object(
        configuration
            .launch
            .settings
            .clone()
            .expect("a gated launch carries flag settings"),
    );
    // Flag settings outrank project/local files the agent could write.
    assert_eq!(settings["disableAllHooks"], json!(false));
    let pre_tool_use = &settings["hooks"]["PreToolUse"];
    assert_eq!(pre_tool_use.as_array().map(Vec::len), Some(1));
    assert_eq!(pre_tool_use[0]["matcher"], json!("*"));
    assert_eq!(pre_tool_use[0]["hooks"][0]["type"], json!("command"));
    let command = pre_tool_use[0]["hooks"][0]["command"]
        .as_str()
        .expect("hook command text");
    // The hook command is run by Claude's hook shell; prove it prints exactly the ask decision.
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .output()
        .expect("run the gate hook command");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        EXPECTED_GATE_OUTPUT
    );
    assert_eq!(configuration.permission_mode, PermissionMode::Default);
    assert_eq!(
        configuration.launch.permission_mode,
        PermissionMode::Default
    );
    assert!(configuration.approval_gate);
    let arguments = launch_arguments(&configuration.launch);
    let rendered = flag_value(&arguments, "--settings").expect("--settings flag");
    assert_eq!(
        serde_json::from_str::<Value>(rendered).expect("settings JSON"),
        settings
    );
    assert_eq!(flag_value(&arguments, "--permission-mode"), Some("default"));
    assert!(arguments.contains(&"--setting-sources=user".to_owned()));
    assert!(arguments.contains(&"--strict-mcp-config".to_owned()));
    assert!(!arguments.contains(&"--allow-dangerously-skip-permissions".to_owned()));
}

#[test]
fn approval_gate_merges_pre_tool_use_ask_hook() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let created = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            effort: Some(ULTRACODE_LEVEL.into()),
            fast_mode: Some(true),
            permission_mode: Some("bypassPermissions".into()),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    assert_gate_settings(&created);
    let settings = created.launch.settings.as_ref().unwrap();
    assert_eq!(settings.get("ultracode"), Some(&Value::Bool(true)));
    assert_eq!(settings.get("fastMode"), Some(&Value::Bool(true)));

    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                kind: Some(SessionKind::Agent),
                effort: Some(ULTRACODE_LEVEL.into()),
                approval_gate: Some(true),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert_gate_settings(&resumed);
    assert_eq!(
        resumed.launch.settings.as_ref().unwrap().get("ultracode"),
        Some(&Value::Bool(true))
    );

    for gate in [None, Some(false)] {
        let ungated = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: "/workspace".into(),
                prompt: "hi".into(),
                fast_mode: Some(true),
                permission_mode: Some("bypassPermissions".into()),
                approval_gate: gate,
                ..ContractLaunchOptions::default()
            })
            .unwrap();
        assert!(!ungated.approval_gate);
        assert_eq!(ungated.permission_mode, PermissionMode::BypassPermissions);
        assert!(!ungated.launch.settings.unwrap().contains_key("hooks"));
    }
}

#[derive(Debug)]
struct FixedAgentTools;
impl interface_contracts::engine::AgentToolsProvider for FixedAgentTools {
    fn descriptor_for(&self, _cwd: &Path) -> Option<CollaborationMcpStdioDescriptor> {
        Some(CollaborationMcpStdioDescriptor {
            name: "interface_agent".into(),
            command: "/bin/echo".into(),
            args: vec!["tools".into()],
            env: Vec::new(),
        })
    }
}

/// Auto and Bypass hand permission decisions to Claude: its own mode, no
/// Interface gate hook, the agent's folders and task tools kept, and the
/// session stays out of cross-model collaboration like a gated agent.
#[test]
fn agent_access_launches_claude_in_its_own_mode_without_the_gate_hook() {
    use interface_contracts::engine::AgentAccess;
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable)
        .unwrap()
        .with_agent_tools(Arc::new(FixedAgentTools));
    for (access, mode) in [
        (AgentAccess::Auto, PermissionMode::Auto),
        (AgentAccess::Bypass, PermissionMode::BypassPermissions),
    ] {
        let created = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: "/workspace".into(),
                prompt: "hi".into(),
                kind: Some(SessionKind::Agent),
                permission_mode: Some("default".into()),
                additional_directories: Some(vec!["/projects/app".into()]),
                agent_access: Some(access),
                ..ContractLaunchOptions::default()
            })
            .unwrap();
        let resumed = adapter
            .resume_configuration(
                "existing".into(),
                "/workspace".into(),
                ResumeOptions {
                    kind: Some(SessionKind::Agent),
                    additional_directories: Some(vec!["/projects/app".into()]),
                    agent_access: Some(access),
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        for configuration in [&created, &resumed] {
            assert!(!configuration.approval_gate);
            assert_eq!(configuration.agent_access, Some(access));
            assert_eq!(configuration.permission_mode, mode);
            assert_eq!(configuration.launch.permission_mode, mode);
            assert!(
                configuration
                    .launch
                    .settings
                    .as_ref()
                    .is_none_or(|settings| !settings.contains_key("hooks"))
            );
            assert!(
                configuration
                    .launch
                    .mcp_servers
                    .contains_key("interface_agent")
            );
            assert!(configuration.collaboration_mcp_factory.is_none());
            let arguments = launch_arguments(&configuration.launch);
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| pair[0] == "--add-dir" && pair[1] == "/projects/app")
            );
        }
        assert!(resumed.prelude.iter().any(|event| matches!(
            &event.event,
            EngineEventKind::AccessChanged { mode: reported } if reported == mode.as_str()
        )));
    }
    for (approval_gate, kind) in [(Some(true), SessionKind::Agent), (None, SessionKind::Code)] {
        let error = adapter
            .create_configuration(ContractLaunchOptions {
                cwd: "/workspace".into(),
                prompt: "hi".into(),
                kind: Some(kind),
                approval_gate,
                agent_access: Some(AgentAccess::Bypass),
                ..ContractLaunchOptions::default()
            })
            .unwrap_err();
        assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    }
}

/// Claude silently starts in its default mode when auto mode is unavailable
/// for the model or account; any Auto session or Auto agent then says so once.
#[test]
fn an_auto_agent_reports_when_claude_started_outside_auto_mode() {
    use interface_contracts::engine::AgentAccess;
    let init = |mode: &str| json!({"type":"system","subtype":"init","permissionMode":mode});
    let notice = auto_mode_notice(
        PermissionMode::Auto,
        Some(AgentAccess::Auto),
        &init("default"),
    )
    .unwrap();
    assert!(matches!(
        notice.event,
        EngineEventKind::Notice { ref message, tone: NoticeTone::Info }
            if message == AUTO_MODE_UNAVAILABLE
    ));
    assert!(auto_mode_notice(PermissionMode::Auto, None, &init("default")).is_some());
    assert!(
        auto_mode_notice(PermissionMode::Auto, Some(AgentAccess::Auto), &init("auto")).is_none()
    );
    assert!(
        auto_mode_notice(
            PermissionMode::BypassPermissions,
            Some(AgentAccess::Bypass),
            &init("default")
        )
        .is_none()
    );
    assert!(auto_mode_notice(PermissionMode::Default, None, &init("default")).is_none());
    assert!(
        auto_mode_notice(
            PermissionMode::Auto,
            Some(AgentAccess::Auto),
            &json!({"type":"assistant","permissionMode":"default"})
        )
        .is_none()
    );
}

#[test]
fn additional_directories_emit_add_dir() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let directories = vec!["/agents/scout".to_owned(), "/projects/app".to_owned()];
    let expected = directories.iter().map(PathBuf::from).collect::<Vec<_>>();
    let created = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            additional_directories: Some(directories.clone()),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let resumed = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                additional_directories: Some(directories),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    for configuration in [&created, &resumed] {
        assert_eq!(configuration.launch.additional_directories, expected);
        let arguments = launch_arguments(&configuration.launch);
        let granted = arguments
            .windows(2)
            .filter(|pair| pair[0] == "--add-dir")
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(granted, ["/agents/scout", "/projects/app"]);
    }

    let error = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            additional_directories: Some(vec!["/projects/app".into(), "relative/dir".into()]),
            ..ContractLaunchOptions::default()
        })
        .unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    let error = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                additional_directories: Some(vec!["relative/dir".into()]),
                ..ResumeOptions::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
}

#[test]
fn gate_rejected_for_chat_kind() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let error = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Chat),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
    let error = adapter
        .resume_configuration(
            "existing".into(),
            "/workspace".into(),
            ResumeOptions {
                kind: Some(SessionKind::Chat),
                approval_gate: Some(true),
                ..ResumeOptions::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
}

/// Whatever still reaches `can_use_tool` in an Auto or Bypass agent is Claude
/// escalating: its classifier declined, auto mode is unavailable for the model
/// (Claude then asks about every edit), or bypass still prompts. The adapter
/// never settles it by the session's mode; it goes to Interface for the owner.
#[tokio::test]
async fn agent_access_sessions_leave_every_escalation_to_interface() {
    use interface_contracts::engine::AgentAccess;
    for access in [AgentAccess::Auto, AgentAccess::Bypass] {
        let directory = tempdir().unwrap();
        let responses = directory.path().join("responses.txt");
        let (_fixture, executable) = fake_cli(&write_approval_script(&responses));
        let adapter = ClaudeAdapter::new(executable).unwrap();
        let mut handle = adapter
            .create_session(ContractLaunchOptions {
                cwd: directory.path().to_string_lossy().into_owned(),
                kind: Some(SessionKind::Agent),
                agent_access: Some(access),
                ..ContractLaunchOptions::default()
            })
            .unwrap();
        handle.ready().await.unwrap();
        let mut events = handle.take_events().unwrap();
        assert!(handle.send("write it".into()).await.is_confirmed());
        loop {
            let next = timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("event deadline")
                .expect("bounded queue")
                .expect("event");
            if let EngineEventKind::ApprovalRequested { tool, .. } = next.event {
                assert_eq!(tool, "Write");
                break;
            }
        }
        assert!(
            fs::read_to_string(&responses)
                .unwrap_or_default()
                .is_empty(),
            "{access:?}: the adapter settled Claude's escalation by its mode"
        );
        handle.close().await.unwrap();
    }
}

/// An agent's mode comes from its settings: an Auto or Bypass agent session
/// refuses every other mode before anything reaches Claude and keeps its own.
#[tokio::test]
async fn an_agent_access_session_keeps_the_mode_its_settings_chose() {
    use interface_contracts::engine::AgentAccess;
    let directory = tempdir().unwrap();
    let stdin = directory.path().join("stdin.txt");
    let (_fixture, executable) = fake_cli(&capturing_script(
        &directory.path().join("args.txt"),
        &stdin,
    ));
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            kind: Some(SessionKind::Agent),
            agent_access: Some(AgentAccess::Auto),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    for mode in [
        "default",
        "acceptEdits",
        "plan",
        "dontAsk",
        "bypassPermissions",
    ] {
        let refused = handle.set_permission_mode(mode.into()).await;
        assert!(
            matches!(
                &refused,
                DeliveryOutcome::DefinitelyUnsent(error)
                    if error.kind == EngineErrorKind::Unsupported
                        && error.message.contains("comes from its settings")
            ),
            "an Auto agent must refuse {mode}: {refused:?}"
        );
    }
    assert!(
        handle
            .set_permission_mode("auto".into())
            .await
            .is_confirmed()
    );
    let mode_requests = fs::read_to_string(&stdin)
        .expect("captured stdin")
        .lines()
        .filter(|line| line.contains("\"subtype\":\"set_permission_mode\""))
        .count();
    assert_eq!(mode_requests, 1);
    handle.close().await.unwrap();
}

/// Host side of the gate: the fake CLI always raises `can_use_tool`, so this
/// proves a gated session keeps its CLI in default mode and never settles a
/// tool request itself. The CLI-side hook is pinned by the settings tests.
#[tokio::test]
async fn gated_session_refuses_mode_changes_and_leaves_every_tool_request_to_interface() {
    let directory = tempdir().unwrap();
    let responses = directory.path().join("responses.txt");
    let stdin = directory.path().join("stdin.txt");
    let script = write_approval_script(&responses).replacen(
        "while IFS= read -r line; do\n",
        &format!(
            "while IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n",
            stdin.display()
        ),
        1,
    );
    let (_fixture, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    for mode in [
        "auto",
        "acceptEdits",
        "plan",
        "dontAsk",
        "bypassPermissions",
    ] {
        let refused = handle.set_permission_mode(mode.into()).await;
        assert!(
            matches!(
                &refused,
                DeliveryOutcome::DefinitelyUnsent(error)
                    if error.kind == EngineErrorKind::Unsupported
                        && error.message.contains("approval gate")
            ),
            "a gated session must refuse {mode}: {refused:?}"
        );
    }
    assert!(
        handle
            .set_permission_mode("default".into())
            .await
            .is_confirmed()
    );
    let mode_requests = fs::read_to_string(&stdin)
        .expect("captured stdin")
        .lines()
        .filter(|line| line.contains("\"subtype\":\"set_permission_mode\""))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(mode_requests.len(), 1, "{mode_requests:?}");
    assert!(mode_requests[0].contains("\"mode\":\"default\""));

    assert!(handle.send("write it".into()).await.is_confirmed());
    loop {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        if let EngineEventKind::ApprovalRequested { tool, .. } = next.event {
            assert_eq!(tool, "Write");
            break;
        }
    }
    assert!(
        fs::read_to_string(&responses)
            .unwrap_or_default()
            .is_empty(),
        "a gated session settled a tool request without Interface's policy"
    );
    handle.close().await.unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn claude_approval_facts_cover_every_tool_row() {
    use interface_contracts::agents::ApprovalFacts;
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let unknown = |tool: &str| ApprovalFacts::Unknown { tool: tool.into() };
    let rows = [
        (
            "ToolSearch",
            json!({"query":"select:mcp__iface_agent__task_status","max_results":5}),
            ApprovalFacts::Internal {
                tool: "ToolSearch".into(),
            },
        ),
        (
            "ToolSearch",
            json!({"query":"tools","execute":true}),
            unknown("ToolSearch"),
        ),
        (
            "Bash",
            json!({"command": "git status", "description": "status"}),
            ApprovalFacts::Shell {
                command: "git status".into(),
                cwd: None,
            },
        ),
        ("Bash", json!({}), unknown("Bash")),
        (
            "Bash",
            json!({"command": ["git", "status"]}),
            unknown("Bash"),
        ),
        (
            "Read",
            json!({"file_path": "/repo/src/main.rs", "offset": 4}),
            ApprovalFacts::Read {
                paths: vec!["/repo/src/main.rs".into()],
            },
        ),
        ("Read", json!({"path": "/repo"}), unknown("Read")),
        (
            "Grep",
            json!({"pattern": "todo", "path": "/repo/src"}),
            ApprovalFacts::Search {
                paths: vec!["/repo/src".into()],
            },
        ),
        (
            "Grep",
            json!({"pattern": "todo"}),
            ApprovalFacts::Search { paths: Vec::new() },
        ),
        (
            "Glob",
            json!({"pattern": "**/*.rs", "path": "/repo"}),
            ApprovalFacts::Search {
                paths: vec!["/repo".into()],
            },
        ),
        (
            "Glob",
            json!({"pattern": "**/*.rs"}),
            ApprovalFacts::Search { paths: Vec::new() },
        ),
        ("Glob", json!({"path": 7}), unknown("Glob")),
        (
            "Edit",
            json!({"file_path": "/repo/a.rs", "old_string": "a", "new_string": "b"}),
            ApprovalFacts::Write {
                paths: vec!["/repo/a.rs".into()],
            },
        ),
        (
            "MultiEdit",
            json!({"file_path": "/repo/b.rs", "edits": []}),
            ApprovalFacts::Write {
                paths: vec!["/repo/b.rs".into()],
            },
        ),
        (
            "Write",
            json!({"file_path": "probe.txt", "content": "x"}),
            ApprovalFacts::Write {
                paths: vec!["probe.txt".into()],
            },
        ),
        ("Write", json!({"content": "x"}), unknown("Write")),
        (
            "NotebookEdit",
            json!({"notebook_path": "/repo/n.ipynb", "new_source": ""}),
            ApprovalFacts::Write {
                paths: vec!["/repo/n.ipynb".into()],
            },
        ),
        (
            "NotebookEdit",
            json!({"file_path": "/repo/n.ipynb"}),
            unknown("NotebookEdit"),
        ),
        (
            "WebFetch",
            json!({"url": "https://example.com/doc", "prompt": "summarize"}),
            ApprovalFacts::WebFetch {
                url: "https://example.com/doc".into(),
                method: "GET".into(),
            },
        ),
        (
            "WebFetch",
            json!({"prompt": "summarize"}),
            unknown("WebFetch"),
        ),
        (
            "WebSearch",
            json!({"query": "rust"}),
            ApprovalFacts::WebSearch {},
        ),
        ("WebSearch", Value::Null, ApprovalFacts::WebSearch {}),
        (
            "mcp__iface_voice__read_pane",
            json!({"pane": 1}),
            ApprovalFacts::Mcp {
                server: "iface_voice".into(),
                tool: "read_pane".into(),
            },
        ),
        (
            "mcp__slack__send_message",
            Value::Null,
            ApprovalFacts::Mcp {
                server: "slack".into(),
                tool: "send_message".into(),
            },
        ),
        ("mcp__slack", json!({}), unknown("mcp__slack")),
        ("mcp____send", json!({}), unknown("mcp____send")),
        ("mcp__slack__", json!({}), unknown("mcp__slack__")),
        (
            "AskUserQuestion",
            json!({"questions": []}),
            unknown("AskUserQuestion"),
        ),
        ("ExitPlanMode", json!({}), unknown("ExitPlanMode")),
        ("", json!({}), unknown("")),
    ];
    for (tool, input, expected) in rows {
        assert_eq!(
            adapter.approval_facts(tool, &input),
            expected,
            "tool {tool} input {input}"
        );
    }
    for tool in [
        "TodoWrite",
        "Task",
        "Agent",
        "BashOutput",
        "KillShell",
        "KillBash",
        "ListMcpResourcesTool",
    ] {
        assert_eq!(
            adapter.approval_facts(tool, &json!({"anything": true})),
            ApprovalFacts::Internal { tool: tool.into() }
        );
    }
}

#[test]
fn voice_prompt_does_not_key_speech_on_the_constant_audio_flag() {
    // control_status always reports audioConnected false, even while replies
    // are spoken, so the prompt must not read speech state from it.
    assert!(!VOICE_PROMPT_APPEND.contains("audioConnected"));
}

/// A CLI that answers `initialize`, keeps a sent turn running, and, when
/// `on_user` is set, ends it with `tail` at once; otherwise it acknowledges
/// an interrupt and then writes `tail`, the stopped turn's own end. Every
/// tail closes with a `commands_changed` frame, so a test reads exactly to
/// the end of it.
fn stopped_turn_script(tail: &[Value], on_user: bool) -> String {
    let end = json!({"type": "system", "subtype": "commands_changed",
                     "commands": [{"name": "/done", "description": "End of tail"}]});
    let mut lines = String::new();
    for frame in tail.iter().chain([&end]) {
        lines.push_str("      printf '%s\\n' '");
        lines.push_str(&frame.to_string());
        lines.push_str("'\n");
    }
    let (user_tail, interrupt_tail) = if on_user {
        (lines.as_str(), "")
    } else {
        ("", lines.as_str())
    };
    format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"subtype":"interrupt"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"cancelled":[],"still_queued":[]}}}}}}\n' "$id"
{interrupt_tail}      ;;
    *'"type":"user"'*)
{user_tail}      ;;
    *'"type":"control_request"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
  esac
done
"#
    )
}

/// Claude Code 2.1.325's end of a stopped turn: the stream's stop, then an
/// `error_during_execution` result carrying an `[ede_diagnostic]` error.
fn ede_stop_tail() -> Vec<Value> {
    vec![
        json!({"type": "stream_event", "event": {"type": "message_stop"}, "parent_tool_use_id": null}),
        json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
               "errors": ["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=null"],
               "usage": {"input_tokens": 3, "output_tokens": 1},
               "fast_mode_state": "off"}),
    ]
}

/// Sends a turn, optionally stops it, and answers the events that follow up
/// to the tail's closing `commands_changed`.
async fn events_after_turn(script: String, stop: bool) -> Vec<EngineEventKind> {
    let (directory, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    assert!(handle.send("count".into()).await.is_confirmed());
    if stop {
        assert!(handle.interrupt().await.is_confirmed());
    }
    let mut seen = Vec::new();
    loop {
        let next = timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event")
            .event;
        if matches!(next, EngineEventKind::CommandsAvailable { ref commands, .. }
            if commands.iter().any(|command| command.name == "done"))
        {
            break;
        }
        seen.push(next);
    }
    handle.close().await.unwrap();
    seen
}

fn errors(events: &[EngineEventKind]) -> Vec<&EngineEventKind> {
    events
        .iter()
        .filter(|event| matches!(event, EngineEventKind::Error { .. }))
        .collect()
}

#[tokio::test]
async fn an_acknowledged_stop_ends_the_turn_idle_without_the_providers_execution_error() {
    let events = events_after_turn(stopped_turn_script(&ede_stop_tail(), false), true).await;
    assert!(
        errors(&events).is_empty(),
        "the stop raised an error: {events:?}"
    );
    let idle = events
        .iter()
        .position(|event| {
            matches!(
                event,
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle
                }
            )
        })
        .unwrap_or_else(|| panic!("the stop never went idle: {events:?}"));
    assert!(
        events[idle + 1..].iter().all(|event| matches!(
            event,
            EngineEventKind::UsageUpdated { .. } | EngineEventKind::FastmodeChanged { .. }
        )),
        "the stopped turn's tail reopened activity: {events:?}"
    );
}

#[tokio::test]
async fn an_execution_error_nobody_stopped_for_is_still_an_error() {
    let events = events_after_turn(stopped_turn_script(&ede_stop_tail(), true), false).await;
    assert!(
        matches!(
            errors(&events).as_slice(),
            [EngineEventKind::Error { message, retryable: false, .. }]
                if message.starts_with("[ede_diagnostic]")
        ),
        "{events:?}"
    );
}

#[tokio::test]
async fn after_a_stop_any_other_failure_is_still_an_error_and_a_success_stays_idle() {
    let max_turns = vec![json!({"type": "result", "subtype": "error_max_turns", "is_error": true})];
    let events = events_after_turn(stopped_turn_script(&max_turns, false), true).await;
    assert_eq!(errors(&events).len(), 1, "{events:?}");

    // Older CLIs end a stopped turn with an ordinary result, or none at all.
    let success = vec![json!({"type": "result", "subtype": "success", "is_error": false})];
    let events = events_after_turn(stopped_turn_script(&success, false), true).await;
    assert!(errors(&events).is_empty(), "{events:?}");
    assert!(matches!(
        events.last(),
        Some(EngineEventKind::StatusChanged {
            status: SessionStatus::Idle
        })
    ));
    let events = events_after_turn(stopped_turn_script(&[], false), true).await;
    assert!(errors(&events).is_empty(), "{events:?}");
}

/// A script that records the `CLAUDE_CONFIG_DIR` it was started with (or
/// `<unset>`) and then answers control requests like [`capturing_script`].
fn environment_script(record: &Path, args_path: &Path, stdin_path: &Path) -> String {
    capturing_script(args_path, stdin_path).replacen(
        "#!/bin/sh\n",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"${{CLAUDE_CONFIG_DIR-<unset>}}\" >> '{}'\n",
            record.display()
        ),
        1,
    )
}

/// Several accounts per provider (protocol 60): an added account's adapter
/// starts its CLI with that account's `CLAUDE_CONFIG_DIR` and reads history
/// from that account's `projects`; the Default account's adapter sets
/// nothing new, so its CLI sees exactly what the host inherited.
#[tokio::test]
async fn an_account_adapter_runs_its_cli_with_its_config_dir_and_reads_its_own_store() {
    let directory = tempdir().unwrap();
    let record = directory.path().join("config-dir.txt");
    let arguments = directory.path().join("args.txt");
    let stdin = directory.path().join("stdin.txt");
    let (_fixture, executable) = fake_cli(&environment_script(&record, &arguments, &stdin));
    let account = directory.path().join("accounts/claude/acct-work");
    fs::create_dir_all(account.join("projects")).unwrap();
    let launch = || ContractLaunchOptions {
        cwd: directory.path().to_string_lossy().into_owned(),
        prompt: String::new(),
        ..ContractLaunchOptions::default()
    };

    let default =
        ClaudeAdapter::new_with_projects_root(&executable, directory.path().join("home")).unwrap();
    assert!(default.environment().is_empty());
    let handle = default.create_session(launch()).unwrap();
    timeout(Duration::from_secs(30), handle.ready())
        .await
        .expect("ready in time")
        .unwrap();
    handle.close().await.unwrap();

    let work = ClaudeAdapter::new_with_projects_root(&executable, account.join("projects"))
        .unwrap()
        .with_environment(vec![(
            std::ffi::OsString::from("CLAUDE_CONFIG_DIR"),
            account.clone().into_os_string(),
        )]);
    let handle = work.create_session(launch()).unwrap();
    timeout(Duration::from_secs(30), handle.ready())
        .await
        .expect("ready in time")
        .unwrap();
    handle.close().await.unwrap();

    let inherited = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "<unset>".to_owned());
    let seen = fs::read_to_string(&record).unwrap();
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        [inherited.as_str(), account.to_str().unwrap()],
        "Default inherits; the account names its own directory"
    );

    let cwd = "/work/app";
    let project = account.join("projects").join(store::project_dir_name(cwd));
    fs::create_dir_all(&project).unwrap();
    let session_id = "11111111-1111-4111-8111-111111111111";
    fs::write(
        project.join(format!("{session_id}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"uuid\":\"22222222-2222-4222-8222-222222222222\",\"sessionId\":\"{session_id}\",\"cwd\":\"{cwd}\",\"message\":{{\"content\":\"hello\"}}}}\n"
        ),
    )
    .unwrap();
    let request = || HistoryRequest {
        cwd: cwd.into(),
        limit: 10,
    };
    let page = work
        .read_history(session_id.into(), request())
        .await
        .unwrap();
    assert_eq!(page.messages, Some(1), "the account's store holds it");
    assert_eq!(work.list_sessions().await.unwrap().sessions.len(), 1);
    assert!(default.list_sessions().await.unwrap().sessions.is_empty());
}

#[tokio::test]
async fn auto_mode_escalations_and_init_fallback_ask_the_user() {
    let directory = tempdir().unwrap();
    let responses = directory.path().join("responses.txt");
    let script = format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{}}}}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{{"type":"system","subtype":"init","session_id":"s-auto","model":"sonnet","cwd":"/work","permissionMode":"default"}}'
      printf '%s\n' '{{"type":"control_request","request_id":"approval-1","request":{{"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool-1","input":{{"file_path":"probe.txt"}}}}}}'
      ;;
    *'"type":"control_response"'*)
      printf '%s\n' "$line" >> '{response}'
      ;;
  esac
done
"#,
        response = responses.display()
    );
    let (_fixture, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let mut handle = adapter
        .create_session(ContractLaunchOptions {
            cwd: directory.path().to_string_lossy().into_owned(),
            permission_mode: Some("auto".into()),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    let mut events = handle.take_events().unwrap();
    assert!(handle.send("write it".into()).await.is_confirmed());

    let mut saw_notice = false;
    let mut saw_access_fallback = false;
    loop {
        let next = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event deadline")
            .expect("bounded queue")
            .expect("event");
        match next.event {
            EngineEventKind::Notice { message, .. } if message == AUTO_MODE_UNAVAILABLE => {
                saw_notice = true;
            }
            EngineEventKind::AccessChanged { mode } if mode == "default" => {
                saw_access_fallback = true;
            }
            EngineEventKind::ApprovalRequested { tool, .. } => {
                assert_eq!(tool, "Write");
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_notice,
        "non-agent auto fallback must emit AUTO_MODE_UNAVAILABLE"
    );
    assert!(
        saw_access_fallback,
        "system/init effective mode must update session access mode"
    );
    assert!(
        fs::read_to_string(&responses)
            .unwrap_or_default()
            .is_empty(),
        "auto mode escalation must not be auto-approved by the host"
    );
    handle.close().await.unwrap();
}

#[test]
fn accept_edits_enforces_canonical_containment_and_blocks_sensitive_paths() {
    let root = tempdir().unwrap();
    let extra = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(outside.path().join("secret.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape_dir")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        root.path().join("src/escape_file.rs"),
    )
    .unwrap();

    let additional = vec![extra.path().to_path_buf()];
    let decide = |tool: &str, input: Value| {
        host_permission_decision(
            PermissionMode::AcceptEdits,
            tool,
            Some(&input),
            root.path(),
            &additional,
            false,
        )
    };

    assert_eq!(
        decide("Write", json!({"file_path": "src/new_file.rs"})),
        HostPermissionDecision::Allow
    );
    assert_eq!(
        decide(
            "NotebookEdit",
            json!({"notebook_path": extra.path().join("analysis.ipynb").to_string_lossy()})
        ),
        HostPermissionDecision::Allow
    );

    for blocked in [
        json!({"file_path": outside.path().join("secret.txt").to_string_lossy()}),
        json!({"file_path": "../outside.txt"}),
        json!({"file_path": "escape_dir/pwn.txt"}),
        json!({"file_path": "src/escape_file.rs"}),
        json!({"file_path": ".git/hooks/pre-commit"}),
        json!({"file_path": "nested/.git/config"}),
        json!({"file_path": ".claude/settings.json"}),
        json!({"file_path": "nested/.claude/settings.local.json"}),
        json!({"file_path": ".mcp.json"}),
        json!({"file_path": "nested/.mcp.json"}),
        json!({"file_path": ".env"}),
        json!({"file_path": "config/.env.production"}),
        json!({"file_path": "keys/id_rsa"}),
    ] {
        assert_eq!(
            decide("Write", blocked.clone()),
            HostPermissionDecision::Ask,
            "AcceptEdits must require confirmation for {blocked}"
        );
    }
    assert_eq!(
        decide(
            "NotebookEdit",
            json!({"notebook_path": ".claude/notebook.ipynb"})
        ),
        HostPermissionDecision::Ask
    );
}

#[tokio::test]
async fn list_provider_commands_forces_user_setting_source_and_strict_mcp_config() {
    let directory = tempdir().unwrap();
    let recorded_args = directory.path().join("args.txt");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{args}'
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${{line#*\"request_id\":\"}}
      id=${{id%%\"*}}
      printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"commands":[{{"name":"review","description":"Review"}}]}}}}}}\n' "$id"
      ;;
  esac
done
"#,
        args = recorded_args.display()
    );
    let (_fixture, executable) = fake_cli(&script);
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let result = adapter
        .list_provider_commands(ProviderCommandRequest {
            cwd: directory.path().to_string_lossy().into_owned(),
            model: None,
            setting_sources: Some(vec!["project".into(), "local".into()]),
        })
        .await
        .unwrap();
    assert!(result.available);
    let args = fs::read_to_string(&recorded_args)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert!(args.contains(&"--setting-sources=user".to_owned()));
    assert!(args.contains(&"--strict-mcp-config".to_owned()));
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("project") || arg.contains("local"))
    );
}

#[test]
fn approval_gate_excludes_project_and_local_setting_sources() {
    let (_directory, executable) = fake_cli("#!/bin/sh\nexit 0\n");
    let adapter = ClaudeAdapter::new(executable).unwrap();
    let gated = adapter
        .create_configuration(ContractLaunchOptions {
            cwd: "/workspace".into(),
            prompt: "hi".into(),
            kind: Some(SessionKind::Agent),
            setting_sources: Some(vec!["user".into(), "project".into(), "local".into()]),
            approval_gate: Some(true),
            ..ContractLaunchOptions::default()
        })
        .unwrap();
    let args = launch_arguments(&gated.launch);
    assert!(args.contains(&"--setting-sources=user".to_owned()));
    assert!(args.contains(&"--strict-mcp-config".to_owned()));
}
