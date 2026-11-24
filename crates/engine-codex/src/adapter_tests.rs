use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use interface_contracts::engine::{
    CollaborationBindingError, CollaborationMcpSessionBinding, CollaborationMcpSessionFactory,
    CollaborationMcpSessionRequest, LaunchOptions, SessionKey,
};
use tempfile::TempDir;
use tokio::time::timeout;

use super::*;

fn fake_codex(script_body: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fake-codex");
    let source = format!("#!/usr/bin/env python3\n{script_body}\n");
    fs::write(&path, source).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    (directory, path)
}

fn adapter(path: PathBuf) -> CodexAdapter {
    let mut config = CodexAdapterConfig::new(path);
    config.transport.request_timeout = Duration::from_secs(2);
    CodexAdapter::new(config).unwrap()
}

async fn wait_until_turn_completed(events: &mut BoundedEventStream<EngineEvent>) {
    for _ in 0..32 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("Codex event timeout")
            .expect("Codex event stream failure")
            .expect("Codex event stream ended before turn completion");
        if matches!(
            event.event,
            EngineEventKind::ThinkingSignal { active: false, .. }
        ) {
            return;
        }
    }
    panic!("Codex did not report turn completion within the bounded event window");
}

async fn wait_for_command(
    events: &mut BoundedEventStream<EngineEvent>,
    name: &str,
) -> Vec<ProviderCommand> {
    for _ in 0..64 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("Codex command event timeout")
            .expect("Codex command event stream failure")
            .expect("Codex command event stream ended");
        if let EngineEventKind::CommandsAvailable { commands } = event.event
            && commands.iter().any(|command| command.name == name)
        {
            return commands;
        }
    }
    panic!("Codex did not advertise {name} within the bounded event window");
}

async fn wait_for_notice(events: &mut BoundedEventStream<EngineEvent>, needle: &str) -> String {
    for _ in 0..64 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("Codex notice event timeout")
            .expect("Codex notice event stream failure")
            .expect("Codex notice event stream ended");
        if let EngineEventKind::Notice { message, .. } = event.event
            && message.contains(needle)
        {
            return message;
        }
    }
    panic!("Codex did not emit a notice containing {needle:?}");
}

async fn wait_for_manual_compact(events: &mut BoundedEventStream<EngineEvent>) {
    for _ in 0..64 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("Codex compact event timeout")
            .expect("Codex compact event stream failure")
            .expect("Codex compact event stream ended");
        if matches!(
            event.event,
            EngineEventKind::ContextCompact {
                trigger: ContextCompactTrigger::Manual,
                ok: Some(true),
                ..
            }
        ) {
            return;
        }
    }
    panic!("Codex did not emit manual compact completion");
}

#[test]
fn launch_scoping_refuses_only_nonempty_named_tool_lists() {
    let ignored = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        cloud: Some(true),
        worktree: Some(true),
        setting_sources: Some(Vec::new()),
        allowed_tools: Some(Vec::new()),
        disallowed_tools: Some(Vec::new()),
        ..LaunchOptions::default()
    };
    validate_launch(&ignored).expect("empty and settings-only scoping launches");
    assert_eq!(
        ignored_launch_scoping(&ignored),
        vec!["settingSources", "allowedTools", "disallowedTools"]
    );

    let refused = LaunchOptions {
        allowed_tools: Some(vec!["Read".into()]),
        disallowed_tools: Some(vec!["Bash".into()]),
        ..ignored
    };
    let error = validate_launch(&refused).expect_err("named tools must fail closed");
    assert!(error.message.contains("allowedTools"));
    assert!(error.message.contains("disallowedTools"));
    assert!(error.message.contains("NOT started"));
}

#[test]
fn explicit_chat_kind_is_refused_on_launch_and_resume() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/session-kind-contract.json"
    ))
    .expect("session-kind contract fixture");
    let refusal = fixture["codexChatRefusal"]
        .as_str()
        .expect("codexChatRefusal");
    let chat = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Chat),
        ..LaunchOptions::default()
    };
    let error = validate_launch(&chat).expect_err("explicit chat must fail closed");
    assert_eq!(error.message, refusal);
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::DefinitelyUnsent
    );

    let resume = ResumeOptions {
        kind: Some(SessionKind::Chat),
        ..ResumeOptions::default()
    };
    let resume_error =
        validate_resume("thread-1", "/tmp", &resume).expect_err("explicit chat resume");
    assert_eq!(resume_error.message, refusal);
    assert_eq!(resume_error.kind, EngineErrorKind::InvalidInput);
    assert_eq!(
        resume_error.certainty,
        interface_engine::EffectCertainty::DefinitelyUnsent
    );

    validate_launch(&LaunchOptions {
        kind: Some(SessionKind::Code),
        ..chat.clone()
    })
    .expect("code launches");
    validate_launch(&LaunchOptions {
        kind: Some(SessionKind::Agent),
        ..chat.clone()
    })
    .expect("agent launches");
    validate_launch(&LaunchOptions {
        kind: None,
        ..chat.clone()
    })
    .expect("absent kind is code");
    validate_resume(
        "thread-1",
        "/tmp",
        &ResumeOptions {
            kind: Some(SessionKind::Code),
            ..ResumeOptions::default()
        },
    )
    .expect("code resumes");
    validate_resume(
        "thread-1",
        "/tmp",
        &ResumeOptions {
            kind: Some(SessionKind::Agent),
            ..ResumeOptions::default()
        },
    )
    .expect("agent resumes");
    validate_resume("thread-1", "/tmp", &ResumeOptions::default()).expect("absent kind resumes");
}

#[test]
fn resume_ignores_stored_scoping_fields_like_the_frozen_adapter() {
    let options = ResumeOptions {
        setting_sources: Some(vec!["project".into()]),
        disallowed_tools: Some(vec!["Bash".into()]),
        model: Some("gpt-5.6".into()),
        effort: Some("high".into()),
        collaboration_depth: Some(1),
        kind: None,
        ..ResumeOptions::default()
    };
    validate_resume("thread-1", "/tmp", &options)
        .expect("resume-only stored scope is ignored by Codex");
}

#[test]
fn nonsteerable_rejection_is_never_safe_to_replace_with_turn_start() {
    let nonsteerable = EngineError::definitely_rejected(
        EngineErrorKind::Provider,
        "activeTurnNotSteerable",
        false,
    );
    assert!(steer_not_steerable(&nonsteerable));
    assert!(!steer_fallback_safe(&nonsteerable));
    for message in [
        "expectedTurnId does not match",
        "no active turn",
        "turn not found",
        "turn is not active",
    ] {
        let stale = EngineError::definitely_rejected(EngineErrorKind::Provider, message, false);
        assert!(!steer_not_steerable(&stale));
        assert!(steer_fallback_safe(&stale));
    }
}

#[test]
fn model_catalog_preserves_the_exact_fast_tier_and_provider_default() {
    let (models, catalogue) = model_catalog(&json!({
        "data": [{
            "id": "display-alias",
            "model": "provider-model-id",
            "displayName": "Provider Model",
            "isDefault": true,
            "serviceTiers": [{"id": "priority", "name": "Fast"}]
        }]
    }));
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].value, "display-alias");
    assert_eq!(models[0].supports_fast_mode, Some(true));
    assert_eq!(catalogue.default_model.as_deref(), Some("display-alias"));
    assert_eq!(catalogue.tier_for(None).as_deref(), Some("priority"));
    assert_eq!(
        catalogue.tier_for(Some("provider-model-id")).as_deref(),
        Some("priority")
    );
}

#[tokio::test]
async fn ordinary_launch_omits_unselected_tier_and_approval_overrides() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        result = {}
    elif method == "thread/start":
        assert params == {"cwd":"/tmp"}
        assert "serviceTier" not in params
        assert "approvalPolicy" not in params
        result = {"thread":{"id":"ordinary-thread"}, "model":"provider-default"}
    elif method == "turn/start":
        assert params == {"threadId":"ordinary-thread", "input":[{"type":"text","text":"hello"}]}
        assert "serviceTier" not in params
        assert "approvalPolicy" not in params
        result = {"turn":{"id":"ordinary-turn"}}
    elif method == "skills/list":
        assert params == {"cwds":["/tmp"]}
        result = {"data":[{"cwd":"/tmp", "errors":[], "skills":[]}]}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "ordinary-thread");
    handle.close().await.unwrap();
}

#[tokio::test]
async fn fast_mode_uses_exact_tier_null_restore_and_model_compatibility() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
turn = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        result = {}
    elif method == "model/list":
        assert params == {}
        result = {"data":[
            {"id":"fast-model", "model":"fast-model", "isDefault":True,
             "serviceTiers":[{"id":"priority", "name":"Fast"}]},
            {"id":"standard-only", "model":"standard-only"}
        ]}
    elif method == "thread/start":
        assert params == {"cwd":"/tmp", "model":"fast-model", "serviceTier":"priority"}
        result = {"thread":{"id":"fast-thread"}, "model":"fast-model", "serviceTier":"priority"}
    elif method == "turn/start":
        turn += 1
        if turn == 1:
            assert params == {"threadId":"fast-thread", "input":[{"type":"text","text":"initial"}],
                              "model":"fast-model", "serviceTier":"priority"}
        elif turn == 2:
            assert params == {"threadId":"fast-thread", "input":[{"type":"text","text":"standard turn"}],
                              "model":"fast-model", "serviceTier":None}
        elif turn == 3:
            assert params == {"threadId":"fast-thread", "input":[{"type":"text","text":"different model"}],
                              "model":"standard-only", "serviceTier":None}
        else:
            raise AssertionError("unexpected turn")
        result = {"turn":{"id":"turn-" + str(turn)}}
    elif method == "skills/list":
        assert params == {"cwds":["/tmp"]}
        result = {"data":[{"cwd":"/tmp", "errors":[], "skills":[]}]}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
    if method == "turn/start":
        print(json.dumps({"method":"turn/completed", "params":{"threadId":"fast-thread",
            "turn":{"id":"turn-" + str(turn), "status":"completed"}}}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            model: Some("fast-model".into()),
            fast_mode: Some(true),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "fast-thread");
    wait_until_turn_completed(&mut events).await;

    assert!(handle.set_fast_mode(false).await.is_confirmed());
    assert!(handle.send("standard turn".into()).await.is_confirmed());
    wait_until_turn_completed(&mut events).await;

    assert!(handle.set_fast_mode(true).await.is_confirmed());
    assert!(
        handle
            .set_model(Some("standard-only".into()))
            .await
            .is_confirmed()
    );
    let different_model = handle.send("different model".into()).await;
    assert!(different_model.is_confirmed(), "{different_model:?}");
    wait_until_turn_completed(&mut events).await;
    handle.close().await.unwrap();
}

#[tokio::test]
async fn unsupported_fast_launch_fails_before_thread_creation() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("thread-started");
    let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let script = format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "model/list":
        result = {{"data":[{{"id":"standard-only", "model":"standard-only", "isDefault":True}}]}}
    elif method == "thread/start":
        open({marker_literal}, "w").write("sent")
        result = {{"thread":{{"id":"must-not-exist"}}}}
    else: result = {{}}
    print(json.dumps({{"id":msg.get("id"), "result":result}}), flush=True)
"#
    );
    let (_executable_directory, executable) = fake_codex(&script);
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            model: Some("standard-only".into()),
            fast_mode: Some(true),
            ..LaunchOptions::default()
        })
        .unwrap();
    let error = handle.ready().await.expect_err("unsupported Fast model");
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
    assert!(error.replay_is_safe());
    assert!(!marker.exists());
}

#[tokio::test]
async fn fast_tier_mismatch_fails_before_the_launch_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("turn-started");
    let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let script = format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "model/list":
        result = {{"data":[{{"id":"fast-model", "model":"fast-model", "isDefault":True,
            "serviceTiers":[{{"id":"priority", "name":"Fast"}}]}}]}}
    elif method == "thread/start":
        result = {{"thread":{{"id":"mismatched-thread"}}, "model":"fast-model", "serviceTier":"standard"}}
    elif method == "turn/start":
        open({marker_literal}, "w").write("sent")
        result = {{"turn":{{"id":"must-not-run"}}}}
    else: result = {{}}
    print(json.dumps({{"id":msg.get("id"), "result":result}}), flush=True)
"#
    );
    let (_executable_directory, executable) = fake_codex(&script);
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            model: Some("fast-model".into()),
            fast_mode: Some(true),
            ..LaunchOptions::default()
        })
        .unwrap();
    let error = handle.ready().await.expect_err("Fast tier mismatch");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    assert!(!marker.exists());
}

#[test]
fn child_environment_hands_the_app_server_the_augmented_search_path() {
    let environment = codex_child_environment(
        Path::new("/resolved/bin/codex"),
        Some(OsStr::new("/usr/bin:/usr/local/bin")),
        Some(Path::new("/Users/test")),
    )
    .unwrap();
    assert_eq!(
        environment,
        vec![(
            OsString::from("PATH"),
            OsString::from(
                "/resolved/bin:/Users/test/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin"
            )
        )]
    );
}

#[test]
fn read_only_launch_requires_the_reported_effective_sandbox() {
    assert!(
        verify_requested_sandbox(
            Some(SandboxMode::ReadOnly),
            &json!({"sandbox": {"type": "readOnly"}}),
        )
        .is_ok()
    );
    assert!(
        verify_requested_sandbox(
            Some(SandboxMode::ReadOnly),
            &json!({"sandbox": {"type": "readOnly", "networkAccess": false}}),
        )
        .is_ok()
    );
    for response in [
        json!({}),
        json!({"sandbox": {"type": "readOnly", "networkAccess": "false"}}),
        json!({"sandbox": {"type": "workspaceWrite"}}),
    ] {
        let error = verify_requested_sandbox(Some(SandboxMode::ReadOnly), &response)
            .expect_err("unverified read-only policy");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::OutcomeUnknown
        );
        assert!(error.message.contains("launch prompt was not sent"));
    }
    assert!(
        verify_requested_sandbox(
            Some(SandboxMode::WorkspaceWrite),
            &json!({"sandbox": {"type": "workspaceWrite"}}),
        )
        .is_ok()
    );
    for response in [
        json!({}),
        json!({"sandbox": {"type": "readOnly"}}),
        json!({"sandbox": {"type": "dangerFullAccess"}}),
        json!({"sandbox": {"type": "workspaceWrite", "networkAccess": "true"}}),
    ] {
        let error = verify_requested_sandbox(Some(SandboxMode::WorkspaceWrite), &response)
            .expect_err("unverified workspace-write policy");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::OutcomeUnknown
        );
        assert!(error.message.contains("launch prompt was not sent"));
    }
}

#[tokio::test]
async fn sandbox_mismatch_refuses_readiness_before_the_launch_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("turn-started");
    let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let script = format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "thread/start":
        result = {{"thread": {{"id": "thread-read-only"}}, "sandbox": {{"type": "workspaceWrite"}}}}
    elif method == "turn/start":
        open({marker_literal}, "w").write("sent")
        result = {{"turn": {{"id": "turn-1"}}}}
    else: result = {{}}
    print(json.dumps({{"id": msg.get("id"), "result": result}}), flush=True)
"#
    );
    let (_executable_directory, executable) = fake_codex(&script);
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            sandbox_mode: Some(SandboxMode::ReadOnly),
            ..LaunchOptions::default()
        })
        .unwrap();

    let error = handle.ready().await.expect_err("sandbox mismatch");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    assert!(!marker.exists());
}

#[derive(Default)]
struct CollaborationTestState {
    registered: Mutex<Vec<SessionKey>>,
    source_disposals: AtomicUsize,
    fence_entries: AtomicUsize,
    fence_disposals: AtomicUsize,
}

struct UnusedSessionFactory;

#[async_trait]
impl CollaborationMcpSessionFactory for UnusedSessionFactory {
    async fn create(
        &self,
        _request: CollaborationMcpSessionRequest,
    ) -> Result<Arc<dyn CollaborationMcpSessionBinding>, CollaborationBindingError> {
        Err(CollaborationBindingError::invalid())
    }
}

struct TestRegistrar {
    state: Arc<CollaborationTestState>,
}

struct TestFence {
    state: Arc<CollaborationTestState>,
}

enum LeaseKind {
    Source,
    Fence,
}

struct TestLease {
    state: Arc<CollaborationTestState>,
    kind: LeaseKind,
    disposed: AtomicBool,
}

#[async_trait]
impl CollaborationSourceRegistrar for TestRegistrar {
    async fn register(
        &self,
        source_session_key: SessionKey,
    ) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError> {
        lock(&self.state.registered).push(source_session_key);
        Ok(Arc::new(TestLease {
            state: Arc::clone(&self.state),
            kind: LeaseKind::Source,
            disposed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl CollaborationDepthFenceEntrant for TestFence {
    async fn enter(&self) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError> {
        self.state.fence_entries.fetch_add(1, Ordering::AcqRel);
        Ok(Arc::new(TestLease {
            state: Arc::clone(&self.state),
            kind: LeaseKind::Fence,
            disposed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl CollaborationLease for TestLease {
    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        match self.kind {
            LeaseKind::Source => {
                self.state.source_disposals.fetch_add(1, Ordering::AcqRel);
            }
            LeaseKind::Fence => {
                self.state.fence_disposals.fetch_add(1, Ordering::AcqRel);
            }
        }
        Ok(())
    }
}

fn collaboration_bindings(state: &Arc<CollaborationTestState>) -> CollaborationProviderBindings {
    CollaborationProviderBindings {
        session_factory: Arc::new(UnusedSessionFactory),
        codex_descriptor: CollaborationMcpStdioDescriptor {
            name: "iface_collaboration".into(),
            command: "/usr/bin/nc".into(),
            args: vec!["-U".into(), "/tmp/codex-collaboration-test.sock".into()],
            env: Vec::new(),
        },
        register_codex_source: Arc::new(TestRegistrar {
            state: Arc::clone(state),
        }),
        enter_codex_depth_fence: Arc::new(TestFence {
            state: Arc::clone(state),
        }),
    }
}

#[tokio::test]
async fn codex_process_binding_registers_sources_and_fences_depth_one_sessions() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
assert sys.argv[1:4] == ["app-server", "-c", "features.plugins=false"]
assert sys.argv[4] == "-c"
assert "mcp_servers.iface_collaboration" in sys.argv[5]
assert "env={}" in sys.argv[5]
assert "required=true" in sys.argv[5]
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        result = {}
    elif method == "thread/start":
        result = {"thread":{"id":"fresh-thread"}}
    elif method == "thread/resume":
        assert params.get("threadId") == "resume-thread"
        instructions = params.get("developerInstructions", "")
        assert "codex#resume-thread" in instructions
        assert "public label grants no authority" in instructions
        result = {"thread":{"id":"resume-thread"}}
    elif method == "turn/start":
        result = {"turn":{"id":"turn-1"}}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
    if method == "turn/start":
        print(json.dumps({"method":"turn/completed","params":{"threadId":"fresh-thread","turn":{"id":"turn-1","status":"completed"}}}), flush=True)
"#,
    );
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();

    let fresh = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(fresh.ready().await.unwrap().session_id, "fresh-thread");
    assert_eq!(lock(&state.registered).as_slice(), ["codex#fresh-thread"]);
    fresh.close().await.unwrap();
    assert_eq!(state.source_disposals.load(Ordering::Acquire), 1);

    let resumed = instance
        .resume_session(
            "resume-thread".into(),
            "/tmp".into(),
            ResumeOptions::default(),
        )
        .unwrap();
    assert_eq!(resumed.ready().await.unwrap().session_id, "resume-thread");
    assert_eq!(
        lock(&state.registered).as_slice(),
        ["codex#fresh-thread", "codex#resume-thread"]
    );
    resumed.close().await.unwrap();
    assert_eq!(state.source_disposals.load(Ordering::Acquire), 2);

    let fenced = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "depth one".into(),
            collaboration_depth: Some(1),
            ..LaunchOptions::default()
        })
        .unwrap();
    fenced.ready().await.unwrap();
    assert_eq!(state.fence_entries.load(Ordering::Acquire), 1);
    assert_eq!(lock(&state.registered).len(), 2);
    fenced.close().await.unwrap();
    assert_eq!(state.fence_disposals.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn created_thread_delivers_its_collaboration_source_on_the_first_turn_only() {
    let directory = tempfile::tempdir().unwrap();
    let turns = directory.path().join("turns.jsonl");
    let turns_literal = serde_json::to_string(&turns.to_string_lossy()).unwrap();
    let (_executable_directory, executable) = fake_codex(&format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {{}})
    if method == "initialize": result = {{}}
    elif method == "thread/start": result = {{"thread":{{"id":"fresh-thread"}}}}
    elif method == "turn/start":
        with open({turns_literal}, "a") as handle:
            handle.write(json.dumps(params.get("input")) + "\n")
        result = {{"turn":{{"id":"turn-1"}}}}
    else: result = {{}}
    print(json.dumps({{"id":msg.get("id"),"result":result}}), flush=True)
    if method == "turn/start":
        print(json.dumps({{"method":"turn/completed","params":{{"threadId":"fresh-thread","turn":{{"id":"turn-1","status":"completed"}}}}}}), flush=True)
"#
    ));
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "fresh-thread");
    assert_eq!(lock(&state.registered).as_slice(), ["codex#fresh-thread"]);
    for _ in 0..40 {
        if std::fs::read_to_string(&turns).is_ok_and(|written| !written.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(handle.send("second turn".into()).await.is_confirmed());

    let written = std::fs::read_to_string(&turns).unwrap();
    let sent: Vec<Vec<Value>> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sent.len(), 2);
    let host_context = sent[0][0]["text"].as_str().unwrap();
    assert!(host_context.starts_with("[Interface host context — not user text]\n"));
    assert!(host_context.contains("codex#fresh-thread"));
    assert!(host_context.contains("public label grants no authority"));
    assert_eq!(sent[0][1], json!({"type":"text","text":"hello"}));
    assert_eq!(sent[1], vec![json!({"type":"text","text":"second turn"})]);
    handle.close().await.unwrap();
}

#[tokio::test]
async fn required_codex_mcp_initialize_failure_restarts_once_without_broker() {
    let (_directory, executable) = fake_codex(
        r#"
import json, os, sys
collaboration = len(sys.argv) > 4
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize" and collaboration:
        os._exit(2)
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread":{"id":"fallback-thread"}}
    elif method == "turn/start": result = {"turn":{"id":"turn-1"}}
    else: result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "fallback-thread");
    assert!(lock(&state.registered).is_empty());
    let mut events = handle.take_events().unwrap();
    let mut saw_notice = false;
    for _ in 0..8 {
        let Ok(Some(event)) = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event timeout")
        else {
            break;
        };
        if matches!(event.event, EngineEventKind::Notice { ref message, .. }
            if message.contains("restarted without the broker"))
        {
            saw_notice = true;
            break;
        }
    }
    assert!(saw_notice);
    handle.close().await.unwrap();
}

fn boundary(prompt: &str, later_same_digest: u32) -> interface_engine::ForkBoundary {
    interface_engine::ForkBoundary {
        prompt_digest: interface_engine::prompt_digest(prompt),
        later_same_digest,
    }
}

/// 6 Oct 2026 (Threads review): forks counted turns from the start of the
/// transcript, so history from before the thread or a prompt sent from a
/// terminal cut at the wrong turn. The boundary names its prompt instead.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn fork_at_a_turn_finds_its_prompt_from_the_newest_turn() {
    let (directory, executable) = fake_codex(
        r#"
import json, os, sys
log = os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "forks.log")
def prompt(text):
    return {"type":"userMessage", "id":"u-" + text, "content":[{"type":"text","text":text}]}
def answer(text):
    return {"type":"agentMessage", "id":"a-" + text, "text":text}
host = "[Interface host context — not user text] consult from claude"
threads = {
    "same": [
        {"id":"t0", "status":"completed", "items":[prompt("before the thread"), answer("0")]},
        {"id":"t1", "status":"completed", "items":[prompt("one"), answer("1")]},
        {"id":"t2", "status":"completed",
            "items":[prompt("two"), prompt("two, steered"), answer("2")]},
        {"id":"t3", "status":"completed", "items":[{"type":"userMessage", "id":"h",
            "content":[{"type":"text","text":host}]}, answer("host")]},
        {"id":"t4", "status":"completed", "items":[prompt("again"), answer("3")]},
        {"id":"t5", "status":"completed", "items":[prompt("from a terminal"), answer("4")]},
        {"id":"t6", "status":"interrupted", "items":[prompt(" again\r\n"), answer("5")]},
        {"id":"t7", "status":"inProgress", "items":[prompt("four")]},
    ],
    "partial": [
        {"id":"p1", "status":"completed", "itemsView":"notLoaded", "items":[]},
        {"id":"p2", "status":"completed", "items":[prompt("later"), answer("x")]},
    ],
}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {"serverInfo":{"version":"fake-1"}}
    elif method == "thread/read":
        assert params["includeTurns"] is True
        result = {"thread":{"id":params["threadId"], "turns":threads[params["threadId"]]}}
    elif method == "thread/fork":
        with open(log, "a") as handle:
            handle.write(json.dumps(params, sort_keys=True) + "\n")
        assert params == {"threadId":params["threadId"], "cwd":"/repo/.interface-worktrees/x",
            "lastTurnId":params["lastTurnId"]}, params
        result = {"thread":{"id":"fork-" + params["lastTurnId"]}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let cwd = || Some("/repo/.interface-worktrees/x".to_owned());
    // History before the thread and a terminal's prompt shift nothing; a
    // prompt steered into t2 belongs to t2; the newest "again" (normalized
    // like a recorded prompt) is t6 and the one before it t4; only the
    // newest turns need to be loaded.
    for (session, (prompt, later), forked) in [
        ("same", ("one", 0), "fork-t1"),
        ("same", ("two", 0), "fork-t2"),
        ("same", ("again", 1), "fork-t4"),
        ("same", ("again", 0), "fork-t6"),
        ("partial", ("later", 0), "fork-p2"),
    ] {
        assert_eq!(
            instance
                .fork_session_at(session.into(), cwd(), boundary(prompt, later))
                .await
                .unwrap(),
            forked,
            "{prompt} with {later} later"
        );
    }
    for (session, (prompt, later), reason) in [
        ("same", ("four", 0), "still running"),
        ("same", ("again", 2), "can't find that message"),
        ("same", ("two, steered", 0), "can't find that message"),
        ("same", ("never sent", 0), "can't find that message"),
        ("partial", ("earlier", 0), "did not load every earlier turn"),
    ] {
        let error = instance
            .fork_session_at(session.into(), cwd(), boundary(prompt, later))
            .await
            .unwrap_err();
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent,
            "{error:?}"
        );
        assert!(!error.retryable);
        assert!(
            error.message.contains(reason),
            "{prompt} with {later} later: {}",
            error.message
        );
    }
    let sent = fs::read_to_string(directory.path().join("forks.log")).unwrap();
    assert_eq!(
        sent.lines().collect::<Vec<_>>(),
        [
            ("same", "t1"),
            ("same", "t2"),
            ("same", "t4"),
            ("same", "t6"),
            ("partial", "p2")
        ]
        .map(|(thread, turn)| format!(
            r#"{{"cwd": "/repo/.interface-worktrees/x", "lastTurnId": "{turn}", "threadId": "{thread}"}}"#
        ))
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn control_plane_matrix_uses_exact_methods_params_and_mappings() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {"serverInfo":{"version":"fake-1"}}
    elif method == "thread/list":
        assert params in ({"limit":200}, {"limit":200,"archived":True})
        archived = params.get("archived") is True
        result = {"data":[{"id":"same", "name":"Archived" if archived else "Live",
            "cwd":"/repo", "status":{"type":"idle"}, "createdAt":1,
            "updatedAt":2}]}
    elif method == "thread/name/set":
        assert params == {"threadId":"same", "name":"New name"}
        result = {}
    elif method == "thread/delete":
        assert params == {"threadId":"same"}
        result = {}
    elif method == "thread/fork":
        assert params == {"threadId":"same", "cwd":"/moved"}
        result = {"thread":{"id":"forked"}}
    elif method in ("thread/archive", "thread/unarchive"):
        assert params == {"threadId":"same"}
        result = {}
    elif method == "skills/list":
        assert params == {"cwds":["/repo"]}
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[{
            "name":"proof", "path":"/repo/.agents/skills/proof/SKILL.md",
            "shortDescription":"Use /repo/.agents/skills/proof/SKILL.md safely",
            "enabled":True}]}]}
    elif method == "thread/read":
        assert params == {"threadId":"same", "includeTurns":True}
        result = {"thread":{"turns":[{"items":[{"type":"userMessage",
            "content":[{"type":"text","text":"hello"}]}]}]}}
    elif method == "account/rateLimits/read":
        assert "params" not in msg
        result = {"rateLimits":{"planType":"pro", "primary":{
            "usedPercent":25, "windowDurationMins":300, "resetsAt":86400}}}
    elif method == "account/usage/read":
        assert "params" not in msg
        result = {"summary":{"lifetimeTokens":1000}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    assert_eq!(
        instance.supported_ops(),
        vec![
            EngineOp::RenameSession,
            EngineOp::DeleteSession,
            EngineOp::ForkSession,
            EngineOp::SetArchived,
            EngineOp::ReadUsage,
        ]
    );

    let listing = instance.list_sessions().await.unwrap();
    assert!(listing.complete);
    assert_eq!(listing.sessions.len(), 1);
    assert!(listing.sessions[0].archived);
    assert_eq!(listing.sessions[0].title, "Archived");
    // A name is Codex's own title, which replaces a launch's prompt title.
    assert_eq!(listing.sessions[0].title_from_summary, Some(true));
    instance
        .rename_session("same".into(), "New name".into(), None)
        .await
        .unwrap();
    instance.delete_session("same".into(), None).await.unwrap();
    assert_eq!(
        instance
            .fork_session("same".into(), Some("/moved".into()))
            .await
            .unwrap(),
        "forked"
    );
    instance
        .set_archived("same".into(), true, None)
        .await
        .unwrap();
    instance
        .set_archived("same".into(), false, None)
        .await
        .unwrap();

    let commands = instance
        .list_provider_commands(ProviderCommandRequest {
            cwd: "/repo".into(),
            model: Some("ignored-by-protocol".into()),
            setting_sources: None,
        })
        .await
        .unwrap();
    assert!(commands.available);
    let skill = commands
        .commands
        .iter()
        .find(|command| command.name == "skill:proof")
        .unwrap();
    assert_eq!(skill.description, "Use proof safely");

    let history = instance
        .read_history(
            "same".into(),
            HistoryRequest {
                cwd: "/ignored".into(),
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(history.messages, Some(1));
    assert!(matches!(
        history.events[0].event,
        EngineEventKind::UserMessage { ref text, .. } if text == "hello"
    ));

    let usage = instance.read_usage().await.unwrap();
    assert!(usage.available);
    assert_eq!(usage.plan.as_deref(), Some("pro"));
    assert_eq!(usage.windows[0].label, "5-hour");
}

#[tokio::test]
async fn listing_with_no_codex_installed_is_unavailable_not_a_failed_refresh() {
    let directory = tempfile::tempdir().unwrap();
    // The host hands over a path with nothing at it when it found no Codex.
    let absent = adapter(directory.path().join("codex"));
    let listing = absent.list_sessions().await.unwrap();
    assert_eq!(listing.unavailable, Some(true));
    assert!(listing.sessions.is_empty());
    assert!(!listing.complete);
    assert_eq!(listing.error.as_deref(), Some("Codex is not installed."));

    // A Codex that is there and will not start is still a failed refresh.
    let broken = directory.path().join("broken");
    fs::write(&broken, "not a program").unwrap();
    fs::set_permissions(&broken, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(adapter(broken).list_sessions().await.is_err());
}

#[tokio::test]
async fn usage_and_command_reads_reuse_the_kept_read_only_child() {
    let state = tempfile::tempdir().unwrap();
    let starts = state.path().join("starts");
    let (_directory, executable) = fake_codex(
        &r#"
import json, sys
with open("STARTS", "a") as handle:
    handle.write("started\n")
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"serverInfo":{"version":"fake-1"}}
    elif method == "account/rateLimits/read":
        result = {"rateLimits":{"planType":"pro", "primary":{
            "usedPercent":25, "windowDurationMins":300, "resetsAt":86400}}}
    elif method == "account/usage/read":
        result = {"summary":{"lifetimeTokens":1000}}
    elif method == "skills/list":
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[]}]}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#
        .replace("STARTS", starts.to_str().unwrap()),
    );
    let instance = adapter(executable);
    for _ in 0..2 {
        let usage = instance.read_usage().await.unwrap();
        assert!(usage.available);
        assert_eq!(usage.plan.as_deref(), Some("pro"));
    }
    let commands = instance
        .list_provider_commands(ProviderCommandRequest {
            cwd: "/repo".into(),
            model: None,
            setting_sources: None,
        })
        .await
        .unwrap();
    assert!(commands.available);
    assert_eq!(fs::read_to_string(&starts).unwrap(), "started\n");
}

#[tokio::test]
async fn cancelled_control_operation_reaps_the_app_server_child() {
    let state = tempfile::tempdir().unwrap();
    let pid_path = state.path().join("pid");
    let (_directory, executable) = fake_codex(
        &r#"
import json, os, sys
with open("PID_PATH", "w") as handle:
    handle.write(str(os.getpid()))
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        print(json.dumps({"id":msg.get("id"),"result":{}}), flush=True)
"#
        .replace("PID_PATH", &pid_path.to_string_lossy()),
    );
    let instance = adapter(executable);
    // Wait for the fake to start rather than for a fixed time: under load a
    // Python start can take longer than any short fixed window.
    {
        let listing = instance.list_sessions();
        tokio::pin!(listing);
        let started = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = &mut listing => panic!("the fake app-server was expected to leave thread/list unanswered"),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
            if pid_path.exists() && started.elapsed() >= Duration::from_millis(750) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "fake app-server never recorded its pid"
            );
        }
    }
    let pid = fs::read_to_string(&pid_path).expect("fake app-server never recorded its pid");
    let mut reaped = false;
    for _ in 0..100 {
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            reaped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        reaped,
        "a cancelled control operation left the Codex app-server running"
    );
}

#[tokio::test]
async fn control_operations_launch_without_the_collaboration_broker() {
    let state_directory = tempfile::tempdir().unwrap();
    let argv_path = state_directory.path().join("argv.jsonl");
    let (_directory, executable) = fake_codex(
        &r#"
import json, sys
with open("ARGV_PATH", "a") as handle:
    handle.write(json.dumps(sys.argv[1:]) + "\n")
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/list": result = {"data":[]}
    else: result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#
        .replace("ARGV_PATH", &argv_path.to_string_lossy()),
    );
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();
    instance.list_sessions().await.unwrap();
    let written = fs::read_to_string(&argv_path).unwrap();
    let argv: Vec<Vec<String>> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        argv,
        vec![vec!["app-server", "-c", "features.plugins=false"]]
    );
}

#[tokio::test]
async fn probe_uses_cli_version_and_exact_engine_capabilities() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
if sys.argv[1:] == ["--version"]:
    print("codex-cli 0.147.0")
    raise SystemExit(0)
assert sys.argv[1:] == ["app-server", "-c", "features.plugins=false"]
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"serverInfo":{"version":"must-not-win"}}
    elif method == "model/list":
        assert msg.get("params") == {}
        result = {"data":[{"id":"provider-model", "displayName":"Provider Model",
            "supportedReasoningEfforts":[{"reasoningEffort":"low"},
                {"reasoningEffort":"xhigh"}],
            "serviceTiers":[{"id":"fast","name":"Fast"}]}]}
    elif method == "account/read":
        assert msg.get("params") == {}
        result = {"account":{"type":"chatgpt","email":"person@example.test"}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let adapter = adapter(executable.clone());
    let info = adapter.probe().await.unwrap();
    // The runtime keeps this probe only while this file is unchanged.
    assert_eq!(adapter.executable(), Some(executable.clone()));
    assert_eq!(info.version, "codex-cli 0.147.0");
    assert_eq!(info.login_identity.as_deref(), Some("person@example.test"));
    assert_eq!(info.auth, Some(EngineAuth::SignedIn));
    assert_eq!(info.display_name, None);
    assert_eq!(
        info.capabilities.permission_modes,
        vec!["untrusted", "on-request", "never"]
    );
    assert_eq!(info.capabilities.effort_levels, vec!["low", "xhigh"]);
    assert!(matches!(
        info.capabilities.consultation_safety,
        Some(ConsultationSafetyProfile::Available {
            launch_options: interface_contracts::engine::ConsultationLaunchOptions {
                permission_mode: Some(ref mode),
                sandbox_mode: Some(SandboxMode::ReadOnly),
                ..
            },
            ..
        }) if mode == "on-request"
    ));
    let model = &info.model_catalog.unwrap()[0];
    assert_eq!(model.value, "provider-model");
    assert_eq!(model.supports_fast_mode, Some(true));
}

#[tokio::test]
async fn probe_asks_the_version_once_per_executable_file() {
    let script = |version: &str| {
        format!(
            r#"
import json, os, sys
if sys.argv[1:] == ["--version"]:
    with open(os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "asked"), "a") as log:
        log.write("x")
    print("{version}")
    raise SystemExit(0)
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    print(json.dumps({{"id":msg["id"], "result":{{}}}}), flush=True)
"#
        )
    };
    let (directory, executable) = fake_codex(&script("codex-cli 1.0.0"));
    let asked = || fs::read_to_string(directory.path().join("asked")).map_or(0, |log| log.len());
    let instance = adapter(executable.clone());

    assert_eq!(instance.probe().await.unwrap().version, "codex-cli 1.0.0");
    assert_eq!(instance.probe().await.unwrap().version, "codex-cli 1.0.0");
    assert_eq!(asked(), 1, "an unchanged executable is not asked again");

    // An update installs a new file at the same path (a new inode).
    let (_update, replacement) = fake_codex(&script("codex-cli 2.0.0"));
    fs::copy(&replacement, directory.path().join("next")).unwrap();
    fs::rename(directory.path().join("next"), &executable).unwrap();
    assert_eq!(instance.probe().await.unwrap().version, "codex-cli 2.0.0");
    assert_eq!(asked(), 2);
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn a_kept_version_never_hides_an_unchanged_executable_that_stopped_starting() {
    let (directory, executable) = fake_codex(
        r#"
import json, os, sys
root = os.path.dirname(os.path.abspath(sys.argv[0]))
if os.path.exists(os.path.join(root, "broken")):
    raise SystemExit(1)
if sys.argv[1:] == ["--version"]:
    with open(os.path.join(root, "asked"), "a") as log:
        log.write("x")
    print("codex-cli 1.0.0")
    raise SystemExit(0)
if os.path.exists(os.path.join(root, "no-server")):
    raise SystemExit(1)
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    print(json.dumps({"id":msg["id"], "result":{}}), flush=True)
"#,
    );
    let asked = || fs::read_to_string(directory.path().join("asked")).map_or(0, |log| log.len());
    let instance = adapter(executable.clone());
    assert_eq!(instance.probe().await.unwrap().version, "codex-cli 1.0.0");
    assert_eq!(asked(), 1);

    // Only the app-server fails to start: the version is asked again and the
    // probe fails with the start error, as an uncached probe does.
    fs::write(directory.path().join("no-server"), "").unwrap();
    let error = instance.probe().await.unwrap_err();
    assert_eq!(
        asked(),
        2,
        "a probe that saw the CLI fail to start asks again"
    );
    let uncached = adapter(executable.clone()).probe().await.unwrap_err();
    assert_eq!(asked(), 3);
    // Which of "closed stdout" and "exited" the transport notices first is a
    // race, so compare the class of the start error, not its wording.
    for start_error in [&error, &uncached] {
        assert_eq!(start_error.kind, EngineErrorKind::Transport);
        assert!(
            start_error
                .message
                .starts_with("initialize: Codex app-server "),
            "{}",
            start_error.message
        );
    }
    assert_eq!(error.certainty, uncached.certainty);
    assert_eq!(error.retryable, uncached.retryable);
    assert_ne!(error.message, "Codex version probe exited unsuccessfully");

    // The same file no longer runs at all: the version probe's error, as
    // before versions were kept.
    fs::write(directory.path().join("broken"), "").unwrap();
    let error = instance.probe().await.unwrap_err();
    let uncached = adapter(executable).probe().await.unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Unavailable);
    assert_eq!(error.message, "Codex version probe exited unsuccessfully");
    assert_eq!(error, uncached);
    instance.dispose().await.unwrap();
}

#[tokio::test]
#[ignore = "manual provider-discovery benchmark"]
async fn benchmark_warm_listing_connections() {
    let (directory, executable) = fake_codex(
        r#"
import json, sys, time, os
with open(os.path.join(os.path.dirname(__file__), "spawns"), "a") as log:
    log.write("spawn\n")
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    if msg.get("method") == "initialize":
        time.sleep(0.05)
        result = {}
    elif msg.get("method") == "thread/list": result = {"data":[]}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let start = std::time::Instant::now();
    for _ in 0..10 {
        assert!(instance.list_sessions().await.unwrap().complete);
    }
    let spawns = fs::read_to_string(directory.path().join("spawns"))
        .unwrap()
        .lines()
        .count();
    eprintln!(
        "warm listing: 10 fresh reads, {spawns} processes, {:?}",
        start.elapsed()
    );
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn slow_listing_returns_the_pages_that_arrived_within_its_budget() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys, time
page = 0
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    if msg.get("method") == "initialize": result = {}
    elif msg.get("method") == "thread/list":
        assert "archived" not in msg["params"]
        time.sleep(0.2)
        page += 1
        result = {"data":[{"id":"t" + str(page), "name":"T", "cwd":"/repo",
            "status":{"type":"idle"}, "createdAt":1, "updatedAt":2}],
            "nextCursor":"c" + str(page)}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let listing = instance.list_sessions().await.unwrap();
    assert!(!listing.complete);
    // Pages keep coming forever, so any non-empty result stopped on the budget.
    assert!(!listing.sessions.is_empty());
    assert!(listing.error.unwrap().contains("slow to list sessions"));
    // The connection is healthy, so it is kept for the next listing.
    assert!(instance.listing_connection.lock().await.is_some());
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn listing_and_history_share_one_child_with_fresh_results_and_restart_after_exit() {
    let (directory, executable) = fake_codex(
        r#"
import json, sys, os
with open(os.path.join(os.path.dirname(__file__), "spawns"), "a") as log:
    log.write("spawn\n")
reads = 0
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    if msg.get("method") == "initialize": result = {}
    elif msg.get("method") == "thread/list": result = {"data":[]}
    elif msg.get("method") == "thread/read":
        assert msg["params"] == {"threadId":"history-fixture", "includeTurns":True}
        reads += 1
        result = {"thread":{"turns":[{"items":[{"type":"userMessage",
            "content":[{"type":"text","text":"read-" + str(reads)}]}]}]}}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    // Every call lists: the runtime, not the adapter, keeps a complete listing
    // for this long (each Codex page reads the whole session store).
    assert_eq!(instance.listing_reuse(), Duration::from_secs(60));
    for index in 1..=3 {
        assert!(instance.list_sessions().await.unwrap().complete);
        let history = instance
            .read_history(
                "history-fixture".into(),
                HistoryRequest {
                    cwd: String::new(),
                    limit: 10,
                },
            )
            .await
            .unwrap();
        assert!(matches!(&history.events[0].event,
            EngineEventKind::UserMessage { text, .. } if text == &format!("read-{index}")));
    }
    let spawns = || {
        fs::read_to_string(directory.path().join("spawns"))
            .unwrap()
            .lines()
            .count()
    };
    assert_eq!(spawns(), 1);
    instance
        .listing_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .transport
        .shutdown();
    assert!(instance.list_sessions().await.unwrap().complete);
    assert_eq!(spawns(), 2);
    instance.dispose().await.unwrap();
    assert!(instance.listing_connection.lock().await.is_none());
    assert!(instance.list_sessions().await.is_err());
    assert!(
        instance
            .history_read_request("history-fixture".into())
            .await
            .is_err()
    );
    assert_eq!(spawns(), 2);
}

#[tokio::test]
async fn four_history_panes_can_read_without_blocking_each_other() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys, os, time
root = os.path.dirname(__file__)
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    if msg.get("method") == "initialize": result = {}
    elif msg.get("method") == "thread/read":
        name = msg["params"]["threadId"]
        open(os.path.join(root, "reading-" + name), "w").close()
        deadline = time.monotonic() + 3
        while len([p for p in os.listdir(root) if p.startswith("reading-")]) < 4:
            if time.monotonic() > deadline: raise AssertionError("serialized history reads")
            time.sleep(.005)
        result = {"thread":{"turns":[]}}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let (a, b, c, d) = tokio::join!(
        instance.history_read_request("a".into()),
        instance.history_read_request("b".into()),
        instance.history_read_request("c".into()),
        instance.history_read_request("d".into())
    );
    for result in [a, b, c, d] {
        assert!(result.is_ok(), "{result:?}");
    }
    instance.dispose().await.unwrap();
    assert!(instance.listing_connection.lock().await.is_none());
    for lane in instance.history_connections.iter() {
        assert!(lane.slot.lock().await.is_none());
    }
}

#[tokio::test]
async fn idle_spare_history_children_exit_and_the_listing_child_stays() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys, os, time
root = os.path.dirname(__file__)
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    if msg.get("method") == "initialize": result = {}
    elif msg.get("method") == "thread/read":
        name = msg["params"]["threadId"]
        open(os.path.join(root, "reading-" + name), "w").close()
        deadline = time.monotonic() + 3
        while len([p for p in os.listdir(root) if p.startswith("reading-")]) < 4:
            if time.monotonic() > deadline: raise AssertionError("serialized history reads")
            time.sleep(.005)
        result = {"thread":{"turns":[]}}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let mut instance = adapter(executable);
    instance.spare_idle = Duration::from_millis(100);
    let (a, b, c, d) = tokio::join!(
        instance.history_read_request("a".into()),
        instance.history_read_request("b".into()),
        instance.history_read_request("c".into()),
        instance.history_read_request("d".into())
    );
    for result in [a, b, c, d] {
        assert!(result.is_ok(), "{result:?}");
    }
    let spares = || {
        instance
            .history_connections
            .iter()
            .filter(|lane| lane.slot.try_lock().is_ok_and(|slot| slot.is_some()))
            .count()
    };
    assert_eq!(spares(), 3, "each overflow read keeps its child for reuse");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while spares() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "idle spare children outlived their idle limit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(instance.listing_connection.lock().await.is_some());
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn history_lane_discards_failed_and_cancelled_reads_without_replay() {
    let (directory, executable) = fake_codex(
        r#"
import json, sys, os, time
root = os.path.dirname(__file__)
with open(os.path.join(root, "spawns"), "a") as log: log.write("spawn\n")
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/read":
        with open(os.path.join(root, "reads"), "a") as log: log.write("read\n")
        mode = open(os.path.join(root, "mode")).read()
        if mode == "cancel":
            open(os.path.join(root, "reading"), "w").close()
            time.sleep(10)
        if mode == "error":
            print(json.dumps({"id":msg["id"], "error":{"code":-32000,"message":"fixture failure"}}), flush=True)
            continue
        result = {"thread":{"turns":[]}}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    fs::write(directory.path().join("mode"), "cancel").unwrap();
    {
        let read = instance.history_read_request("fixture".into());
        tokio::pin!(read);
        timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut read => panic!("cancelled fixture returned: {result:?}"),
                    () = tokio::time::sleep(Duration::from_millis(5)) => {
                        if directory.path().join("reading").exists() { break; }
                    }
                }
            }
        })
        .await
        .unwrap();
    }
    assert!(instance.listing_connection.lock().await.is_none());
    fs::write(directory.path().join("mode"), "error").unwrap();
    assert!(
        instance
            .history_read_request("fixture".into())
            .await
            .is_err()
    );
    assert!(instance.listing_connection.lock().await.is_none());
    fs::write(directory.path().join("mode"), "ready").unwrap();
    assert!(
        instance
            .history_read_request("fixture".into())
            .await
            .is_ok()
    );
    for name in ["reads", "spawns"] {
        assert_eq!(
            fs::read_to_string(directory.path().join(name))
                .unwrap()
                .lines()
                .count(),
            3
        );
    }
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn oversize_history_response_fails_the_read_lane_at_once() {
    let (directory, executable) = fake_codex(
        r#"
import json, sys, os
root = os.path.dirname(__file__)
with open(os.path.join(root, "spawns"), "a") as log: log.write("spawn\n")
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/read":
        if open(os.path.join(root, "mode")).read() == "huge":
            result = {"thread":{"turns":[{"text":"x" * 200000}]}}
        else:
            result = {"thread":{"turns":[]}}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let mut config = CodexAdapterConfig::new(executable);
    config.transport.request_timeout = Duration::from_secs(20);
    config.transport.max_frame_bytes = 64 * 1024;
    let instance = CodexAdapter::new(config).unwrap();
    fs::write(directory.path().join("mode"), "huge").unwrap();
    let started = std::time::Instant::now();
    let error = instance
        .history_read_request("fixture".into())
        .await
        .expect_err("an oversize thread/read cannot load");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the dropped frame left the read waiting {:?}",
        started.elapsed()
    );
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    // The failed lane is discarded, never put back or replayed.
    assert!(instance.listing_connection.lock().await.is_none());
    fs::write(directory.path().join("mode"), "small").unwrap();
    assert!(
        instance
            .history_read_request("fixture".into())
            .await
            .is_ok()
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("spawns"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn listing_lane_discards_cancelled_and_failed_reads_and_never_reuses_for_mutations() {
    let (directory, executable) = fake_codex(
        r#"
import json, sys, os, time
root = os.path.dirname(__file__)
with open(os.path.join(root, "spawns"), "a") as log: log.write("spawn\n")
listed = False
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/list":
        listed = True
        mode = open(os.path.join(root, "mode")).read()
        if mode == "cancel":
            open(os.path.join(root, "reading"), "w").close()
            time.sleep(10)
        if mode == "error":
            print(json.dumps({"id":msg["id"], "error":{"code":-32000,"message":"fixture failure"}}), flush=True)
            continue
        result = {"data":[]}
    elif method == "thread/archive":
        assert not listed, "mutation reused listing connection"
        result = {}
    else: raise AssertionError(msg)
    print(json.dumps({"id":msg["id"], "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    fs::write(directory.path().join("mode"), "cancel").unwrap();
    {
        let read = instance.list_sessions();
        tokio::pin!(read);
        timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut read => panic!("cancelled fixture unexpectedly returned: {result:?}"),
                    () = tokio::time::sleep(Duration::from_millis(5)) => {
                        if directory.path().join("reading").exists() { break; }
                    }
                }
            }
        }).await.unwrap();
    }
    assert!(instance.listing_connection.lock().await.is_none());
    fs::write(directory.path().join("mode"), "error").unwrap();
    assert!(instance.list_sessions().await.is_err());
    assert!(instance.listing_connection.lock().await.is_none());
    fs::write(directory.path().join("mode"), "ready").unwrap();
    assert!(instance.list_sessions().await.unwrap().complete);
    instance
        .control_request(
            "thread/archive",
            Some(json!({"threadId":"fixture"})),
            "archive",
        )
        .await
        .unwrap();
    assert!(instance.list_sessions().await.unwrap().complete);
    assert_eq!(
        fs::read_to_string(directory.path().join("spawns"))
            .unwrap()
            .lines()
            .count(),
        4
    );
    instance.dispose().await.unwrap();
}

#[tokio::test]
async fn fake_app_server_proves_readiness_exact_ids_and_events() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize": result = {"serverInfo": {"version": "fake-1"}}
    elif method == "thread/start": result = {"thread": {"id": "thread-exact"}, "model": "fake-model"}
    elif method == "turn/start":
        result = {"turn": {"id": "turn-1"}}
        print(json.dumps({"method":"turn/started","params":{"threadId":"thread-exact","turn":{"id":"turn-1"}}}), flush=True)
        print(json.dumps({"method":"item/agentMessage/delta","params":{"threadId":"thread-exact","delta":"hello"}}), flush=True)
    elif method == "turn/interrupt": result = {}
    else: result = {}
    print(json.dumps({"id": ident, "result": result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            model: Some("fake-model".into()),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-exact");
    assert_eq!(handle.session_id(), "thread-exact");
    let mut events = handle.take_events().unwrap();
    let mut saw_delta = false;
    for _ in 0..8 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(
            event.event,
            EngineEventKind::MessageDelta { ref text, .. } if text == "hello"
        ) {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    assert!(handle.interrupt().await.is_confirmed());
    handle.close().await.unwrap();
}

#[tokio::test]
async fn interrupt_denies_parked_approvals_then_clears_the_confirmed_turn() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
approval_denied = False
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None and ident == "approval-one":
        assert msg.get("result") == {"denied":{"rejection":"Interrupted"}}
        approval_denied = True
        continue
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread":{"id":"thread-one"}}
    elif method == "turn/start":
        result = {"turn":{"id":"turn-one"}}
        print(json.dumps({"id":ident,"result":result}), flush=True)
        print(json.dumps({"method":"turn/started","params":{"threadId":"thread-one","turn":{"id":"turn-one"}}}), flush=True)
        print(json.dumps({"id":"approval-one","method":"execCommandApproval","params":{"threadId":"thread-one","command":"git status"}}), flush=True)
        continue
    elif method == "turn/interrupt":
        assert approval_denied
        assert msg.get("params") == {"threadId":"thread-one","turnId":"turn-one"}
        result = {}
    elif method == "skills/list": result = {"data":[{"cwd":"/tmp","errors":[],"skills":[]}]}
    else: result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-one");
    let approval_id = loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = event.event {
            break approval_id;
        }
    };

    assert!(handle.interrupt().await.is_confirmed());
    assert!(
        handle
            .set_permission_mode("never".into())
            .await
            .is_confirmed(),
        "a confirmed interrupt releases the exact active-turn fence"
    );
    assert!(matches!(
        handle
            .respond_to_approval(
                approval_id,
                ApprovalVerdict::Deny,
                ApprovalResponse::default()
            )
            .await,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    handle.close().await.unwrap();
}

#[tokio::test]
async fn nonsteerable_turn_refuses_input_without_starting_a_replacement_turn() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
turn_starts = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread":{"id":"thread-one"}}
    elif method == "turn/start":
        turn_starts += 1
        assert turn_starts == 1
        result = {"turn":{"id":"turn-review"}}
    elif method == "turn/steer":
        print(json.dumps({"id":ident,"error":{"code":-32000,"message":"activeTurnNotSteerable"}}), flush=True)
        continue
    elif method == "turn/interrupt": result = {}
    elif method == "skills/list": result = {"data":[{"cwd":"/tmp","errors":[],"skills":[]}]}
    else: result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "review turn".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-one");

    let refused = handle.send("mid-turn input".into()).await;
    assert!(matches!(
        refused,
        DeliveryOutcome::DefinitelyUnsent(ref error)
            if error.message.contains("activeTurnNotSteerable")
    ));
    assert!(handle.interrupt().await.is_confirmed());
    handle.close().await.unwrap();
}

#[tokio::test]
async fn process_exit_after_turn_dispatch_is_not_claimed_unsent() {
    let (_directory, executable) = fake_codex(
        r#"
import json, os, sys
turns = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread": {"id": "thread-1"}}
    elif method == "turn/start":
        turns += 1
        if turns == 2: os._exit(0)
        result = {"turn": {"id": "turn-1"}}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
    if method == "turn/start" and turns == 1:
        print(json.dumps({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-1");
    // The next send must take the turn/start path, not steer the finished turn.
    wait_until_turn_completed(&mut events).await;
    let ambiguous = handle.send("will be ambiguous".into()).await;
    assert!(matches!(ambiguous, DeliveryOutcome::OutcomeUnknown(_)));
}

#[tokio::test]
async fn a_failed_turn_leaves_the_session_sendable() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
turns = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread": {"id": "thread-1"}}
    elif method == "turn/start":
        turns += 1
        result = {"turn": {"id": "turn-" + str(turns)}}
    else: result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
    if method == "turn/start" and turns == 1:
        print(json.dumps({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1",
            "willRetry":False,"error":{"message":"usage limit reached"}}}), flush=True)
        print(json.dumps({"method":"turn/completed","params":{"threadId":"thread-1",
            "turn":{"id":"turn-1","status":"failed","error":{"message":"usage limit reached"}}}}),
            flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-1");
    // The failed turn reports a non-retryable error, then settles.
    let mut failed_turn = false;
    for _ in 0..32 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("Codex event timeout")
            .expect("Codex event stream failure")
            .expect("Codex event stream ended before the failed turn settled");
        if let EngineEventKind::Error {
            message, retryable, ..
        } = &event.event
        {
            assert_eq!(message, "usage limit reached");
            assert!(!retryable);
            failed_turn = true;
        } else if failed_turn
            && matches!(
                event.event,
                EngineEventKind::ThinkingSignal { active: false, .. }
            )
        {
            break;
        }
    }
    assert!(failed_turn, "Codex did not report the failed turn");
    // A failed turn is not a failed session: settings and prompts still go.
    assert!(
        handle
            .set_model(Some("gpt-next".into()))
            .await
            .is_confirmed()
    );
    assert!(handle.send("try again".into()).await.is_confirmed());
    handle.close().await.unwrap();
}

#[tokio::test]
async fn app_owned_commands_are_fenced_before_launch_and_live_dispatch() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        assert params == {"threadId":"thread-1", "cwd":"/repo"}
        result = {"thread":{"id":"thread-1", "cwd":"/repo"}, "model":"provider-default"}
    elif method == "skills/list":
        assert params == {"cwds":["/repo"]}
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[]}]}
    elif method == "turn/start":
        assert params == {"threadId":"thread-1",
            "input":[{"type":"text", "text":"//model provider-default"}]}
        result = {"turn":{"id":"ordinary-turn"}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let launch_error = match instance.create_session(LaunchOptions {
        cwd: "/repo".into(),
        prompt: "/model gpt-5.6-sol".into(),
        ..LaunchOptions::default()
    }) {
        Ok(_) => panic!("app-owned launch command was accepted"),
        Err(error) => error,
    };
    assert!(launch_error.message.contains("Interface owns /model"));
    assert!(
        launch_error
            .message
            .contains("nothing was sent to the provider")
    );

    let handle = instance
        .resume_session("thread-1".into(), "/repo".into(), ResumeOptions::default())
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-1");
    for text in [
        "/rename hidden-title",
        "/set_permission never",
        "/rewind\n1",
    ] {
        let outcome = handle.send(text.into()).await;
        let DeliveryOutcome::DefinitelyUnsent(error) = outcome else {
            panic!("app-owned command {text:?} crossed the provider boundary");
        };
        assert!(error.message.contains("nothing was sent to the provider"));
    }
    assert!(
        handle
            .send("//model provider-default".into())
            .await
            .is_confirmed()
    );
    handle.close().await.unwrap();
}

/// 7 Oct 2026: outside an agent, a resume carries the approval policy its
/// owner chose for the thread (kept by the host across restarts), so Codex does
/// not fall back to its own default; a token Codex does not take is left out.
#[tokio::test]
async fn resume_sends_the_approval_policy_the_owner_chose() {
    for (requested, expected) in [
        (Some("never"), Some("never")),
        (Some("auto"), None),
        (None, None),
    ] {
        let expected_params = match expected {
            Some(policy) => format!(
                r#"{{"threadId":"thread-owner", "cwd":"/repo", "approvalPolicy":"{policy}"}}"#
            ),
            None => r#"{"threadId":"thread-owner", "cwd":"/repo"}"#.to_owned(),
        };
        let (_directory, executable) = fake_codex(&format!(
            r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "thread/resume":
        assert params == {expected_params}, params
        result = {{"thread":{{"id":"thread-owner", "cwd":"/repo"}}, "model":"gpt-5.6-sol",
                  "approvalPolicy": params.get("approvalPolicy", "on-request")}}
    else:
        result = {{}}
    if "id" in msg:
        print(json.dumps({{"id":msg.get("id"), "result":result}}), flush=True)
"#
        ));
        let instance = adapter(executable);
        let handle = instance
            .resume_session(
                "thread-owner".into(),
                "/repo".into(),
                ResumeOptions {
                    permission_mode: requested.map(Into::into),
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            handle.ready().await.unwrap().session_id,
            "thread-owner",
            "{requested:?}"
        );
        handle.close().await.unwrap();
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_commands_use_exact_native_rpcs_and_keep_skill_paths_private() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
review = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        assert params == {"threadId":"thread-commands", "cwd":"/repo"}
        result = {"thread":{"id":"thread-commands", "cwd":"/repo"},
                  "model":"gpt-5.6-sol", "approvalPolicy":"never"}
    elif method == "skills/list":
        assert params == {"cwds":["/repo"]}
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[{
            "name":"proof", "path":"/private/proof/SKILL.md", "enabled":True,
            "shortDescription":"Prove the lifecycle at /private/proof/SKILL.md"}]}]}
    elif method == "account/read":
        assert params == {}
        result = {"account":{"type":"chatgpt", "planType":"pro",
                             "email":"private@example.test"}}
    elif method == "mcpServerStatus/list":
        assert params == {}
        result = {"data":[{"name":"docs", "tools":{"search":{}},
            "resources":[{"uri":"docs://index"}], "authStatus":"oAuth"}]}
    elif method == "account/logout":
        assert "params" not in msg
        result = {}
    elif method == "review/start":
        review += 1
        expected = [
            {"threadId":"thread-commands",
             "target":{"type":"custom", "instructions":"focus on lifecycle races"},
             "delivery":"inline"},
            {"threadId":"thread-commands", "target":{"type":"baseBranch", "branch":"main"},
             "delivery":"inline"},
            {"threadId":"thread-commands",
             "target":{"type":"commit", "sha":"abc123", "title":None},
             "delivery":"inline"},
        ][review - 1]
        assert params == expected
        result = {"turn":{"id":"review-" + str(review)}}
    elif method == "turn/start":
        assert params == {"threadId":"thread-commands", "input":[
            {"type":"skill", "name":"proof", "path":"/private/proof/SKILL.md"},
            {"type":"text", "text":"check the result"}]}
        result = {"turn":{"id":"skill-turn"}}
    elif method == "thread/compact/start":
        assert params == {"threadId":"thread-commands"}
        result = {}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
    if method == "review/start":
        print(json.dumps({"method":"turn/completed", "params":{
            "threadId":"thread-commands",
            "turn":{"id":"review-" + str(review), "status":"completed"}}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"method":"turn/completed", "params":{
            "threadId":"thread-commands",
            "turn":{"id":"skill-turn", "status":"completed"}}}), flush=True)
    elif method == "thread/compact/start":
        print(json.dumps({"method":"thread/compacted",
            "params":{"threadId":"thread-commands"}}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .resume_session(
            "thread-commands".into(),
            "/repo".into(),
            ResumeOptions::default(),
        )
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-commands");
    let commands = wait_for_command(&mut events, "skill:proof").await;
    assert_eq!(
        commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "status",
            "mcp",
            "skills",
            "review",
            "review-branch",
            "review-commit",
            "compact",
            "logout",
            "skill:proof"
        ]
    );
    let serialized = serde_json::to_string(&commands).unwrap();
    assert!(!serialized.contains("/private/proof/SKILL.md"));
    assert!(serialized.contains("Prove the lifecycle at proof"));

    for command in ["/status", "/mcp", "/skills", "/logout"] {
        assert!(handle.send(command.into()).await.is_confirmed());
    }
    assert!(
        handle
            .send("/review focus on lifecycle races".into())
            .await
            .is_confirmed()
    );
    wait_until_turn_completed(&mut events).await;
    assert!(
        handle
            .send("/review-branch main".into())
            .await
            .is_confirmed()
    );
    wait_until_turn_completed(&mut events).await;
    assert!(
        handle
            .send("/review-commit abc123".into())
            .await
            .is_confirmed()
    );
    wait_until_turn_completed(&mut events).await;
    assert!(
        handle
            .send("/skill:proof check the result".into())
            .await
            .is_confirmed()
    );
    wait_until_turn_completed(&mut events).await;
    assert!(handle.send("/compact".into()).await.is_confirmed());
    wait_for_manual_compact(&mut events).await;

    handle.close().await.unwrap();
}

#[tokio::test]
async fn initial_skill_waits_for_its_cwd_catalogue_before_turn_dispatch() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys, time
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {}
    elif method == "thread/start":
        assert params == {"cwd":"/repo"}
        result = {"thread":{"id":"thread-initial-skill", "cwd":"/repo"}}
    elif method == "skills/list":
        assert params == {"cwds":["/repo"]}
        time.sleep(0.1)
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[{
            "name":"verify", "path":"/private/verify/SKILL.md", "enabled":True,
            "shortDescription":"Verify the result"}]}]}
    elif method == "turn/start":
        assert params == {"threadId":"thread-initial-skill", "input":[
            {"type":"skill", "name":"verify", "path":"/private/verify/SKILL.md"},
            {"type":"text", "text":"focus on lifecycle"}]}
        result = {"turn":{"id":"initial-skill-turn"}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
    if method == "turn/start":
        print(json.dumps({"method":"turn/completed", "params":{
            "threadId":"thread-initial-skill",
            "turn":{"id":"initial-skill-turn", "status":"completed"}}}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/repo".into(),
            prompt: "/skill:verify focus on lifecycle".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(
        handle.ready().await.unwrap().session_id,
        "thread-initial-skill"
    );
    wait_until_turn_completed(&mut events).await;
    handle.close().await.unwrap();
}

#[tokio::test]
async fn invalidated_skill_is_refused_while_its_authoritative_refresh_is_pending() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys, time
skill_reads = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params")
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        assert params == {"threadId":"thread-stale", "cwd":"/repo"}
        result = {"thread":{"id":"thread-stale", "cwd":"/repo"}, "model":"old-model"}
    elif method == "skills/list":
        assert params == {"cwds":["/repo"]}
        skill_reads += 1
        if skill_reads == 2:
            time.sleep(0.15)
        result = {"data":[{"cwd":"/repo", "errors":[], "skills":[{
            "name":"old-skill", "path":"/private/old/SKILL.md", "enabled":True,
            "shortDescription":"Old skill"}]}]}
    else:
        raise AssertionError("stale skill reached provider method " + str(method))
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .resume_session(
            "thread-stale".into(),
            "/repo".into(),
            ResumeOptions::default(),
        )
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-stale");
    wait_for_command(&mut events, "skill:old-skill").await;

    assert!(
        handle
            .set_model(Some("new-model".into()))
            .await
            .is_confirmed()
    );
    assert!(
        handle
            .send("/skill:old-skill must not become model text".into())
            .await
            .is_confirmed()
    );
    let warning = wait_for_notice(&mut events, "no longer available").await;
    assert_eq!(
        warning,
        "/skill:old-skill is no longer available for this directory."
    );
    wait_for_command(&mut events, "skill:old-skill").await;
    handle.close().await.unwrap();
}

#[test]
fn timestamps_are_stable_rfc3339() {
    assert_eq!(unix_timestamp(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(unix_timestamp(86_400), "1970-01-02T00:00:00.000Z");
}

#[test]
fn questions_preserve_answers_and_denial_is_fail_closed() {
    let pending = PendingApproval {
        rpc_id: RpcId::Number(7),
        method: "item/tool/requestUserInput".into(),
        params: Some(json!({"questions": [{"id": "choice"}]})),
        responding: false,
    };
    let response = ApprovalResponse {
        note: None,
        answers: vec![("choice".into(), "yes".into())],
    };
    assert_eq!(
        approval_result(&pending, ApprovalVerdict::Allow, &response),
        json!({"answers": {"choice": "yes"}})
    );
    assert_eq!(
        approval_result(&pending, ApprovalVerdict::Deny, &response),
        json!({"answers": {}})
    );
}

/// Codex 0.154.0's real gate echo: the thread cwd is implicitly writable and is
/// not repeated in `writableRoots`, which lists only the granted folders.
const GATED_THREAD_SERVER: &str = r#"
import json, sys
# No collaboration MCP override, then the gated feature-disable flags.
gated_features = ["apps", "browser_use", "computer_use", "hooks", "in_app_browser",
                  "multi_agent", "multi_agent_v2", "unified_exec", "unified_exec_tty",
                  "workspace_dependencies"]
expected_argv = ["app-server", "-c", "features.plugins=false"]
for feature in gated_features:
    expected_argv += ["-c", "features.%s=false" % feature]
assert sys.argv[1:] == expected_argv, sys.argv
expected_gate = {
    "approvalPolicy": "untrusted",
    "approvalsReviewer": "user",
    "sandbox": "workspace-write",
    "config": {
        "notify": [],
        "sandbox_workspace_write": {"writable_roots": ["/w/p"], "network_access": True, "exclude_slash_tmp": True},
        "mcp_servers": {"browser": {"enabled": False}, "docs": {"enabled": False}},
    },
}
gated_response = {
    "cwd": "/tmp",
    "approvalPolicy": "untrusted",
    "approvalsReviewer": "user",
    "sandbox": {"type": "workspaceWrite", "writableRoots": ["/w/p"], "networkAccess": True,
                "excludeSlashTmp": True, "excludeTmpdirEnvVar": False},
}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        assert params.get("capabilities") == {"experimentalApi": True}, params
        result = {}
    elif method == "config/read":
        assert params == {"cwd": "/tmp", "includeLayers": False}, params
        result = {"config": {"model": None, "mcp_servers": {
            "browser": {"command": "/bin/browser-mcp", "enabled": True},
            "docs": {"url": "http://127.0.0.1:9/mcp", "enabled": True}}}}
    elif method == "thread/start":
        assert params == dict(expected_gate, cwd="/tmp"), params
        result = dict(gated_response, thread={"id": "gated-thread"})
    elif method == "thread/resume":
        assert params == dict(expected_gate, threadId="gated-thread", cwd="/tmp", runtimeWorkspaceRoots=[]), params
        result = dict(gated_response, thread={"id": "gated-thread"})
    elif method == "turn/start":
        raise AssertionError("a gated idle launch must not start a turn")
    else:
        result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
"#;

#[tokio::test]
async fn approval_gate_thread_start_params() {
    let (_directory, executable) = fake_codex(GATED_THREAD_SERVER);
    // Bindings are installed, yet a gated thread never reaches the collaboration broker.
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: String::new(),
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            additional_directories: Some(vec!["/w/p".into()]),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "gated-thread");
    let refused = handle.set_permission_mode("never".into()).await;
    assert!(
        matches!(&refused, DeliveryOutcome::DefinitelyUnsent(error) if error.kind == EngineErrorKind::Unsupported),
        "a gated session cannot relax its approval policy: {refused:?}"
    );
    handle.close().await.unwrap();

    let resumed = instance
        .resume_session(
            "gated-thread".into(),
            "/tmp".into(),
            ResumeOptions {
                kind: Some(SessionKind::Agent),
                approval_gate: Some(true),
                additional_directories: Some(vec!["/w/p".into()]),
                ..ResumeOptions::default()
            },
        )
        .unwrap();
    assert_eq!(resumed.ready().await.unwrap().session_id, "gated-thread");
    resumed.close().await.unwrap();
    assert!(lock(&state.registered).is_empty());
    assert_eq!(state.fence_entries.load(Ordering::Acquire), 0);
}

/// Auto keeps the gate's sandbox, MCP and feature switches and hands escalations
/// to Codex's own reviewer instead of Interface.
const AUTO_THREAD_SERVER: &str = r#"
import json, sys
gated_features = ["apps", "browser_use", "computer_use", "hooks", "in_app_browser",
                  "multi_agent", "multi_agent_v2", "unified_exec", "unified_exec_tty",
                  "workspace_dependencies"]
expected_argv = ["app-server", "-c", "features.plugins=false"]
for feature in gated_features:
    expected_argv += ["-c", "features.%s=false" % feature]
assert sys.argv[1:] == expected_argv, sys.argv
expected = {
    "approvalPolicy": "on-request",
    "approvalsReviewer": "auto_review",
    "sandbox": "workspace-write",
    "config": {
        "notify": [],
        "sandbox_workspace_write": {"writable_roots": ["/w/p"], "network_access": True, "exclude_slash_tmp": True},
        "mcp_servers": {"browser": {"enabled": False}},
    },
}
response = {
    "cwd": "/tmp",
    "approvalPolicy": "on-request",
    "approvalsReviewer": "auto_review",
    "sandbox": {"type": "workspaceWrite", "writableRoots": ["/w/p"], "networkAccess": True,
                "excludeSlashTmp": True, "excludeTmpdirEnvVar": False},
}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        assert params.get("capabilities") == {"experimentalApi": True}, params
        result = {}
    elif method == "config/read":
        result = {"config": {"mcp_servers": {"browser": {"command": "/bin/browser-mcp", "enabled": True}}}}
    elif method == "thread/start":
        assert params == dict(expected, cwd="/tmp"), params
        result = dict(response, thread={"id": "auto-thread"})
    elif method == "thread/resume":
        assert params == dict(expected, threadId="auto-thread", cwd="/tmp", runtimeWorkspaceRoots=[]), params
        result = dict(response, thread={"id": "auto-thread"})
    else:
        result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
"#;

/// Bypass is Codex with no approvals and no sandbox: the owner's MCP servers and
/// features are left alone, so their configuration is never even read.
const BYPASS_THREAD_SERVER: &str = r#"
import json, sys
assert sys.argv[1:] == ["app-server", "-c", "features.plugins=false"], sys.argv
expected = {"approvalPolicy": "never", "sandbox": "danger-full-access"}
response = {"cwd": "/tmp", "approvalPolicy": "never", "sandbox": {"type": "dangerFullAccess"}}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {})
    if method == "initialize":
        result = {}
    elif method == "config/read":
        raise AssertionError("bypass must not read or switch off the owner's MCP servers")
    elif method == "thread/start":
        assert params == dict(expected, cwd="/tmp"), params
        result = dict(response, thread={"id": "bypass-thread"})
    elif method == "thread/resume":
        assert params == dict(expected, threadId="bypass-thread", cwd="/tmp", runtimeWorkspaceRoots=[]), params
        result = dict(response, thread={"id": "bypass-thread"})
    else:
        result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
"#;

#[tokio::test]
async fn agent_access_threads_use_codex_auto_review_or_full_bypass() {
    use interface_contracts::engine::AgentAccess;
    for (server, access, thread) in [
        (AUTO_THREAD_SERVER, AgentAccess::Auto, "auto-thread"),
        (BYPASS_THREAD_SERVER, AgentAccess::Bypass, "bypass-thread"),
    ] {
        let (_directory, executable) = fake_codex(server);
        let state = Arc::new(CollaborationTestState::default());
        let instance = adapter(executable)
            .with_collaboration_bindings(&collaboration_bindings(&state))
            .unwrap();
        let handle = instance
            .create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: String::new(),
                kind: Some(SessionKind::Agent),
                agent_access: Some(access),
                additional_directories: Some(vec!["/w/p".into()]),
                ..LaunchOptions::default()
            })
            .unwrap();
        assert_eq!(handle.ready().await.unwrap().session_id, thread);
        let refused = handle.set_permission_mode("untrusted".into()).await;
        assert!(
            matches!(&refused, DeliveryOutcome::DefinitelyUnsent(error)
                if error.kind == EngineErrorKind::Unsupported
                    && error.message.contains("comes from its settings")),
            "{access:?}: {refused:?}"
        );
        handle.close().await.unwrap();
        let resumed = instance
            .resume_session(
                thread.into(),
                "/tmp".into(),
                ResumeOptions {
                    kind: Some(SessionKind::Agent),
                    agent_access: Some(access),
                    additional_directories: Some(vec!["/w/p".into()]),
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        assert_eq!(resumed.ready().await.unwrap().session_id, thread);
        resumed.close().await.unwrap();
        // Agent sessions never reach cross-model collaboration, in any mode.
        assert!(lock(&state.registered).is_empty());
    }
}

#[test]
fn agent_access_verification_checks_each_modes_echo() {
    let auto = GateRequest {
        mode: AgentMode::Auto,
        cwd: "/tmp".to_owned(),
        directories: vec!["/w/p".to_owned()],
    };
    let auto_echo = json!({
        "cwd": "/tmp",
        "approvalPolicy": "on-request",
        "approvalsReviewer": "auto_review",
        "sandbox": {"type": "workspaceWrite", "writableRoots": ["/w/p"], "networkAccess": true, "excludeSlashTmp": true},
    });
    verify_gate(Some(&auto), &auto_echo).expect("matching auto thread");
    let mut user_review = auto_echo.clone();
    user_review["approvalsReviewer"] = json!("user");
    let error = verify_gate(Some(&auto), &user_review).expect_err("reviewer must be auto_review");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    let bypass = GateRequest {
        mode: AgentMode::Bypass,
        cwd: "/tmp".to_owned(),
        directories: vec!["/w/p".to_owned()],
    };
    let bypass_echo =
        json!({"cwd": "/tmp", "approvalPolicy": "never", "sandbox": {"type": "dangerFullAccess"}});
    verify_gate(Some(&bypass), &bypass_echo).expect("matching bypass thread");
    for (pointer, value) in [
        ("/approvalPolicy", json!("untrusted")),
        ("/sandbox/type", json!("workspaceWrite")),
    ] {
        let mut changed = bypass_echo.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(verify_gate(Some(&bypass), &changed).is_err(), "{pointer}");
    }
}

#[test]
fn agent_access_launches_validate_like_the_gate() {
    use interface_contracts::engine::AgentAccess;
    let auto = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Agent),
        agent_access: Some(AgentAccess::Auto),
        additional_directories: Some(vec!["/w/p".into()]),
        ..LaunchOptions::default()
    };
    validate_launch(&auto).expect("an auto agent may carry its folders");
    for refused in [
        LaunchOptions {
            approval_gate: Some(true),
            ..auto.clone()
        },
        LaunchOptions {
            kind: Some(SessionKind::Code),
            ..auto.clone()
        },
        LaunchOptions {
            sandbox_mode: Some(SandboxMode::ReadOnly),
            ..auto.clone()
        },
        LaunchOptions {
            permission_mode: Some("untrusted".into()),
            ..auto.clone()
        },
    ] {
        let error = validate_launch(&refused).expect_err("refused access launch");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
    }
}

/// A gated thread must not start unless every owner MCP server can be switched off first.
#[tokio::test]
async fn gated_launch_is_not_started_when_codex_cannot_list_its_mcp_servers() {
    for config_read in [
        r#"{"config": {"model": None}}"#,
        r#"{"config": {"mcp_servers": None}}"#,
        r#"{"config": {"mcp_servers": ["browser"]}}"#,
        r"{}",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("thread-started");
        let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
        let (_executable_directory, executable) = fake_codex(&format!(
            r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "config/read": result = {config_read}
    elif method in ("thread/start", "thread/resume"):
        open({marker_literal}, "w").write("started")
        result = {{"thread": {{"id": "gated-thread"}}}}
    else: result = {{}}
    print(json.dumps({{"id": msg.get("id"), "result": result}}), flush=True)
"#
        ));
        let instance = adapter(executable);
        let created = instance
            .create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: "must remain unsent".into(),
                kind: Some(SessionKind::Agent),
                approval_gate: Some(true),
                ..LaunchOptions::default()
            })
            .unwrap();
        let error = created.ready().await.expect_err("unlisted MCP servers");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent,
            "{config_read}: {}",
            error.message
        );
        assert!(error.message.contains("MCP"), "{}", error.message);
        let resumed = instance
            .resume_session(
                "gated-thread".into(),
                "/tmp".into(),
                ResumeOptions {
                    kind: Some(SessionKind::Agent),
                    approval_gate: Some(true),
                    ..ResumeOptions::default()
                },
            )
            .unwrap();
        let error = resumed.ready().await.expect_err("unlisted MCP servers");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
        assert!(!marker.exists(), "{config_read}: a thread was opened");
    }
}

/// A gated thread must not start if `config/read` still reports an acting feature enabled after
/// the app-server was spawned with it disabled. A `features` table that omits it, or none at all,
/// is fine: the spawn flags are the authoritative disable.
#[tokio::test]
async fn gated_launch_is_not_started_when_an_acting_feature_is_still_enabled() {
    let refuse = [
        r#"{"browser_use": True}"#,
        r#"{"computer_use": {"enabled": True}}"#,
        r#"{"unified_exec": "on"}"#,
    ];
    let allow = [
        r#"{"browser_use": False}"#,
        r#"{"computer_use": {"enabled": False}}"#,
        r#"{"apps": None}"#,
        r#"{"shell_tool": True}"#,
        r#"{"code_mode_host": True}"#,
        r"{}",
    ];
    for (features, refused) in refuse
        .iter()
        .map(|f| (f, true))
        .chain(allow.iter().map(|f| (f, false)))
    {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("thread-started");
        let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
        let (_executable_directory, executable) = fake_codex(&format!(
            r#"
import json, sys
gated_response = {{
    "cwd": "/tmp", "approvalPolicy": "untrusted", "approvalsReviewer": "user",
    "sandbox": {{"type": "workspaceWrite", "writableRoots": [], "networkAccess": True,
                "excludeSlashTmp": True, "excludeTmpdirEnvVar": False}},
}}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "config/read":
        result = {{"config": {{"mcp_servers": {{}}, "features": {features}}}}}
    elif method in ("thread/start", "thread/resume"):
        open({marker_literal}, "w").write("started")
        result = dict(gated_response, thread={{"id": "gated-thread"}})
    else: result = {{}}
    print(json.dumps({{"id": msg.get("id"), "result": result}}), flush=True)
"#
        ));
        let instance = adapter(executable);
        let created = instance
            .create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: String::new(),
                kind: Some(SessionKind::Agent),
                approval_gate: Some(true),
                ..LaunchOptions::default()
            })
            .unwrap();
        let ready = created.ready().await;
        if refused {
            let error = ready.expect_err("acting feature still enabled");
            assert_eq!(
                error.certainty,
                interface_engine::EffectCertainty::DefinitelyUnsent,
                "{features}: {}",
                error.message
            );
            assert!(error.message.contains("features"), "{}", error.message);
            assert!(!marker.exists(), "{features}: a thread was opened");
        } else {
            ready.expect("gated launch with the feature off");
            assert!(marker.exists(), "{features}: the thread did not start");
        }
    }
}

/// Codex echoes the thread's working directory; a different one is not the gate Interface asked for.
#[tokio::test]
async fn gated_launch_refuses_a_thread_opened_in_another_directory() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "config/read": result = {"config": {"mcp_servers": {}}}
    elif method == "thread/start":
        result = {"thread": {"id": "gated-thread"}, "cwd": "/elsewhere", "approvalPolicy": "untrusted",
                  "approvalsReviewer": "user",
                  "sandbox": {"type": "workspaceWrite", "writableRoots": ["/tmp", "/elsewhere"],
                              "networkAccess": True, "excludeSlashTmp": True}}
    elif method == "turn/start": raise AssertionError("the launch prompt must not be sent")
    else: result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
"#,
    );
    let handle = adapter(executable)
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..LaunchOptions::default()
        })
        .unwrap();
    let error = handle.ready().await.expect_err("foreign working directory");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    assert!(
        error.message.contains("working directory"),
        "{}",
        error.message
    );
}

#[test]
fn ungated_thread_params_keep_their_frozen_wire_shape() {
    let start = serde_json::to_value(ThreadStartParams {
        cwd: "/tmp".into(),
        model: None,
        service_tier: None,
        approval_policy: None,
        approvals_reviewer: None,
        sandbox: None,
        config: None,
    })
    .unwrap();
    assert_eq!(start, json!({"cwd": "/tmp"}));
    let resume = serde_json::to_value(ThreadResumeParams {
        thread_id: "thread-1".into(),
        cwd: "/tmp".into(),
        developer_instructions: None,
        runtime_workspace_roots: None,
        approval_policy: None,
        approvals_reviewer: None,
        sandbox: None,
        config: None,
    })
    .unwrap();
    assert_eq!(resume, json!({"threadId": "thread-1", "cwd": "/tmp"}));
}

#[test]
fn approval_gate_verification() {
    let gate = GateRequest {
        mode: AgentMode::Gated,
        cwd: "/tmp".to_owned(),
        directories: vec!["/w/p".to_owned()],
    };
    let verified = json!({
        "cwd": "/tmp",
        "approvalPolicy": "untrusted",
        "approvalsReviewer": "user",
        "sandbox": {"type": "workspaceWrite", "writableRoots": ["/w/p/"], "networkAccess": true, "excludeSlashTmp": true, "excludeTmpdirEnvVar": false},
    });
    verify_gate(Some(&gate), &verified).expect("matching gate");
    verify_gate(None, &json!({})).expect("an ungated launch verifies nothing");
    let no_folders = GateRequest {
        mode: AgentMode::Gated,
        cwd: "/tmp".to_owned(),
        directories: Vec::new(),
    };
    let mut bare = verified.clone();
    bare["sandbox"]["writableRoots"] = json!([]);
    verify_gate(Some(&no_folders), &bare).expect("the cwd is not repeated in writableRoots");
    bare["sandbox"]
        .as_object_mut()
        .unwrap()
        .remove("writableRoots");
    verify_gate(Some(&no_folders), &bare)
        .expect_err("empty grants still need an explicit root set");

    let with = |pointer: &str, value: Value| {
        let mut response = verified.clone();
        *response.pointer_mut(pointer).expect("pointer") = value;
        response
    };
    let without = |key: &str| {
        let mut response = verified.clone();
        response["sandbox"].as_object_mut().unwrap().remove(key);
        response
    };
    let cases = [
        (with("/approvalPolicy", json!("never")), "approval policy"),
        (
            with("/approvalPolicy", json!({"granular": {}})),
            "approval policy",
        ),
        (
            {
                let mut response = verified.clone();
                response.as_object_mut().unwrap().remove("approvalPolicy");
                response
            },
            "approval policy",
        ),
        (with("/approvalsReviewer", json!("auto_review")), "reviewer"),
        (with("/sandbox/type", json!("dangerFullAccess")), "sandbox"),
        (with("/sandbox", json!("workspace-write")), "sandbox"),
        (with("/sandbox/networkAccess", json!(false)), "network"),
        (without("networkAccess"), "network"),
        (
            with("/sandbox/excludeSlashTmp", json!(false)),
            "excludeSlashTmp",
        ),
        (
            with("/sandbox/excludeSlashTmp", json!("true")),
            "excludeSlashTmp",
        ),
        (without("excludeSlashTmp"), "excludeSlashTmp"),
        (with("/cwd", json!("/elsewhere")), "working directory"),
        (with("/cwd", json!(null)), "working directory"),
        (
            {
                let mut response = verified.clone();
                response.as_object_mut().unwrap().remove("cwd");
                response
            },
            "working directory",
        ),
        (with("/sandbox/writableRoots", json!([])), "/w/p"),
        (without("writableRoots"), "missing"),
        (
            with("/sandbox/writableRoots", json!(["/w/p", "/"])),
            "unrequested",
        ),
        (
            with("/sandbox/writableRoots", json!(["/w/p", null])),
            "malformed",
        ),
    ];
    for (response, needle) in cases {
        let error = verify_gate(Some(&gate), &response).expect_err("unverified gate");
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::OutcomeUnknown,
            "{response}"
        );
        assert!(
            error.message.contains(needle) && error.message.contains("launch prompt was not sent"),
            "{needle}: {}",
            error.message
        );
    }
}

#[test]
fn implicit_temp_echo_requires_the_exact_inherited_private_directory_and_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let temp = directory.path().to_str().unwrap();
    let allowed = json!({"excludeTmpdirEnvVar": false});
    let allowed = allowed.as_object().unwrap();
    assert!(super::matches_reported_implicit_temp(temp, allowed, temp));
    for root in ["/", "/tmp", "/private/tmp"] {
        assert!(!super::matches_reported_implicit_temp(root, allowed, temp));
        assert!(!super::matches_reported_implicit_temp(root, allowed, root));
    }
    for metadata in [
        json!({}),
        json!({"excludeTmpdirEnvVar": true}),
        json!({"excludeTmpdirEnvVar": "false"}),
    ] {
        assert!(!super::matches_reported_implicit_temp(
            temp,
            metadata.as_object().unwrap(),
            temp
        ));
    }
}

/// The cwd and each granted folder compare resolved on both sides, so a link or macOS's
/// `/tmp` -> `/private/tmp` spelling is the same directory.
#[test]
fn approval_gate_verification_compares_resolved_directories() {
    let directory = tempfile::tempdir().unwrap();
    let real = fs::canonicalize(directory.path()).unwrap();
    fs::create_dir_all(real.join("home")).unwrap();
    fs::create_dir_all(real.join("folder")).unwrap();
    std::os::unix::fs::symlink(real.join("home"), real.join("home-link")).unwrap();
    std::os::unix::fs::symlink(real.join("folder"), real.join("folder-link")).unwrap();
    let text = |name: &str| real.join(name).to_string_lossy().into_owned();
    let response = |cwd: String, roots: Vec<String>| {
        json!({
            "cwd": cwd,
            "approvalPolicy": "untrusted",
            "approvalsReviewer": "user",
            "sandbox": {"type": "workspaceWrite", "writableRoots": roots, "networkAccess": true, "excludeSlashTmp": true},
        })
    };
    let gate = GateRequest {
        mode: AgentMode::Gated,
        cwd: text("home-link"),
        directories: vec![text("folder-link")],
    };
    verify_gate(Some(&gate), &response(text("home"), vec![text("folder")]))
        .expect("requested links match their resolved echo");
    let gate = GateRequest {
        mode: AgentMode::Gated,
        cwd: text("home"),
        directories: vec![text("folder")],
    };
    verify_gate(
        Some(&gate),
        &response(text("home-link"), vec![text("folder-link")]),
    )
    .expect("echoed links match the resolved request");
    let error = verify_gate(Some(&gate), &response(text("folder"), vec![text("folder")]))
        .expect_err("another existing directory");
    assert!(
        error.message.contains("working directory"),
        "{}",
        error.message
    );
    if Path::new("/private/tmp").is_dir() {
        let gate = GateRequest {
            mode: AgentMode::Gated,
            cwd: "/tmp".to_owned(),
            directories: Vec::new(),
        };
        verify_gate(
            Some(&gate),
            &response("/private/tmp".to_owned(), Vec::new()),
        )
        .expect("/tmp and /private/tmp are one directory");
    }
    // A granted folder that is the cwd itself is not requested again.
    let gate = gate_request(
        Some(AgentMode::Gated),
        &text("home"),
        Some(&[text("home-link"), text("folder")]),
    )
    .expect("gated");
    assert_eq!(gate.directories, [text("folder")]);
}

#[test]
fn gate_overrides_turn_off_every_listed_mcp_server() {
    let gate = GateRequest {
        mode: AgentMode::Gated,
        cwd: "/tmp".to_owned(),
        directories: vec!["/w/p".to_owned()],
    };
    assert_eq!(
        gate_overrides(&gate, &[]),
        json!({"notify": [], "sandbox_workspace_write": {"writable_roots": ["/w/p"], "network_access": true, "exclude_slash_tmp": true}})
    );
    assert_eq!(
        gate_overrides(&gate, &["browser".to_owned(), "docs".to_owned()])["mcp_servers"],
        json!({"browser": {"enabled": false}, "docs": {"enabled": false}})
    );
    assert_eq!(
        configured_mcp_servers(
            &json!({"config": {"mcp_servers": {"b": {}, "a": {"enabled": false}}}})
        ),
        Some(vec!["a".to_owned(), "b".to_owned()])
    );
    assert_eq!(
        configured_mcp_servers(&json!({"config": {"mcp_servers": {}}})),
        Some(Vec::new())
    );
    for unlisted in [
        json!({}),
        json!({"config": {}}),
        json!({"config": {"mcp_servers": null}}),
        json!({"config": {"mcp_servers": ["a"]}}),
    ] {
        assert_eq!(configured_mcp_servers(&unlisted), None, "{unlisted}");
    }
}

#[tokio::test]
async fn approval_gate_mismatch_refuses_readiness_before_the_launch_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("turn-started");
    let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let script = format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "config/read": result = {{"config": {{"mcp_servers": {{}}}}}}
    elif method == "thread/start":
        result = {{"thread": {{"id": "gated-thread"}}, "cwd": "/tmp", "approvalPolicy": "untrusted", "approvalsReviewer": "user",
                   "sandbox": {{"type": "workspaceWrite", "writableRoots": [], "networkAccess": False}}}}
    elif method == "turn/start":
        open({marker_literal}, "w").write("sent")
        result = {{"turn": {{"id": "turn-1"}}}}
    else: result = {{}}
    print(json.dumps({{"id": msg.get("id"), "result": result}}), flush=True)
"#
    );
    let (_executable_directory, executable) = fake_codex(&script);
    let instance = adapter(executable);
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            approval_gate: Some(true),
            ..LaunchOptions::default()
        })
        .unwrap();
    let error = handle.ready().await.expect_err("gate mismatch");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
    assert!(error.message.contains("network"), "{}", error.message);
    assert!(!marker.exists());
}

/// How an open is launched in [`open_refusals_keep_their_precedence`].
#[derive(Clone, Copy, Debug)]
enum PrecedenceOpen {
    /// A Fast launch, read-only or gated.
    Create { read_only: bool, gated: bool },
    /// A resume of `requested-thread`.
    Resume { gated: bool },
}

/// Opens a session against a fake Codex whose `thread/start` and
/// `thread/resume` answer `thread`, and returns the open's refusal and whether
/// a thread was requested. The launch prompt must never be sent.
async fn refused_open(
    open: PrecedenceOpen,
    model_list: &str,
    config_read: &str,
    thread: &str,
) -> (EngineError, bool) {
    const SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {}
    elif method == "model/list": result = MODEL_LIST
    elif method == "config/read": result = CONFIG_READ
    elif method in ("thread/start", "thread/resume"):
        open(THREAD_MARKER, "w").write("started")
        result = THREAD
    elif method == "turn/start":
        open(TURN_MARKER, "w").write("sent")
        result = {"turn": {"id": "turn-1"}}
    else: result = {}
    print(json.dumps({"id": msg.get("id"), "result": result}), flush=True)
"#;
    let directory = tempfile::tempdir().unwrap();
    let thread_marker = directory.path().join("thread-started");
    let turn_marker = directory.path().join("turn-started");
    let literal = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let script = SERVER
        .replace("MODEL_LIST", model_list)
        .replace("CONFIG_READ", config_read)
        .replace("THREAD_MARKER", &literal(&thread_marker))
        .replace("TURN_MARKER", &literal(&turn_marker))
        .replace("THREAD", thread);
    let (_executable_directory, executable) = fake_codex(&script);
    let instance = adapter(executable);
    let handle = match open {
        PrecedenceOpen::Create { read_only, gated } => instance.create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "must remain unsent".into(),
            model: Some("fast-model".into()),
            fast_mode: Some(true),
            sandbox_mode: read_only.then_some(SandboxMode::ReadOnly),
            kind: gated.then_some(SessionKind::Agent),
            approval_gate: gated.then_some(true),
            ..LaunchOptions::default()
        }),
        PrecedenceOpen::Resume { gated } => instance.resume_session(
            "requested-thread".into(),
            "/tmp".into(),
            ResumeOptions {
                kind: gated.then_some(SessionKind::Agent),
                approval_gate: gated.then_some(true),
                ..ResumeOptions::default()
            },
        ),
    }
    .unwrap();
    let error = handle.ready().await.expect_err("refused open");
    assert!(
        !turn_marker.exists(),
        "{open:?} {thread}: launch prompt sent"
    );
    (error, thread_marker.exists())
}

/// When several checks would refuse an open, the first in today's order wins:
/// the Fast tier is resolved before the gate's `config/read`, and the opened
/// thread is checked for its sandbox, then the gate, then the Fast tier, then an
/// exact provider id, then the resumed id. No refusal sends the launch prompt.
#[tokio::test]
async fn open_refusals_keep_their_precedence() {
    const FAST: &str = r#"{"data": [{"id": "fast-model", "model": "fast-model", "isDefault": True,
    "serviceTiers": [{"id": "priority", "name": "Fast"}]}]}"#;
    const STANDARD_ONLY: &str =
        r#"{"data": [{"id": "fast-model", "model": "fast-model", "isDefault": True}]}"#;
    const MCP_LISTED: &str = r#"{"config": {"mcp_servers": {}}}"#;
    // A gated response whose sandbox refuses the network the gate asked for.
    const GATE_MISMATCH: &str = r#""cwd": "/tmp", "approvalPolicy": "untrusted",
    "approvalsReviewer": "user", "sandbox": {"type": "workspaceWrite", "writableRoots": [],
    "networkAccess": False, "excludeSlashTmp": True}"#;
    use interface_engine::EffectCertainty::{DefinitelyRejected, DefinitelyUnsent, OutcomeUnknown};
    let read_only = PrecedenceOpen::Create {
        read_only: true,
        gated: false,
    };
    let gated = PrecedenceOpen::Create {
        read_only: false,
        gated: true,
    };
    let resume = |gated| PrecedenceOpen::Resume { gated };
    let cases = [
        (
            read_only,
            FAST,
            "{}",
            r#"{"sandbox": {"type": "workspaceWrite"}, "serviceTier": "standard"}"#.to_owned(),
            "after Interface requested read-only",
            OutcomeUnknown,
        ),
        (
            read_only,
            FAST,
            "{}",
            r#"{"sandbox": {"type": "readOnly"}, "serviceTier": "standard"}"#.to_owned(),
            "requested Fast service tier",
            OutcomeUnknown,
        ),
        (
            read_only,
            FAST,
            "{}",
            r#"{"sandbox": {"type": "readOnly"}, "serviceTier": "priority"}"#.to_owned(),
            "without an exact provider id",
            DefinitelyRejected,
        ),
        (
            gated,
            FAST,
            MCP_LISTED,
            format!(r#"{{{GATE_MISMATCH}, "serviceTier": "standard"}}"#),
            "network",
            OutcomeUnknown,
        ),
        (
            gated,
            STANDARD_ONLY,
            "{}",
            r#"{"thread": {"id": "must-not-exist"}}"#.to_owned(),
            "does not advertise a Fast service tier",
            DefinitelyUnsent,
        ),
        (
            resume(true),
            "{}",
            MCP_LISTED,
            format!(r#"{{{GATE_MISMATCH}, "thread": {{"id": "other-thread"}}}}"#),
            "network",
            OutcomeUnknown,
        ),
        (
            resume(false),
            "{}",
            "{}",
            "{}".to_owned(),
            "without an exact provider id",
            DefinitelyRejected,
        ),
        (
            resume(false),
            "{}",
            "{}",
            r#"{"thread": {"id": "other-thread"}}"#.to_owned(),
            "different thread id",
            DefinitelyRejected,
        ),
    ];
    for (open, model_list, config_read, thread, expected, certainty) in cases {
        let (error, thread_requested) = refused_open(open, model_list, config_read, &thread).await;
        assert!(
            error.message.contains(expected),
            "{open:?} {thread}: expected {expected:?}, got {}",
            error.message
        );
        assert_eq!(error.certainty, certainty, "{open:?} {thread}");
        assert_eq!(
            thread_requested,
            certainty != DefinitelyUnsent,
            "{open:?} {thread}: thread request"
        );
    }
}

#[test]
fn approval_gate_rejects_conflicting_modes() {
    let gated = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Agent),
        approval_gate: Some(true),
        ..LaunchOptions::default()
    };
    validate_launch(&gated).expect("plain gated launch");
    validate_launch(&LaunchOptions {
        sandbox_mode: Some(SandboxMode::WorkspaceWrite),
        permission_mode: Some("untrusted".into()),
        additional_directories: Some(vec!["/w/p".into()]),
        ..gated.clone()
    })
    .expect("options the gate already implies");
    let definitely_unsent = |error: EngineError| {
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
        error
    };
    for conflicting in [
        LaunchOptions {
            sandbox_mode: Some(SandboxMode::ReadOnly),
            ..gated.clone()
        },
        LaunchOptions {
            sandbox_mode: Some(SandboxMode::DangerFullAccess),
            ..gated.clone()
        },
        LaunchOptions {
            permission_mode: Some("never".into()),
            ..gated.clone()
        },
        LaunchOptions {
            permission_mode: Some("on-request".into()),
            ..gated.clone()
        },
    ] {
        let error = definitely_unsent(validate_launch(&conflicting).expect_err("conflict"));
        assert_eq!(error.kind, EngineErrorKind::Unsupported);
    }
    let error = definitely_unsent(
        validate_launch(&LaunchOptions {
            additional_directories: Some(vec!["relative/dir".into()]),
            ..gated.clone()
        })
        .expect_err("relative directory"),
    );
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    let error = definitely_unsent(
        validate_launch(&LaunchOptions {
            approval_gate: None,
            additional_directories: Some(vec!["/w/p".into()]),
            ..gated.clone()
        })
        .expect_err("ungated directory grant"),
    );
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
    validate_launch(&LaunchOptions {
        approval_gate: Some(false),
        additional_directories: Some(Vec::new()),
        sandbox_mode: Some(SandboxMode::ReadOnly),
        ..gated.clone()
    })
    .expect("an explicit false gate keeps ordinary launch rules");

    let gated_resume = ResumeOptions {
        approval_gate: Some(true),
        additional_directories: Some(vec!["/w/p".into()]),
        ..ResumeOptions::default()
    };
    validate_resume("thread-1", "/tmp", &gated_resume).expect("gated resume");
    let error = definitely_unsent(
        validate_resume(
            "thread-1",
            "/tmp",
            &ResumeOptions {
                additional_directories: Some(vec!["relative".into()]),
                ..gated_resume.clone()
            },
        )
        .expect_err("relative resume directory"),
    );
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
    let error = definitely_unsent(
        validate_resume(
            "thread-1",
            "/tmp",
            &ResumeOptions {
                approval_gate: None,
                ..gated_resume
            },
        )
        .expect_err("ungated resume directory grant"),
    );
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
}

#[test]
#[allow(clippy::too_many_lines)]
fn codex_approval_facts_map_commands_file_changes_and_the_rest() {
    use interface_contracts::agents::ApprovalFacts;
    let instance = adapter(PathBuf::from("/usr/bin/false"));
    let unknown = |tool: &str| ApprovalFacts::Unknown { tool: tool.into() };
    let rows = [
        (
            "command",
            json!({"itemId": "i", "command": "git status", "cwd": "/repo"}),
            ApprovalFacts::Shell {
                command: "git status".into(),
                cwd: Some("/repo".into()),
            },
        ),
        (
            "command",
            json!({"command": ["git", "push", "origin"]}),
            ApprovalFacts::Shell {
                command: "git push origin".into(),
                cwd: None,
            },
        ),
        ("command", json!({"command": null}), unknown("command")),
        ("command", json!({"command": ""}), unknown("command")),
        ("command", json!({"command": []}), unknown("command")),
        (
            "command",
            json!({"command": ["git", 7]}),
            unknown("command"),
        ),
        (
            "command",
            json!({"command": "git status", "kind": "command"}),
            ApprovalFacts::Shell {
                command: "git status".into(),
                cwd: None,
            },
        ),
        (
            "command",
            json!({"command": "bash", "kind": "writeStdin"}),
            unknown("command"),
        ),
        (
            "command",
            json!({"command": "bash", "kind": null}),
            unknown("command"),
        ),
        (
            "command",
            json!({"command": "curl https://example.com", "networkApprovalContext": {"host": "example.com"}}),
            unknown("command"),
        ),
        (
            "command",
            json!({"command": "git status", "networkApprovalContext": null}),
            ApprovalFacts::Shell {
                command: "git status".into(),
                cwd: None,
            },
        ),
        (
            "command",
            json!({"command": ["git", "-c", "a.b='", "push", "origin", "HEAD:refs/heads/x'"]}),
            ApprovalFacts::Shell {
                command: r"git -c 'a.b='\''' push origin 'HEAD:refs/heads/x'\'''".into(),
                cwd: None,
            },
        ),
        (
            "command",
            json!({"command": ["echo", "", "a b", "~/x", "$HOME", "ok_1@%+=:,./-"]}),
            ApprovalFacts::Shell {
                command: "echo '' 'a b' '~/x' '$HOME' ok_1@%+=:,./-".into(),
                cwd: None,
            },
        ),
        (
            "fileChange",
            json!({"itemId": "patch-1", "interfacePaths": ["/repo/a.rs", "/elsewhere/b.rs"]}),
            ApprovalFacts::Write {
                paths: vec!["/repo/a.rs".into(), "/elsewhere/b.rs".into()],
            },
        ),
        (
            "fileChange",
            json!({"itemId": "patch-1"}),
            unknown("fileChange"),
        ),
        (
            "fileChange",
            json!({"interfacePaths": ["/repo/a.rs"], "grantRoot": "/"}),
            unknown("fileChange"),
        ),
        (
            "fileChange",
            json!({"interfacePaths": ["/repo/a.rs"], "grantRoot": null}),
            ApprovalFacts::Write {
                paths: vec!["/repo/a.rs".into()],
            },
        ),
        (
            "fileChange",
            json!({"interfacePaths": []}),
            unknown("fileChange"),
        ),
        (
            "fileChange",
            json!({"interfacePaths": ["/repo/a.rs", 3]}),
            unknown("fileChange"),
        ),
        (
            "permissions",
            json!({"permissions": {}}),
            unknown("permissions"),
        ),
        ("question", json!({"questions": []}), unknown("question")),
    ];
    for (tool, input, expected) in rows {
        assert_eq!(
            instance.approval_facts(tool, &input),
            expected,
            "tool {tool} input {input}"
        );
    }
}

/// Codex 0.154.0 reports every command inside its own login-shell wrapper (live probe P2).
const LIVE_RG_WRAPPER: &str = r#"/bin/zsh -lc "rg --files -g '*TODO*' -g '*todo*' -g '"'!.*'"' -g '"'!*.env'"' -g '"'!**/*credential*'"' -g '"'!**/*service-account*'"' -g '"'!**/*key*'"'""#;

fn command_facts(
    instance: &CodexAdapter,
    command: &Value,
) -> interface_contracts::agents::ApprovalFacts {
    instance.approval_facts("command", &json!({"command": command, "cwd": "/repo"}))
}

fn shell_facts(command: &str) -> interface_contracts::agents::ApprovalFacts {
    interface_contracts::agents::ApprovalFacts::Shell {
        command: command.into(),
        cwd: Some("/repo".into()),
    }
}

#[test]
fn codex_command_approvals_unwrap_the_live_harness_shell_wrappers() {
    let instance = adapter(PathBuf::from("/usr/bin/false"));
    let rows = [
        ("/bin/zsh -lc 'cat MEMORY.md'", "cat MEMORY.md"),
        (
            "/bin/zsh -lc 'git push origin main'",
            "git push origin main",
        ),
        (
            LIVE_RG_WRAPPER,
            "rg --files -g '*TODO*' -g '*todo*' -g '!.*' -g '!*.env' -g '!**/*credential*' \
             -g '!**/*service-account*' -g '!**/*key*'",
        ),
    ];
    for (wrapped, script) in rows {
        assert_eq!(
            command_facts(&instance, &json!(wrapped)),
            shell_facts(script),
            "{wrapped}"
        );
    }
}

#[test]
fn codex_command_approvals_unwrap_exactly_one_posix_quoted_wrapper() {
    let instance = adapter(PathBuf::from("/usr/bin/false"));
    let rows = [
        ("bash -c ls", "ls"),
        ("sh -lc 'ls -la'", "ls -la"),
        ("/bin/sh -c 'ls -la'", "ls -la"),
        ("/usr/bin/bash -lc 'ls'", "ls"),
        ("/opt/homebrew/bin/bash -lc 'ls'", "ls"),
        ("  /bin/zsh\t-lc   'ls'  ", "ls"),
        ("b\"ash\" '-lc' ls", "ls"),
        (r"/bin/zsh -lc 'echo '\''hi'\'''", "echo 'hi'"),
        (
            r#"/bin/zsh -lc "a \"b\" \\ \$HOME \`id\` \n é""#,
            r#"a "b" \ $HOME `id` \n é"#,
        ),
        (
            r#"/bin/zsh -lc 'git commit -m "x"'"#,
            r#"git commit -m "x""#,
        ),
        ("/bin/zsh -lc 'a'\"b\"c", "abc"),
        ("/bin/zsh -lc 'line one\nline two'", "line one\nline two"),
        // Exactly one wrapper comes off; the inner one stays for the policy.
        (
            "/bin/zsh -lc \"bash -lc 'git push'\"",
            "bash -lc 'git push'",
        ),
    ];
    for (wrapped, script) in rows {
        assert_eq!(
            command_facts(&instance, &json!(wrapped)),
            shell_facts(script),
            "{wrapped}"
        );
    }
    assert_eq!(
        command_facts(
            &instance,
            &json!(["/bin/zsh", "-lc", "git push origin main"])
        ),
        shell_facts("git push origin main")
    );
    assert_eq!(
        command_facts(&instance, &json!(["bash", "-c", "echo 'a b' | wc -c"])),
        shell_facts("echo 'a b' | wc -c")
    );
    // An empty script is no command at all.
    for empty in [json!("/bin/zsh -lc ''"), json!(["/bin/zsh", "-lc", " "])] {
        assert_eq!(
            command_facts(&instance, &empty),
            interface_contracts::agents::ApprovalFacts::Unknown {
                tool: "command".into()
            }
        );
    }
}

#[test]
fn codex_command_approvals_keep_anything_but_one_exact_wrapper() {
    let instance = adapter(PathBuf::from("/usr/bin/false"));
    let unchanged = [
        "/bin/zsh -lc 'a' extra",
        "/usr/bin/env bash -lc x",
        "fish -c x",
        "bash -x -c x",
        "bash -l -c x",
        "bash -cl x",
        "bash --login -c x",
        "bash -lc",
        "bash -lc 'unbalanced",
        "bash -lc \"unbalanced",
        "bash -lc 'a'\\",
        "/bin/zsh -lc 'a\0b'",
        "./zsh -lc ls",
        "bin/zsh -lc ls",
        "/work/project/zsh -lc ls",
        "/bin/../tmp/zsh -lc ls",
        "/bin/zshx -lc ls",
        "ZSH -lc ls",
        "bash -lc ls;rm",
        "bash -lc ls\nrm -rf /",
        "bash -lc $CMD",
        "bash -lc ~/x",
        "bash -lc *",
        "bash -lc é",
        "bash -lc \\ls",
        "bash -lc \"$(id)\"",
        "bash -lc \"`id`\"",
        "bash -lc 'ls' # comment",
        "git push origin main",
    ];
    for command in unchanged {
        assert_eq!(
            command_facts(&instance, &json!(command)),
            shell_facts(command),
            "{command:?}"
        );
    }
    let arrays = [
        (json!(["/bin/zsh", "-lc", "a", "b"]), "/bin/zsh -lc a b"),
        (json!(["fish", "-c", "x"]), "fish -c x"),
        (
            json!(["/usr/bin/env", "bash", "-lc"]),
            "/usr/bin/env bash -lc",
        ),
        (json!(["bash", "-l", "x"]), "bash -l x"),
        (json!(["/tmp/bash", "-c", "x"]), "/tmp/bash -c x"),
        (json!(["/bin/zsh", "-lc", "a\0b"]), "/bin/zsh -lc 'a\0b'"),
    ];
    for (argv, joined) in arrays {
        assert_eq!(
            command_facts(&instance, &argv),
            shell_facts(joined),
            "{argv}"
        );
    }
}

#[test]
fn codex_harness_unwrap_reverses_the_argv_quoting() {
    let instance = adapter(PathBuf::from("/usr/bin/false"));
    let scripts = [
        "ls",
        "git push origin main",
        "echo 'a b' \"c\" \\ $HOME `id` it's",
        "printf '%s\\n' \"$(cat MEMORY.md)\"",
        "a\nb\tc",
        "héllo ünïcode",
        "'",
        "''\\''",
    ];
    for script in scripts {
        let wrapped = format!("/bin/zsh -lc {}", shell_word(script));
        assert_eq!(
            command_facts(&instance, &json!(wrapped)),
            shell_facts(script),
            "{wrapped}"
        );
    }
}

#[tokio::test]
async fn file_change_approval_carries_the_paths_of_its_started_item() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        continue
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread": {"id": "thread-one"}}
    elif method == "turn/start":
        print(json.dumps({"id": ident, "result": {"turn": {"id": "turn-one"}}}), flush=True)
        print(json.dumps({"method": "turn/started", "params": {"threadId": "thread-one", "turn": {"id": "turn-one"}}}), flush=True)
        print(json.dumps({"method": "item/started", "params": {"threadId": "thread-one", "turnId": "turn-one", "item": {
            "type": "fileChange", "id": "patch-1", "status": "inProgress", "changes": [
                {"path": "/tmp/a.txt", "kind": {"type": "add"}, "diff": "+a"},
                {"path": "/tmp/b.txt", "kind": {"type": "update", "move_path": "/elsewhere/b.txt"}, "diff": ""}]}}}), flush=True)
        print(json.dumps({"method": "item/fileChange/patchUpdated", "params": {"threadId": "thread-one", "turnId": "turn-one",
            "itemId": "patch-1", "changes": [{"path": "/tmp/c.txt", "kind": {"type": "delete"}, "diff": ""}]}}), flush=True)
        print(json.dumps({"id": "approval-one", "method": "item/fileChange/requestApproval", "params": {
            "threadId": "thread-one", "turnId": "turn-one", "itemId": "patch-1", "startedAtMs": 1,
            "interfacePaths": ["/forged"]}}), flush=True)
        print(json.dumps({"id": "approval-two", "method": "item/fileChange/requestApproval", "params": {
            "threadId": "thread-one", "turnId": "turn-one", "itemId": "patch-unseen", "startedAtMs": 2,
            "interfacePaths": ["/forged"]}}), flush=True)
        continue
    else: result = {}
    print(json.dumps({"id": ident, "result": result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "edit".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-one");
    let mut approvals = Vec::new();
    while approvals.len() < 2 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval deadline")
            .unwrap()
            .unwrap();
        if let EngineEventKind::ApprovalRequested { tool, input, .. } = event.event {
            assert_eq!(tool, "fileChange");
            approvals.push(input);
        }
    }
    assert_eq!(
        approvals[0]["interfacePaths"],
        json!(["/tmp/a.txt", "/tmp/b.txt", "/elsewhere/b.txt", "/tmp/c.txt"])
    );
    assert_eq!(
        instance.approval_facts("fileChange", &approvals[0]),
        interface_contracts::agents::ApprovalFacts::Write {
            paths: vec![
                "/tmp/a.txt".into(),
                "/tmp/b.txt".into(),
                "/elsewhere/b.txt".into(),
                "/tmp/c.txt".into()
            ],
        }
    );
    assert!(approvals[1].get("interfacePaths").is_none());
    assert_eq!(
        instance.approval_facts("fileChange", &approvals[1]),
        interface_contracts::agents::ApprovalFacts::Unknown {
            tool: "fileChange".into()
        }
    );
    handle.close().await.unwrap();
}

#[test]
fn file_change_path_stash_is_bounded() {
    let mut stash = VecDeque::new();
    for index in 0..300 {
        remember_file_change_paths(
            &mut stash,
            &format!("patch-{index}"),
            Some(vec![format!("/tmp/{index}")]),
        );
    }
    assert_eq!(stash.len(), MAX_FILE_CHANGE_ITEMS);
    assert!(stash.iter().all(|(id, _)| id != "patch-0"));
    remember_file_change_paths(
        &mut stash,
        "patch-299",
        Some(vec!["/tmp/299".into(), "/tmp/more".into()]),
    );
    assert_eq!(stash.len(), MAX_FILE_CHANGE_ITEMS);
    let paths_of = |stash: &VecDeque<(String, Option<Vec<String>>)>, item: &str| {
        stash
            .iter()
            .find(|(id, _)| id == item)
            .and_then(|(_, paths)| paths.clone())
    };
    assert_eq!(
        paths_of(&stash, "patch-299"),
        Some(vec!["/tmp/299".to_owned(), "/tmp/more".to_owned()])
    );
    // One unreadable change poisons the item: its approval stays Unknown.
    remember_file_change_paths(&mut stash, "patch-299", None);
    remember_file_change_paths(&mut stash, "patch-299", Some(vec!["/tmp/late".into()]));
    assert_eq!(paths_of(&stash, "patch-299"), None);
    assert_eq!(
        file_change_targets(Some(&json!([
            {"path": "/tmp/a", "kind": {"type": "update", "move_path": "/tmp/b"}},
            {"path": "/tmp/a", "kind": {"type": "update", "move_path": null}}
        ]))),
        Some(vec!["/tmp/a".to_owned(), "/tmp/b".to_owned()])
    );
    assert_eq!(
        file_change_targets(Some(&json!([{"kind": {"type": "add"}}]))),
        None
    );
    assert_eq!(
        file_change_targets(Some(
            &json!([{"path": "/tmp/a", "kind": {"type": "update", "move_path": 7}}])
        )),
        None
    );
    assert_eq!(file_change_targets(None), None);
}

#[test]
fn only_the_host_supplied_agent_endpoint_gets_native_tool_approval() {
    let descriptor = CollaborationMcpStdioDescriptor {
        name: "iface_agent".into(),
        command: "/usr/bin/nc".into(),
        args: vec!["-U".into(), "/private/tmp/fixture.sock".into()],
        env: vec![],
    };
    let config = agent_tool_config(&descriptor);
    assert_eq!(config["default_tools_approval_mode"], "approve");
    assert_eq!(config["required"], true);
    assert_eq!(config["enabled_tools"].as_array().unwrap().len(), 8);
    assert_eq!(config["tools"]["consult_apply"]["approval_mode"], "approve");
    let gate = GateRequest {
        mode: AgentMode::Gated,
        cwd: "/fixture".into(),
        directories: vec![],
    };
    assert_eq!(
        gate_overrides(&gate, &["external".into()])["mcp_servers"]["external"],
        json!({"enabled":false})
    );
}

/// The thinking signals a turn emits, read until the stream falls quiet.
async fn thinking_signals(events: &mut BoundedEventStream<EngineEvent>) -> Vec<bool> {
    let mut signals = Vec::new();
    while let Ok(event) = timeout(Duration::from_millis(400), events.recv()).await {
        let event = event
            .expect("Codex event stream failure")
            .expect("Codex event stream ended");
        if let EngineEventKind::ThinkingSignal { active, .. } = event.event {
            signals.push(active);
        }
    }
    signals
}

#[tokio::test]
async fn a_reasoning_span_emits_one_thinking_signal_and_each_turn_ends_it() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
def note(method, **params):
    params["threadId"] = "reasoning-thread"
    print(json.dumps({"method":method, "params":params}), flush=True)
turn = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method is None:
        continue
    if method == "thread/start":
        result = {"thread":{"id":"reasoning-thread"}}
    elif method == "turn/start":
        turn += 1
        result = {"turn":{"id":"turn-" + str(turn)}}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
    if method == "turn/start":
        note("turn/started", turn={"id":"turn-" + str(turn)})
        # More than the 15 frames the inbound queue once held, in one burst.
        for _ in range(40):
            note("item/reasoning/textDelta", itemId="r" + str(turn), delta="private")
        note("item/completed", item={"type":"reasoning", "id":"r" + str(turn)})
        note("turn/completed", turn={"id":"turn-" + str(turn), "status":"completed"})
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "think".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "reasoning-thread");
    // Forty reasoning deltas are one signal; the reasoning item and the turn both end it,
    // and the second end repeats the first.
    assert_eq!(thinking_signals(&mut events).await, [true, false]);
    assert!(handle.send("think again".into()).await.is_confirmed());
    assert_eq!(thinking_signals(&mut events).await, [true, false]);
    handle.close().await.unwrap();
}

#[tokio::test]
async fn a_prompt_no_frame_can_carry_is_refused_before_any_provider_effect() {
    let (directory, executable) = fake_codex(
        r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
open(os.path.join(here, "started"), "a").close()
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method is None:
        continue
    if method == "thread/start":
        result = {"thread":{"id":"frame-thread"}}
    elif method == "turn/start":
        open(os.path.join(here, "turn"), "a").close()
        result = {"turn":{"id":"turn-1"}}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"), "result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    // Under the 8 MiB prompt bound but over the 4 MiB frame bound once escaped.
    let unframable = "x".repeat(crate::transport::DEFAULT_MAX_FRAME_BYTES);
    let error = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: unframable.clone(),
            ..LaunchOptions::default()
        })
        .err()
        .expect("an unframable launch prompt is refused");
    assert_eq!(error.message, "Codex launch prompt must be bounded");
    assert!(!error.retryable);
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::DefinitelyUnsent
    );
    assert!(
        !directory.path().join("started").exists(),
        "no thread was created"
    );

    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: String::new(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "frame-thread");
    match handle.send(unframable).await {
        DeliveryOutcome::DefinitelyUnsent(error) => {
            assert_eq!(error.message, "Codex prompt must be non-empty and bounded");
            assert!(!error.retryable);
        }
        other => panic!("an unframable prompt must be refused unsent: {other:?}"),
    }
    assert!(!directory.path().join("turn").exists());
    handle.close().await.unwrap();
}

#[test]
fn listing_titles_an_unnamed_thread_by_its_preview() {
    let engine: EngineId = "codex".parse().unwrap();
    let page = serde_json::json!({"data": [
        {"id": "unnamed", "name": null, "preview": "Fix the login bug", "cwd": "/repo"},
        {"id": "named", "name": "Release", "preview": "ignored", "cwd": "/repo"},
        {"id": "blank", "name": "", "preview": "", "cwd": "/repo"},
    ]});
    let titles: Vec<(String, Option<bool>)> = parse_thread_page(&engine, &page, false)
        .into_iter()
        .map(|summary| (summary.title, summary.title_from_summary))
        .collect();
    // Only a name is Codex's own title; a preview is the first prompt.
    assert_eq!(
        titles,
        [
            ("Fix the login bug".to_owned(), None),
            ("Release".to_owned(), Some(true)),
            ("Codex session".to_owned(), None)
        ]
    );
}

#[tokio::test]
async fn a_refused_first_turn_keeps_the_owed_collaboration_source() {
    let directory = tempfile::tempdir().unwrap();
    let turns = directory.path().join("turns.jsonl");
    let turns_literal = serde_json::to_string(&turns.to_string_lossy()).unwrap();
    let (_executable_directory, executable) = fake_codex(&format!(
        r#"
import json, sys
starts = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    params = msg.get("params", {{}})
    if method == "turn/start":
        starts += 1
        with open({turns_literal}, "a") as handle:
            handle.write(json.dumps(params.get("input")) + "\n")
        if starts == 1:
            print(json.dumps({{"id":msg.get("id"),"error":{{"code":-32600,"message":"model not available"}}}}), flush=True)
            continue
        result = {{"turn":{{"id":"turn-2"}}}}
    elif method == "initialize": result = {{}}
    elif method == "thread/start": result = {{"thread":{{"id":"fresh-thread"}}}}
    else: result = {{}}
    print(json.dumps({{"id":msg.get("id"),"result":result}}), flush=True)
    if method == "turn/start":
        print(json.dumps({{"method":"turn/completed","params":{{"threadId":"fresh-thread","turn":{{"id":"turn-2","status":"completed"}}}}}}), flush=True)
"#
    ));
    let state = Arc::new(CollaborationTestState::default());
    let instance = adapter(executable)
        .with_collaboration_bindings(&collaboration_bindings(&state))
        .unwrap();
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: String::new(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "fresh-thread");
    let refused = handle.send("hello".into()).await;
    assert!(!refused.is_confirmed(), "{refused:?}");
    let outcome = handle.send("try again".into()).await;
    assert!(outcome.is_confirmed(), "{outcome:?}");

    let written = std::fs::read_to_string(&turns).unwrap();
    let sent: Vec<Vec<Value>> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sent.len(), 2);
    for input in &sent {
        let host_context = input[0]["text"].as_str().unwrap();
        assert!(host_context.contains("codex#fresh-thread"), "{input:?}");
    }
    assert_eq!(sent[1][1], json!({"type":"text","text":"try again"}));
    handle.close().await.unwrap();
}

#[test]
fn codex_sign_in_state_comes_from_its_own_account_read() {
    assert_eq!(
        codex_auth(
            &json!({"account": {"type": "chatgpt", "email": "a@b.test", "planType": "plus"},
            "requiresOpenaiAuth": true})
        ),
        Some(EngineAuth::SignedIn)
    );
    assert_eq!(
        codex_auth(&json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": true})),
        Some(EngineAuth::SignedIn)
    );
    assert_eq!(
        codex_auth(&json!({"account": null, "requiresOpenaiAuth": true})),
        Some(EngineAuth::SignedOut)
    );
    // A provider that needs no OpenAI sign-in (a local model) says nothing.
    assert_eq!(
        codex_auth(&json!({"account": null, "requiresOpenaiAuth": false})),
        None
    );
    assert_eq!(codex_auth(&json!({})), None);
}

#[test]
fn a_refresh_repoints_the_codex_executable_for_later_launches() {
    let config = CodexAdapterConfig::new("/opt/old/codex");
    let shared = config.executable_path.clone();
    let adapter = CodexAdapter::new(config).unwrap();
    assert_eq!(adapter.executable(), Some(PathBuf::from("/opt/old/codex")));
    assert!(shared.set(PathBuf::from("/opt/homebrew/bin/codex")));
    assert!(!shared.set(PathBuf::from("/opt/homebrew/bin/codex")));
    assert_eq!(
        adapter.executable(),
        Some(PathBuf::from("/opt/homebrew/bin/codex"))
    );
}

#[tokio::test]
async fn concurrent_approvals_stay_waiting_until_last_approval_is_confirmed() {
    let (_directory, executable) = fake_codex(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        if ident == "approval-one":
            print(json.dumps({"method":"item/agentMessage/delta","params":{"threadId":"thread-one","turnId":"turn-one","itemId":"msg-1","delta":"first approved"}}), flush=True)
        elif ident == "approval-two":
            print(json.dumps({"method":"item/agentMessage/delta","params":{"threadId":"thread-one","turnId":"turn-one","itemId":"msg-1","delta":" and second"}}), flush=True)
        continue
    if method == "initialize": result = {}
    elif method == "thread/start": result = {"thread":{"id":"thread-one"}}
    elif method == "turn/start":
        result = {"turn":{"id":"turn-one"}}
        print(json.dumps({"id":ident,"result":result}), flush=True)
        print(json.dumps({"method":"turn/started","params":{"threadId":"thread-one","turn":{"id":"turn-one"}}}), flush=True)
        print(json.dumps({"id":"approval-one","method":"execCommandApproval","params":{"threadId":"thread-one","command":"git status"}}), flush=True)
        print(json.dumps({"id":"approval-two","method":"execCommandApproval","params":{"threadId":"thread-one","command":"git diff"}}), flush=True)
        continue
    elif method == "skills/list": result = {"data":[{"cwd":"/tmp","errors":[],"skills":[]}]}
    else: result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "initial".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "thread-one");

    let mut status = SessionStatus::Idle;
    let mut approvals = Vec::new();
    while approvals.len() < 2 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        status = interface_contracts::engine::next_status(status, &event);
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = event.event {
            approvals.push(approval_id);
        }
    }
    assert_eq!(status, SessionStatus::Waiting);

    assert!(
        handle
            .respond_to_approval(
                approvals[0].clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    let mut saw_first_delta = false;
    while let Ok(event) = timeout(Duration::from_millis(200), events.recv()).await {
        let event = event.unwrap().unwrap();
        status = interface_contracts::engine::next_status(status, &event);
        saw_first_delta |= matches!(event.event, EngineEventKind::MessageDelta { .. });
    }
    assert!(saw_first_delta);
    assert_eq!(
        status,
        SessionStatus::Waiting,
        "answering the first of two concurrent Codex approvals must keep Waiting"
    );

    assert!(
        handle
            .respond_to_approval(
                approvals[1].clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    while let Ok(event) = timeout(Duration::from_millis(200), events.recv()).await {
        let event = event.unwrap().unwrap();
        status = interface_contracts::engine::next_status(status, &event);
    }
    assert_eq!(status, SessionStatus::Working);
    handle.close().await.unwrap();
}

/// Team tools (protocol 57): an ordinary code thread gets the `iface_team`
/// server through its thread config, with a tool timeout past the longest
/// wait, confirmed for the id Codex gave it; a resume re-attaches it,
/// confirmed for the id it asked for. A thread one hop down (collaboration
/// depth 1, as a started thread runs) and an agent never ask (Codex runs no chat).
#[tokio::test]
#[allow(clippy::too_many_lines)] // A start, a resume, then every shape that never asks.
async fn codex_threads_get_team_tools_through_thread_config_exactly_when_they_should() {
    use interface_engine::test_support::{CollaborationTestState, TeamToolsTestFactory};
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("opens.jsonl");
    let log_literal = serde_json::to_string(&log.to_string_lossy()).unwrap();
    let (_executable_directory, executable) = fake_codex(&format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": result = {{}}
    elif method == "config/read": result = {{"config": {{"mcp_servers": {{}}}}}}
    elif method in ("thread/start", "thread/resume"):
        with open({log_literal}, "a") as out:
            out.write(json.dumps({{"method": method, "params": msg.get("params")}}) + "\n")
        thread = msg["params"].get("threadId") or "fresh-thread"
        result = {{"thread": {{"id": thread}}}}
    else: result = {{}}
    if msg.get("id") is not None:
        print(json.dumps({{"id": msg.get("id"), "result": result}}), flush=True)
"#
    ));
    let state = Arc::new(CollaborationTestState::default());
    let mut config = CodexAdapterConfig::new(executable);
    config.transport.request_timeout = Duration::from_secs(30);
    config.team_tools = Some(TeamToolsTestFactory::shared(
        &state,
        "/tmp/iface-tt-test/s.sock",
        true,
    ));
    let instance = CodexAdapter::new(config).unwrap();
    let opens = || -> Vec<Value> {
        fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };

    let created = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    timeout(Duration::from_secs(30), created.ready())
        .await
        .unwrap()
        .unwrap();
    let server = &opens()[0]["params"]["config"]["mcp_servers"]["iface_team"];
    assert_eq!(server["command"], "/usr/bin/nc");
    assert_eq!(server["args"], json!(["-U", "/tmp/iface-tt-test/s.sock"]));
    assert_eq!(server["tool_timeout_sec"], 660);
    assert_eq!(server["required"], false);
    assert_eq!(
        server["enabled_tools"],
        json!(interface_contracts::engine::TEAM_TOOL_NAMES)
    );
    assert_eq!(
        lock(&state.confirmed).as_slice(),
        ["codex#fresh-thread".to_owned()]
    );
    // The app-server's own process is named to the binding (the host
    // answers only it and the processes it starts).
    let providers = lock(&state.providers).clone();
    assert_eq!(providers.len(), 1, "{providers:?}");
    assert!(providers[0] != 0 && providers[0] != std::process::id());
    created.close().await.unwrap();
    assert_eq!(
        state.disposed.load(Ordering::Acquire),
        1,
        "revoked at close"
    );

    let resumed = instance
        .resume_session("old-thread".into(), "/tmp".into(), ResumeOptions::default())
        .unwrap();
    timeout(Duration::from_secs(30), resumed.ready())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opens()[1]["method"], "thread/resume");
    assert!(opens()[1]["params"]["config"]["mcp_servers"]["iface_team"].is_object());
    assert_eq!(
        lock(&state.confirmed).last().map(String::as_str),
        Some("codex#old-thread")
    );
    assert!(lock(&state.requests)[1].resumed);
    assert_eq!(
        lock(&state.providers).len(),
        2,
        "the resumed app-server too"
    );
    resumed.close().await.unwrap();

    let asked = lock(&state.requests).len();
    for options in [
        LaunchOptions {
            cwd: "/tmp".into(),
            collaboration_depth: Some(1),
            ..LaunchOptions::default()
        },
        LaunchOptions {
            cwd: "/tmp".into(),
            kind: Some(SessionKind::Agent),
            agent_access: Some(interface_contracts::engine::AgentAccess::Bypass),
            ..LaunchOptions::default()
        },
    ] {
        let session = instance.create_session(options).unwrap();
        let _ = timeout(Duration::from_secs(30), session.ready())
            .await
            .unwrap();
        let _ = session.close().await;
    }
    assert_eq!(lock(&state.requests).len(), asked, "never asked");
    assert!(
        opens()[2..]
            .iter()
            .all(|open| open["params"]["config"]["mcp_servers"]["iface_team"].is_null()),
        "{:?}",
        opens()
    );
}

/// Several accounts per provider (protocol 60): an added account's adapter
/// starts every app-server, session and control lane alike, with that
/// account's `CODEX_HOME`, on connections of its own; the Default account's
/// adapter sets nothing new, so its children see what the host inherited.
#[tokio::test]
async fn each_account_runs_its_own_app_servers_with_its_own_codex_home() {
    let record = tempfile::tempdir().unwrap();
    let log = record.path().join("homes.txt");
    let (_directory, executable) = fake_codex(&format!(
        r#"
import json, os, sys
with open({log:?}, "a") as out:
    out.write("%d %s\n" % (os.getpid(), os.environ.get("CODEX_HOME", "<unset>")))
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if ident is None: continue
    if method == "thread/start": result = {{"thread": {{"id": "thread-%d" % os.getpid()}}}}
    elif method == "thread/list": result = {{"data": []}}
    else: result = {{}}
    print(json.dumps({{"id": ident, "result": result}}), flush=True)
"#,
        log = log.to_str().unwrap()
    ));
    let account = record.path().join("accounts/codex/acct-work");
    let default = adapter(executable.clone());
    let mut config = CodexAdapterConfig::new(executable);
    config.transport.request_timeout = Duration::from_secs(30);
    config.environment = vec![(
        std::ffi::OsString::from("CODEX_HOME"),
        account.clone().into_os_string(),
    )];
    let work = CodexAdapter::new(config).unwrap();
    let launch = || LaunchOptions {
        cwd: "/tmp".into(),
        prompt: String::new(),
        ..LaunchOptions::default()
    };
    for instance in [&default, &work] {
        let handle = instance.create_session(launch()).unwrap();
        timeout(Duration::from_secs(30), handle.ready())
            .await
            .expect("ready in time")
            .unwrap();
        timeout(Duration::from_secs(30), instance.list_sessions())
            .await
            .expect("listed in time")
            .unwrap();
        handle.close().await.unwrap();
    }
    let inherited = std::env::var("CODEX_HOME").unwrap_or_else(|_| "<unset>".to_owned());
    let lines = fs::read_to_string(&log).unwrap();
    let spawns = lines
        .lines()
        .map(|line| line.split_once(' ').unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        spawns.len(),
        4,
        "a session and a listing lane per account: {lines}"
    );
    let homes = spawns.iter().map(|(_, home)| *home).collect::<Vec<_>>();
    assert_eq!(
        homes,
        [
            inherited.as_str(),
            inherited.as_str(),
            account.to_str().unwrap(),
            account.to_str().unwrap()
        ]
    );
    let pids = spawns.iter().map(|(pid, _)| *pid).collect::<HashSet<_>>();
    assert_eq!(pids.len(), 4, "no connection is shared across accounts");
    default.dispose().await.unwrap();
    work.dispose().await.unwrap();
}
