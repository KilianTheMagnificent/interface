use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Stdio,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

use interface_engine::test_support::{
    CollaborationTestFactory, CollaborationTestState, TeamToolsTestFactory,
};
use tempfile::TempDir;
use tokio::time::timeout;

use super::*;
use crate::profile::{AcpProfile, validate_profile};

fn fake_agent(script_body: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fake-acp");
    fs::write(&path, format!("#!/usr/bin/env python3\n{script_body}\n")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    (directory, path)
}

fn custom_adapter(path: PathBuf) -> AcpAdapter {
    let profile = validate_profile(AcpProfile {
        slug: "fake".into(),
        display_name: "Fake ACP".into(),
        executable_path: path,
        args: Vec::new(),
    })
    .unwrap();
    let mut config = AcpAdapterConfig::custom(profile);
    config.transport.request_timeout = Duration::from_secs(2);
    config.prompt_timeout = Duration::from_secs(2);
    AcpAdapter::new(config).unwrap()
}

fn grok_adapter(path: PathBuf) -> AcpAdapter {
    let mut config =
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, path).expect("Grok config");
    config.transport.request_timeout = Duration::from_secs(2);
    config.prompt_timeout = Duration::from_secs(2);
    AcpAdapter::new(config).expect("Grok adapter")
}

const PORTABLE_EFFORT_AGENT: &str = r#"
import json, sys
session = "portable-session"
expected_effort = None
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"portable-1"},"agentCapabilities":{"loadSession":True},"currentModelId":"portable-model","models":{"availableModels":[{"modelId":"portable-model","name":"Portable","_meta":{"reasoningEfforts":["low","high"]}}]}}
    elif method == "session/new":
        expected_effort = "low"
        result = {"sessionId":session,"currentModelId":"portable-model"}
    elif method == "session/load":
        session = msg.get("params", {}).get("sessionId", "stored-portable")
        expected_effort = "high"
        result = {"sessionId":session,"currentModelId":"portable-model"}
    elif method == "session/prompt":
        effort = msg.get("params", {}).get("_meta", {}).get("reasoningEffort")
        if effort != expected_effort:
            print(json.dumps({"id":msg.get("id"),"error":{"code":-1,"message":"unadvertised effort crossed boundary"}}), flush=True)
            continue
        result = {"stopReason":"end_turn"}
    else:
        result = {}
    if msg.get("id") is not None:
        print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#;

/// The socket the shared collaboration fake's bindings name.
const COLLABORATION_SOCKET: &str = "/tmp/grok-collaboration-test.sock";

struct QwenLifecycleTestRuntime {
    settings_path: PathBuf,
    probe_prepares: AtomicUsize,
    launch_prepares: AtomicUsize,
    disposed: AtomicUsize,
}

#[async_trait]
impl QwenBackendLifecycle for QwenLifecycleTestRuntime {
    fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    async fn prepare_probe(&self) -> Result<(), QwenBackendError> {
        self.probe_prepares.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    async fn prepare(&self) -> Result<(), QwenBackendError> {
        self.launch_prepares.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    async fn dispose(&self) -> Result<(), QwenBackendError> {
        self.disposed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[tokio::test]
async fn failed_handshake_reaps_the_agent_child() {
    let state = tempfile::tempdir().unwrap();
    let pid_path = state.path().join("pid");
    let (_directory, executable) = fake_agent(
        // Like a real agent, it does not exit when its stdin closes: only
        // an explicit shutdown reaps it.
        &r#"
import os, time
with open("PID_PATH", "w") as handle:
    handle.write(str(os.getpid()))
time.sleep(30)
"#
        .replace("PID_PATH", &pid_path.to_string_lossy()),
    );
    let instance = custom_adapter(executable);
    assert!(instance.probe().await.is_err());
    let pid = fs::read_to_string(&pid_path).expect("fake agent never recorded its pid");
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
    assert!(reaped, "a failed ACP handshake left the agent running");
}

#[tokio::test]
async fn dropped_usage_read_after_the_handshake_reaps_the_agent_child() {
    let state = tempfile::tempdir().unwrap();
    let pid_path = state.path().join("pid");
    let (_directory, executable) = fake_agent(
        // Answers the handshake, never answers the billing read, and (like a
        // real agent) does not exit when its stdin closes.
        &r#"
import json, os, sys, time
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        print(json.dumps({"id":msg.get("id"),"result":{"protocolVersion":1,"agentCapabilities":{}}}), flush=True)
    elif msg.get("method") == "_x.ai/billing":
        with open("PID_PATH.tmp", "w") as handle:
            handle.write(str(os.getpid()))
        os.rename("PID_PATH.tmp", "PID_PATH")
time.sleep(30)
"#
        .replace("PID_PATH", &pid_path.to_string_lossy()),
    );
    let instance = grok_adapter(executable);
    let mut usage = Box::pin(instance.read_usage());
    let waited = tokio::time::Instant::now();
    while !pid_path.exists() {
        tokio::select! {
            _ = &mut usage => panic!("the billing read was never meant to answer"),
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        assert!(
            waited.elapsed() < Duration::from_secs(10),
            "fake agent never saw the billing read"
        );
    }
    // The runtime's listing timeout drops the future exactly here.
    drop(usage);
    let pid = fs::read_to_string(&pid_path).unwrap();
    let mut reaped = false;
    // Well inside the 2 s request timeout, which would otherwise end the read.
    for _ in 0..60 {
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
    assert!(reaped, "a dropped ACP usage read left the agent running");
}

#[tokio::test]
async fn app_owned_commands_are_fenced_before_launch_and_live_dispatch() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
session = "slash-session"
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentCapabilities":{}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt":
        text = msg.get("params", {}).get("prompt", [{}])[0].get("text", "")
        if not text.startswith("/future-command"):
            print(json.dumps({"id":msg.get("id"),"error":{"code":-1,"message":"unexpected prompt"}}), flush=True)
            continue
        result = {"stopReason":"end_turn"}
    else:
        result = {}
    if msg.get("id") is not None:
        print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);

    let launch_error = match instance.create_session(LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "/effort high".into(),
        ..LaunchOptions::default()
    }) {
        Ok(_) => panic!("app-owned launch command crossed the ACP boundary"),
        Err(error) => error,
    };
    assert!(launch_error.message.contains("Interface owns /effort"));
    assert!(
        launch_error
            .message
            .contains("nothing was sent to the provider")
    );

    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "/future-command launch".into(),
            ..LaunchOptions::default()
        })
        .expect("unknown launch slash text stays ordinary provider input");
    let mut events = handle.take_events().expect("event stream");
    handle
        .ready()
        .await
        .expect("unknown launch slash delivered");
    loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("launch prompt event timeout")
            .expect("launch prompt stream")
            .expect("launch prompt event");
        if matches!(
            event.event,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            }
        ) {
            break;
        }
    }

    let live_error = handle.send("/permissions bypass".into()).await;
    assert!(matches!(
        live_error,
        DeliveryOutcome::DefinitelyUnsent(error)
            if error.message.contains("Interface owns /permissions")
                && error.message.contains("nothing was sent to the provider")
    ));
    assert!(
        handle
            .send("/future-command live".into())
            .await
            .is_confirmed()
    );
    handle.close().await.expect("close slash session");
}

#[tokio::test]
async fn built_in_grok_injects_one_binding_and_confirms_the_final_provider_id() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentCapabilities":{"loadSession":True}}
    elif method == "session/new":
        assert msg.get("params", {}).get("mcpServers") == [{
            "name":"iface_collaboration", "command":"/usr/bin/nc",
            "args":["-U","/tmp/grok-collaboration-test.sock"], "env":[]}]
        result = {"sessionId":"grok-final"}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let state = Arc::new(CollaborationTestState::default());
    let factory = CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let instance = grok_adapter(executable)
        .with_collaboration_factory(factory)
        .unwrap();
    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "grok-final");
    assert_eq!(lock(&state.confirmed).as_slice(), ["acp:grok#grok-final"]);
    assert_eq!(lock(&state.requests).len(), 1);
    instance.dispose().await.unwrap();
    assert_eq!(state.disposed.load(Ordering::Acquire), 1);
    handle.close().await.unwrap();
}

#[tokio::test]
async fn built_in_qwen_prepares_each_lifecycle_and_injects_only_its_local_environment() {
    let (_directory, executable) = fake_agent(
        r#"
import json, os, sys
assert os.environ.get("OPENAI_API_KEY") == "local"
assert os.environ.get("OPENAI_BASE_URL") == "http://127.0.0.1:8080/v1"
assert os.environ.get("OPENAI_MODEL") == "mlx-community/Qwen3.8-27B-4bit"
assert os.environ.get("QWEN_CODE_SYSTEM_SETTINGS_PATH") == "/tmp/qwen-lifecycle-test.json"
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"qwen-test"},"agentCapabilities":{}}
    elif method == "session/new":
        assert msg.get("params", {}).get("mcpServers") == []
        result = {"sessionId":"qwen-final"}
    elif method == "session/prompt":
        result = {"stopReason":"end_turn"}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let runtime = Arc::new(QwenLifecycleTestRuntime {
        settings_path: PathBuf::from("/tmp/qwen-lifecycle-test.json"),
        probe_prepares: AtomicUsize::new(0),
        launch_prepares: AtomicUsize::new(0),
        disposed: AtomicUsize::new(0),
    });
    let mut config =
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Qwen, executable).expect("Qwen config");
    config.transport.request_timeout = Duration::from_secs(2);
    let adapter = AcpAdapter::new(config)
        .expect("Qwen adapter")
        .with_qwen_backend_runtime(runtime.clone())
        .expect("Qwen runtime");

    let info = adapter.probe().await.expect("Qwen probe");
    assert_eq!(info.version, "qwen-test");
    assert!(info.capabilities.effort_levels.is_empty());
    assert_eq!(runtime.probe_prepares.load(Ordering::Acquire), 1);
    assert_eq!(runtime.launch_prepares.load(Ordering::Acquire), 0);

    let launch_error = match adapter.create_session(LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        model: Some(QWEN_MODEL_ID.into()),
        effort: Some("high".into()),
        ..LaunchOptions::default()
    }) {
        Ok(_) => panic!("Qwen accepted an effort its fixed catalogue does not advertise"),
        Err(error) => error,
    };
    assert!(launch_error.message.contains("effort"));
    let resume_error = match adapter.resume_session(
        "stored-qwen".into(),
        "/tmp".into(),
        ResumeOptions {
            model: Some(QWEN_MODEL_ID.into()),
            effort: Some("high".into()),
            ..ResumeOptions::default()
        },
    ) {
        Ok(_) => panic!("Qwen resumed with an effort it does not advertise"),
        Err(error) => error,
    };
    assert!(resume_error.message.contains("effort"));
    assert_eq!(runtime.launch_prepares.load(Ordering::Acquire), 0);

    let handle = adapter
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            model: Some(QWEN_MODEL_ID.into()),
            ..LaunchOptions::default()
        })
        .expect("Qwen session");
    assert_eq!(
        handle.ready().await.expect("Qwen ready").session_id,
        "qwen-final"
    );
    assert_eq!(runtime.launch_prepares.load(Ordering::Acquire), 1);
    assert!(handle.set_model(None).await.is_confirmed());
    assert!(
        handle
            .set_model(Some(QWEN_MODEL_ID.into()))
            .await
            .is_confirmed()
    );
    assert!(matches!(
        handle.set_model(Some("another-model".into())).await,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    assert!(matches!(
        handle.set_effort(Some("high".into())).await,
        DeliveryOutcome::DefinitelyUnsent(error) if error.message.contains("does not offer")
    ));
    handle.close().await.expect("Qwen close");
    adapter.dispose().await.expect("Qwen dispose");
    adapter.dispose().await.expect("idempotent Qwen dispose");
    assert_eq!(runtime.disposed.load(Ordering::Acquire), 1);
}

#[test]
fn qwen_fixed_model_is_accepted_for_launch_and_resume_but_other_models_are_not() {
    let qwen_launch = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        model: Some(QWEN_MODEL_ID.into()),
        ..LaunchOptions::default()
    };
    assert!(validate_launch(AcpProviderFlavor::Qwen, &qwen_launch, &[]).is_ok());
    let mut qwen_effort_launch = qwen_launch.clone();
    qwen_effort_launch.effort = Some("high".into());
    assert!(validate_launch(AcpProviderFlavor::Qwen, &qwen_effort_launch, &[]).is_err());
    let mut wrong_launch = qwen_launch;
    wrong_launch.model = Some("another-model".into());
    assert!(validate_launch(AcpProviderFlavor::Qwen, &wrong_launch, &[]).is_err());
    assert!(validate_launch(AcpProviderFlavor::Custom, &wrong_launch, &[]).is_err());

    let qwen_resume = ResumeOptions {
        model: Some(QWEN_MODEL_ID.into()),
        ..ResumeOptions::default()
    };
    assert!(
        validate_resume(
            AcpProviderFlavor::Qwen,
            "provider-session",
            "/tmp",
            &qwen_resume,
            &[],
        )
        .is_ok()
    );
    let mut qwen_effort_resume = qwen_resume.clone();
    qwen_effort_resume.effort = Some("high".into());
    assert!(
        validate_resume(
            AcpProviderFlavor::Qwen,
            "provider-session",
            "/tmp",
            &qwen_effort_resume,
            &[],
        )
        .is_err()
    );
    let mut wrong_resume = qwen_resume;
    wrong_resume.model = Some("another-model".into());
    assert!(
        validate_resume(
            AcpProviderFlavor::Qwen,
            "provider-session",
            "/tmp",
            &wrong_resume,
            &[],
        )
        .is_err()
    );
    assert!(
        validate_resume(
            AcpProviderFlavor::Custom,
            "provider-session",
            "/tmp",
            &wrong_resume,
            &[],
        )
        .is_err()
    );
}

#[test]
fn explicit_chat_kind_is_refused_for_every_flavor_on_launch_and_resume() {
    let chat = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Chat),
        ..LaunchOptions::default()
    };
    let resume = ResumeOptions {
        kind: Some(SessionKind::Chat),
        ..ResumeOptions::default()
    };
    for flavor in [
        AcpProviderFlavor::Grok,
        AcpProviderFlavor::Qwen,
        AcpProviderFlavor::Custom,
    ] {
        let error = validate_launch(flavor, &chat, &[]).expect_err("ACP chat must fail closed");
        assert_eq!(error.message, ACP_CHAT_REFUSAL);
        assert_eq!(error.kind, EngineErrorKind::InvalidInput);
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
        let resume_error = validate_resume(flavor, "provider-session", "/tmp", &resume, &[])
            .expect_err("ACP chat resume must fail closed");
        assert_eq!(resume_error.message, ACP_CHAT_REFUSAL);
        assert_eq!(resume_error.kind, EngineErrorKind::InvalidInput);
        assert_eq!(
            resume_error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
    }

    let mut allowed = chat.clone();
    allowed.kind = Some(SessionKind::Code);
    assert!(validate_launch(AcpProviderFlavor::Grok, &allowed, &[]).is_ok());
    allowed.kind = Some(SessionKind::Agent);
    assert!(validate_launch(AcpProviderFlavor::Grok, &allowed, &[]).is_ok());
    allowed.kind = None;
    assert!(validate_launch(AcpProviderFlavor::Grok, &allowed, &[]).is_ok());
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &ResumeOptions {
                kind: Some(SessionKind::Code),
                ..ResumeOptions::default()
            },
            &[],
        )
        .is_ok()
    );
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &ResumeOptions {
                kind: Some(SessionKind::Agent),
                ..ResumeOptions::default()
            },
            &[],
        )
        .is_ok()
    );
}

#[test]
fn empty_tool_lists_and_fast_mode_match_the_frozen_acp_boundary() {
    let launch = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        allowed_tools: Some(Vec::new()),
        disallowed_tools: Some(Vec::new()),
        fast_mode: Some(true),
        ..LaunchOptions::default()
    };
    assert!(validate_launch(AcpProviderFlavor::Grok, &launch, &[]).is_ok());

    let mut scoped = launch.clone();
    scoped.setting_sources = Some(Vec::new());
    assert!(validate_launch(AcpProviderFlavor::Grok, &scoped, &[]).is_err());

    let mut constrained = launch;
    constrained.allowed_tools = Some(vec!["Read".into()]);
    assert!(validate_launch(AcpProviderFlavor::Grok, &constrained, &[]).is_err());

    let empty_resume = ResumeOptions {
        disallowed_tools: Some(Vec::new()),
        ..ResumeOptions::default()
    };
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &empty_resume,
            &[],
        )
        .is_ok()
    );

    let mut scoped_resume = empty_resume.clone();
    scoped_resume.setting_sources = Some(Vec::new());
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &scoped_resume,
            &[],
        )
        .is_err()
    );

    let mut constrained_resume = empty_resume;
    constrained_resume.disallowed_tools = Some(vec!["Bash".into()]);
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &constrained_resume,
            &[],
        )
        .is_err()
    );
}

#[test]
fn qwen_and_custom_profiles_refuse_grok_collaboration_injection() {
    let state = Arc::new(CollaborationTestState::default());
    let factory = || CollaborationTestFactory::shared(&state, COLLABORATION_SOCKET);
    let qwen = AcpAdapter::new(
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Qwen, "/opt/qwen".into()).unwrap(),
    )
    .unwrap();
    assert!(qwen.with_collaboration_factory(factory()).is_err());

    let profile = validate_profile(AcpProfile {
        slug: "custom".into(),
        display_name: "Custom".into(),
        executable_path: "/opt/custom".into(),
        args: Vec::new(),
    })
    .unwrap();
    let custom = AcpAdapter::new(AcpAdapterConfig::custom(profile)).unwrap();
    assert!(custom.with_collaboration_factory(factory()).is_err());
    assert!(lock(&state.requests).is_empty());
}

#[tokio::test]
async fn provider_flavors_keep_distinct_store_and_command_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let mut grok_config =
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, "/opt/grok".into()).unwrap();
    grok_config.grok_sessions_root = root.path().join("grok");
    let grok = AcpAdapter::new(grok_config).unwrap();
    assert_eq!(
        grok.supported_ops(),
        vec![EngineOp::DeleteSession, EngineOp::ReadUsage]
    );
    assert_eq!(
        grok.list_provider_commands(ProviderCommandRequest::default())
            .await
            .unwrap()
            .reason
            .as_deref(),
        Some(
            "Grok exposes project commands only after a chat session starts. Interface did not create a hidden chat for autocomplete."
        )
    );
    let listing = grok.list_sessions().await.unwrap();
    assert!(listing.complete);
    assert!(listing.sessions.is_empty());

    let qwen = AcpAdapter::new(
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Qwen, "/opt/qwen".into()).unwrap(),
    )
    .unwrap();
    assert!(qwen.supported_ops().is_empty());
    let commands = qwen
        .list_provider_commands(ProviderCommandRequest::default())
        .await
        .unwrap();
    assert_eq!(
        commands.reason.as_deref(),
        Some(
            "Qwen ACP exposes commands only inside a chat session. Interface did not create a hidden session for autocomplete."
        )
    );
    let listing = qwen.list_sessions().await.unwrap();
    assert_eq!(listing.unavailable, Some(true));
    assert_eq!(
        listing.error.as_deref(),
        Some("Qwen ACP does not expose a stored-session listing.")
    );
    let history = qwen
        .read_history(
            "session".into(),
            HistoryRequest {
                cwd: "/tmp".into(),
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        history.reason.as_deref(),
        Some("Qwen ACP does not expose stored transcript replay.")
    );
}

#[tokio::test]
async fn grok_adapter_store_operations_preserve_results_and_delete_certainty() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("%2Fwork%2Fa").join("grok-1");
    fs::create_dir_all(&session).unwrap();
    fs::write(
        session.join("summary.json"),
        r#"{
              "info":{"id":"grok-1","cwd":"/work/a"},
              "generated_title":"Fixture", "created_at":"2026-08-01T00:00:00Z",
              "last_active_at":"2026-08-02T00:00:00Z", "current_model_id":"grok-4.6"
            }"#,
    )
    .unwrap();
    fs::write(
        session.join("chat_history.jsonl"),
        concat!(
            "{\"type\":\"user\",\"content\":\"<user_query>hello</user_query>\"}\n",
            "{\"type\":\"assistant\",\"content\":\"world\"}\n"
        ),
    )
    .unwrap();
    let mut config =
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, "/opt/grok".into()).unwrap();
    config.grok_sessions_root = root.path().to_path_buf();
    let grok = AcpAdapter::new(config).unwrap();

    let listing = grok.list_sessions().await.unwrap();
    assert!(listing.complete);
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].session_id, "grok-1");
    assert_eq!(listing.sessions[0].title, "Fixture");

    let history = grok
        .read_history(
            "grok-1".into(),
            HistoryRequest {
                cwd: "/work/a".into(),
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert_eq!(history.messages, Some(2));
    assert_eq!(history.older_messages, 0);

    grok.delete_session("grok-1".into(), Some("/work/a".into()))
        .await
        .unwrap();
    assert!(!session.exists());
    let error = grok
        .delete_session("grok-1".into(), Some("/work/a".into()))
        .await
        .expect_err("a missing stored session must remain a provider rejection");
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::DefinitelyRejected
    );
}

#[tokio::test(flavor = "current_thread")]
async fn grok_store_read_keeps_the_async_runtime_responsive() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let store_work = run_grok_store(false, move || {
        let _ = started_tx.send(());
        release_rx.recv_timeout(Duration::from_secs(1)).is_ok()
    });
    let release = async move {
        started_rx.await.expect("blocking store work started");
        release_tx
            .send(())
            .expect("async runtime released blocking store work");
    };

    let (completed, ()) = tokio::join!(store_work, release);

    assert!(
        completed.expect("blocking read joined"),
        "the current-thread runtime did not run while store work was blocked"
    );
}

#[tokio::test]
async fn grok_store_join_failures_preserve_effect_certainty() {
    let read_error = run_grok_store(false, || -> () { panic!("read worker panic") })
        .await
        .expect_err("read worker panic must fail closed");
    assert_eq!(read_error.kind, EngineErrorKind::Internal);
    assert_eq!(
        read_error.certainty,
        interface_engine::EffectCertainty::DefinitelyUnsent
    );

    let mutation_error = run_grok_store(true, || -> () { panic!("mutation worker panic") })
        .await
        .expect_err("mutation worker panic must be ambiguous");
    assert_eq!(mutation_error.kind, EngineErrorKind::Internal);
    assert_eq!(
        mutation_error.certainty,
        interface_engine::EffectCertainty::OutcomeUnknown
    );
}

#[tokio::test]
async fn grok_probe_uses_version_and_provider_model_extension() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
if "--version" in sys.argv:
    print("grok-test 1.2.3")
    raise SystemExit(0)
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"ignored-acp"},"agentCapabilities":{}}
    elif method == "x.ai/models/list":
        result = {"models":{"availableModels":[{"modelId":"provider-grok","name":"Provider Grok","_meta":{"reasoningEfforts":["low","xhigh"]}}]}}
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let mut config = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, executable).unwrap();
    config.transport.request_timeout = Duration::from_secs(2);
    let info = AcpAdapter::new(config).unwrap().probe().await.unwrap();
    assert_eq!(info.version, "grok-test 1.2.3");
    assert_eq!(info.display_name, None);
    assert!(info.capabilities.resume);
    assert_eq!(
        info.capabilities.permission_modes,
        vec!["default", "auto", "bypassPermissions"]
    );
    assert_eq!(
        info.model_catalog.as_ref().unwrap()[0].value,
        "provider-grok"
    );
}

#[tokio::test]
async fn grok_version_is_asked_once_per_executable_identity() {
    let script = |version: &str| {
        format!(
            r#"
import os, sys
if "--version" in sys.argv:
    with open(os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "asked"), "a") as log:
        log.write("x")
    print("{version}")
"#
        )
    };
    let (directory, executable) = fake_agent(&script("grok-test 1.0.0"));
    let asked = || fs::read_to_string(directory.path().join("asked")).map_or(0, |log| log.len());
    let config = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, executable.clone()).unwrap();
    let adapter = AcpAdapter::new(config).unwrap();
    // The runtime keeps a probe only while this same file is unchanged.
    assert_eq!(adapter.executable(), Some(executable.clone()));

    assert_eq!(adapter.grok_version().await.unwrap(), "grok-test 1.0.0");
    assert_eq!(adapter.grok_version().await.unwrap(), "grok-test 1.0.0");
    assert_eq!(asked(), 1, "an unchanged binary is not asked again");

    // An update installs a new file at the same path (a new inode).
    let (_update, replacement) = fake_agent(&script("grok-test 2.0.0"));
    fs::copy(&replacement, directory.path().join("next")).unwrap();
    fs::rename(directory.path().join("next"), &executable).unwrap();
    assert_eq!(adapter.grok_version().await.unwrap(), "grok-test 2.0.0");
    assert_eq!(asked(), 2);
}

#[tokio::test]
async fn grok_probe_keeps_models_dynamic_when_the_private_extension_fails() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
if "--version" in sys.argv:
    print("grok-test 1.2.3")
    raise SystemExit(0)
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id":msg.get("id"),"result":{"protocolVersion":1,"agentCapabilities":{}}}), flush=True)
    elif method == "x.ai/models/list":
        print(json.dumps({"id":msg.get("id"),"error":{"code":-32601,"message":"method not found"}}), flush=True)
"#,
    );
    let mut config = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, executable).unwrap();
    config.transport.request_timeout = Duration::from_secs(2);
    let adapter = AcpAdapter::new(config).unwrap();
    let info = adapter
        .probe()
        .await
        .expect("Grok handshake remains healthy");

    assert_eq!(info.version, "grok-test 1.2.3");
    assert!(matches!(
        info.capabilities.models,
        EngineModels::Dynamic(ref value) if value == "dynamic"
    ));
    assert!(info.capabilities.effort_levels.is_empty());
    assert!(info.model_catalog.is_none());
    assert!(lock(&adapter.probed_effort_levels).is_empty());
}

#[tokio::test]
async fn portable_effort_controls_use_only_the_last_probed_catalogue() {
    let (_directory, executable) = fake_agent(PORTABLE_EFFORT_AGENT);
    let adapter = custom_adapter(executable);
    let info = adapter.probe().await.expect("portable probe");
    assert_eq!(info.capabilities.effort_levels, ["low", "high"]);
    assert_eq!(
        lock(&adapter.probed_effort_levels).as_slice(),
        ["low", "high"]
    );

    let bad_launch = match adapter.create_session(LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        effort: Some("xhigh".into()),
        ..LaunchOptions::default()
    }) {
        Ok(_) => panic!("portable launch accepted an unadvertised effort"),
        Err(error) => error,
    };
    assert!(bad_launch.message.contains("effort"));
    let bad_resume = match adapter.resume_session(
        "stored-portable".into(),
        "/tmp".into(),
        ResumeOptions {
            effort: Some("xhigh".into()),
            ..ResumeOptions::default()
        },
    ) {
        Ok(_) => panic!("portable resume accepted an unadvertised effort"),
        Err(error) => error,
    };
    assert!(bad_resume.message.contains("effort"));

    let mut launched = adapter
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            effort: Some("low".into()),
            ..LaunchOptions::default()
        })
        .expect("advertised portable effort launch");
    let mut events = launched.take_events().expect("portable events");
    launched.ready().await.expect("portable launch ready");
    loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("portable prompt event timeout")
            .expect("portable prompt stream")
            .expect("portable prompt event");
        if matches!(
            event.event,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            }
        ) {
            break;
        }
    }
    assert!(
        launched
            .set_effort(Some("high".into()))
            .await
            .is_confirmed()
    );
    assert!(matches!(
        launched.set_effort(Some("xhigh".into())).await,
        DeliveryOutcome::DefinitelyUnsent(error) if error.message.contains("does not offer")
    ));
    launched.close().await.expect("close portable launch");

    let resumed = adapter
        .resume_session(
            "stored-portable".into(),
            "/tmp".into(),
            ResumeOptions {
                effort: Some("high".into()),
                ..ResumeOptions::default()
            },
        )
        .expect("advertised portable effort resume");
    resumed.ready().await.expect("portable resume ready");
    assert!(resumed.send("resumed".into()).await.is_confirmed());
    resumed.close().await.expect("close portable resume");
}

#[test]
fn qwen_catalog_disables_effort_and_appends_the_fixed_local_model() {
    let catalog = model_catalog(
        &json!({"models":{"availableModels":[{"modelId":"portable",
            "name":"Portable", "_meta":{"reasoningEfforts":["high"]}}]}}),
        AcpProviderFlavor::Qwen,
    );
    assert_eq!(catalog.len(), 2);
    assert_eq!(catalog[0].value, "portable");
    assert!(!catalog[0].supports_effort);
    assert!(catalog[0].effort_levels.is_empty());
    assert_eq!(catalog[1].value, "mlx-community/Qwen3.8-27B-4bit");
}

#[tokio::test]
async fn fake_agent_proves_readiness_mapping_and_exact_permission_response() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
session = "session-exact"
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{"loadSession":True}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt":
        print(json.dumps({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private"}}}}), flush=True)
        print(json.dumps({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"visible"}}}}), flush=True)
        result = {"stopReason":"end_turn"}
    elif method == "session/cancel":
        continue
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
    if method == "session/new":
        print(json.dumps({"id":"permission-1","method":"session/request_permission","params":{"sessionId":session,"toolCall":{"kind":"edit","title":"Edit file","rawInput":{"path":"/tmp/a"}},"options":[{"optionId":"allow-1","kind":"allow_once"},{"optionId":"deny-1","kind":"reject_once"}]}}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "session-exact");
    assert_eq!(handle.session_id(), "session-exact");
    let mut events = handle.take_events().unwrap();
    let mut saw_visible = false;
    let mut approval_id = None;
    for _ in 0..12 {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match event.event {
            EngineEventKind::MessageDelta { text, .. } if text == "visible" => {
                saw_visible = true;
            }
            EngineEventKind::ApprovalRequested {
                approval_id: id, ..
            } => approval_id = Some(id),
            _ => {}
        }
        if saw_visible && approval_id.is_some() {
            break;
        }
    }
    assert!(saw_visible);
    let approval_id = approval_id.expect("fake agent requested permission");
    assert!(
        handle
            .respond_to_approval(
                approval_id.clone(),
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .is_confirmed()
    );
    assert!(
        handle
            .respond_to_approval(
                approval_id,
                ApprovalVerdict::Allow,
                ApprovalResponse::default(),
            )
            .await
            .replay_is_safe()
    );
    handle.close().await.unwrap();
}

#[tokio::test]
async fn interrupt_cancels_parked_permissions_before_provider_cancel() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
session = "cancel-order-session"
permission_cancelled = False
cancel_seen = False
bad_order = False
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        if ident != "permission-cancel-order" or msg.get("result") != {"outcome":{"outcome":"cancelled"}}:
            bad_order = True
        permission_cancelled = True
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt":
        text = msg.get("params", {}).get("prompt", [{}])[0].get("text", "")
        if text == "after" and (bad_order or not cancel_seen):
            print(json.dumps({"id":ident,"error":{"code":-1,"message":"permission was not cancelled before session/cancel"}}), flush=True)
            continue
        result = {"stopReason":"end_turn"}
    elif method == "session/cancel":
        if not permission_cancelled:
            bad_order = True
        cancel_seen = True
        continue
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
    if method == "session/new":
        print(json.dumps({"id":"permission-cancel-order","method":"session/request_permission","params":{"sessionId":session,"toolCall":{"kind":"edit","title":"Edit file"},"options":[{"optionId":"deny-1","kind":"reject_once"}]}}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .expect("permission cancellation session");
    let mut events = handle.take_events().expect("permission events");
    handle.ready().await.expect("permission session ready");
    let mut approval_id = None;
    let mut idle = false;
    while approval_id.is_none() || !idle {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("permission event timeout")
            .expect("permission stream")
            .expect("permission event");
        match event.event {
            EngineEventKind::ApprovalRequested {
                approval_id: id, ..
            } => approval_id = Some(id),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } => idle = true,
            _ => {}
        }
    }

    assert!(handle.interrupt().await.is_confirmed());
    let stale = handle
        .respond_to_approval(
            approval_id.expect("parked approval"),
            ApprovalVerdict::Deny,
            ApprovalResponse::default(),
        )
        .await;
    assert!(matches!(stale, DeliveryOutcome::DefinitelyUnsent(_)));
    assert!(handle.send("after".into()).await.is_confirmed());
    handle.close().await.expect("close cancellation session");
}

/// The next approval the agent asked for, skipping every other event.
async fn next_approval(events: &mut BoundedEventStream<EngineEvent>) -> String {
    loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval event timeout")
            .expect("approval stream")
            .expect("approval event");
        if let EngineEventKind::ApprovalRequested { approval_id, .. } = event.event {
            return approval_id;
        }
    }
}

#[tokio::test]
async fn a_parked_prompt_reports_its_dispatch_and_stays_answerable_and_stoppable() {
    // After the launch prompt, the agent parks every turn on a permission and
    // answers session/prompt only once that permission is answered (allowed)
    // or cancelled (Stop).
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
session = "parked-session"
prompt_id = None
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        if prompt_id is not None:
            outcome = msg.get("result", {}).get("outcome", {})
            reason = "cancelled" if outcome.get("outcome") == "cancelled" else "end_turn"
            print(json.dumps({"id":prompt_id,"result":{"stopReason":reason}}), flush=True)
            prompt_id = None
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt" and msg["params"]["prompt"][0]["text"] == "hello":
        result = {"stopReason":"end_turn"}
    elif method == "session/prompt":
        prompt_id = ident
        print(json.dumps({"id":"permission-" + str(ident),"method":"session/request_permission","params":{"sessionId":session,"toolCall":{"kind":"edit","title":"Edit file"},"options":[{"optionId":"allow-1","kind":"allow_once"},{"optionId":"deny-1","kind":"reject_once"}]}}), flush=True)
        continue
    elif method == "session/cancel":
        continue
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            ..LaunchOptions::default()
        })
        .expect("parked session");
    let mut events = handle.take_events().expect("parked events");
    handle.ready().await.expect("parked session ready");
    // The launch prompt's turn ends first.
    loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("launch event timeout")
            .expect("launch stream")
            .expect("launch event");
        if matches!(
            event.event,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            }
        ) {
            break;
        }
    }
    let handle: Arc<dyn SessionHandle> = Arc::from(handle);

    for (text, allow) in [("answered", true), ("stopped", false)] {
        let (dispatched, queued) = oneshot::channel();
        let sending = {
            let handle = Arc::clone(&handle);
            tokio::spawn(async move {
                handle
                    .send_reporting_dispatch(text.into(), dispatched)
                    .await
            })
        };
        timeout(Duration::from_secs(2), queued)
            .await
            .expect("the prompt reported its dispatch before the turn ended")
            .expect("dispatch fired rather than dropped");
        let approval_id = next_approval(&mut events).await;
        assert!(
            !sending.is_finished(),
            "the turn is parked on the permission"
        );
        if allow {
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
        } else {
            assert!(handle.interrupt().await.is_confirmed());
        }
        let outcome = timeout(Duration::from_secs(2), sending)
            .await
            .expect("the turn ended")
            .expect("send task");
        assert!(outcome.is_confirmed(), "{text}: {outcome:?}");
    }
    handle.close().await.expect("close parked session");
}

#[tokio::test]
async fn resume_prefers_non_replaying_session_resume_capability() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":1,"agentCapabilities":{"loadSession":True,"sessionCapabilities":{"resume":{}}}}
    elif method == "session/resume":
        result = {"sessionId":"stored"}
    elif method == "session/load":
        print(json.dumps({"id":msg.get("id"),"error":{"code":-1,"message":"load must not be used"}}), flush=True)
        continue
    else:
        result = {}
    print(json.dumps({"id":msg.get("id"),"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let handle = instance
        .resume_session("stored".into(), "/tmp".into(), ResumeOptions::default())
        .unwrap();
    assert_eq!(handle.ready().await.unwrap().session_id, "stored");
    handle.close().await.unwrap();
}

/// Every refusal after the agent was spawned fails the open with its own error
/// and certainty, and shuts the agent down.
#[tokio::test]
async fn open_refusals_after_the_spawn_fail_the_open_and_reap_the_agent() {
    const AGENT: &str = r#"
import json, os, sys, time
with open(PID_PATH, "w") as handle:
    handle.write(str(os.getpid()))
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize": reply = INITIALIZE
    elif method in ("session/new", "session/load", "session/resume"): reply = OPEN
    else: reply = {"result": {}}
    reply["id"] = msg.get("id")
    print(json.dumps(reply), flush=True)
time.sleep(30)
"#;
    const PLAIN: &str = r#"{"result": {"protocolVersion": 1, "agentCapabilities": {}}}"#;
    const LOADS: &str =
        r#"{"result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}}"#;
    use interface_engine::EffectCertainty::{DefinitelyRejected, DefinitelyUnsent};
    let cases = [
        (
            false,
            r#"{"error": {"code": -32000, "message": "initialize refused"}}"#,
            r#"{"result": {}}"#,
            None,
        ),
        (
            false,
            PLAIN,
            r#"{"result": {}}"#,
            Some(("created a session without an id", DefinitelyRejected)),
        ),
        (
            true,
            PLAIN,
            r#"{"result": {"sessionId": "stored"}}"#,
            Some((
                "does not advertise session/resume or session/load",
                DefinitelyUnsent,
            )),
        ),
        (
            true,
            LOADS,
            r#"{"result": {"sessionId": "other"}}"#,
            Some(("resumed a different session id", DefinitelyRejected)),
        ),
    ];
    for (resume, initialize, open, expected) in cases {
        let state = tempfile::tempdir().unwrap();
        let pid_path = state.path().join("pid");
        let pid_literal = serde_json::to_string(&pid_path.to_string_lossy()).unwrap();
        let (_directory, executable) = fake_agent(
            &AGENT
                .replace("PID_PATH", &pid_literal)
                .replace("INITIALIZE", initialize)
                .replace("OPEN", open),
        );
        let instance = custom_adapter(executable);
        let handle = if resume {
            instance.resume_session("stored".into(), "/tmp".into(), ResumeOptions::default())
        } else {
            instance.create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: "must remain unsent".into(),
                ..LaunchOptions::default()
            })
        }
        .unwrap();
        let error = handle.ready().await.expect_err("refused open");
        if let Some((message, certainty)) = expected {
            assert!(
                error.message.contains(message),
                "{open}: expected {message:?}, got {}",
                error.message
            );
            assert_eq!(error.certainty, certainty, "{open}");
        }
        let pid = fs::read_to_string(&pid_path).expect("fake agent never recorded its pid");
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
            "{initialize} {open}: a refused open left the agent running"
        );
    }
}

#[test]
fn confirmed_prompt_stop_reasons_match_the_previous_error_gate() {
    assert_eq!(
        stop_reason_message(&json!({"stopReason": "end_turn"})),
        None
    );
    assert_eq!(
        stop_reason_message(&json!({"stopReason": "cancelled"})),
        None
    );
    assert_eq!(
        stop_reason_message(&json!({"stopReason": "max_tokens"})),
        Some("ACP stopped the turn: max_tokens".into())
    );
    assert_eq!(
        stop_reason_message(&json!({})),
        Some("ACP ended a prompt without a stop reason".into())
    );
}

#[test]
fn builtins_have_distinct_exact_argv_profiles() {
    let grok = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, "/opt/grok".into()).unwrap();
    let qwen = AcpAdapterConfig::builtin(BuiltinAcpProvider::Qwen, "/opt/qwen".into()).unwrap();
    assert_eq!(grok.profile.args, ["--no-auto-update", "agent", "stdio"]);
    assert_eq!(qwen.profile.args, ["--acp"]);
    assert!(qwen.login_shell_path.is_none());
}

#[test]
fn built_in_children_get_the_augmented_search_path_and_custom_profiles_do_not() {
    let environment = builtin_child_environment(
        AcpProviderFlavor::Grok,
        Path::new("/resolved/bin/grok"),
        Some(OsStr::new("/usr/bin:/usr/local/bin")),
        Some(Path::new("/Users/test")),
    )
    .unwrap();
    assert_eq!(
        environment,
        vec![(
            OsString::from("PATH"),
            OsString::from(
                "/resolved/bin:/Users/test/.grok/bin:/Users/test/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin"
            )
        )]
    );
    assert_eq!(
        builtin_child_environment(
            AcpProviderFlavor::Custom,
            Path::new("/opt/fake/bin/agent"),
            Some(OsStr::new("/usr/bin")),
            Some(Path::new("/Users/test")),
        )
        .unwrap(),
        Vec::new()
    );
}

const EXPECTED_GATE_REFUSAL: &str =
    "This ACP agent cannot enforce Interface's approval gate yet, so it can't run as an agent.";

/// An agent's Auto or Bypass mode cannot be confirmed through ACP either, so
/// every flavor refuses it unsent, exactly like the gate.
#[test]
fn agent_access_is_refused_for_every_acp_flavor() {
    use interface_contracts::engine::AgentAccess;
    for flavor in [
        AcpProviderFlavor::Grok,
        AcpProviderFlavor::Qwen,
        AcpProviderFlavor::Custom,
    ] {
        for access in [AgentAccess::Auto, AgentAccess::Bypass] {
            let launch = LaunchOptions {
                cwd: "/tmp".into(),
                prompt: "hello".into(),
                kind: Some(SessionKind::Agent),
                agent_access: Some(access),
                ..LaunchOptions::default()
            };
            let resume = ResumeOptions {
                kind: Some(SessionKind::Agent),
                agent_access: Some(access),
                ..ResumeOptions::default()
            };
            for error in [
                validate_launch(flavor, &launch, &[]).expect_err("agent access launch"),
                validate_resume(flavor, "provider-session", "/tmp", &resume, &[])
                    .expect_err("agent access resume"),
            ] {
                assert_eq!(error.message, EXPECTED_GATE_REFUSAL);
                assert_eq!(
                    error.certainty,
                    interface_engine::EffectCertainty::DefinitelyUnsent
                );
            }
        }
    }
}

#[tokio::test]
async fn approval_gate_and_folder_grants_are_refused_for_every_acp_flavor() {
    let base = LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Agent),
        ..LaunchOptions::default()
    };
    let refused_launches = [
        LaunchOptions {
            approval_gate: Some(true),
            ..base.clone()
        },
        LaunchOptions {
            additional_directories: Some(vec!["/w/p".into()]),
            ..base.clone()
        },
    ];
    let refused_resumes = [
        ResumeOptions {
            approval_gate: Some(true),
            ..ResumeOptions::default()
        },
        ResumeOptions {
            additional_directories: Some(vec!["/w/p".into()]),
            ..ResumeOptions::default()
        },
    ];
    let assert_refusal = |error: EngineError| {
        assert_eq!(error.message, EXPECTED_GATE_REFUSAL);
        assert_eq!(error.kind, EngineErrorKind::Unsupported);
        assert_eq!(
            error.certainty,
            interface_engine::EffectCertainty::DefinitelyUnsent
        );
    };
    for flavor in [
        AcpProviderFlavor::Grok,
        AcpProviderFlavor::Qwen,
        AcpProviderFlavor::Custom,
    ] {
        for launch in &refused_launches {
            assert_refusal(validate_launch(flavor, launch, &[]).expect_err("gated ACP launch"));
        }
        for resume in &refused_resumes {
            assert_refusal(
                validate_resume(flavor, "provider-session", "/tmp", resume, &[])
                    .expect_err("gated ACP resume"),
            );
        }
    }
    let ungated = LaunchOptions {
        approval_gate: Some(false),
        additional_directories: Some(Vec::new()),
        ..base.clone()
    };
    assert!(validate_launch(AcpProviderFlavor::Grok, &ungated, &[]).is_ok());
    assert!(
        validate_resume(
            AcpProviderFlavor::Grok,
            "provider-session",
            "/tmp",
            &ResumeOptions {
                approval_gate: Some(false),
                additional_directories: Some(Vec::new()),
                ..ResumeOptions::default()
            },
            &[],
        )
        .is_ok()
    );

    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("spawned");
    let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let (_agent_directory, executable) =
        fake_agent(&format!("open({marker_literal}, \"w\").write(\"spawned\")"));
    let adapter = custom_adapter(executable);
    let Err(error) = adapter.create_session(LaunchOptions {
        approval_gate: Some(true),
        ..base
    }) else {
        panic!("a gated ACP launch must be refused before any spawn");
    };
    assert_refusal(error);
    let Err(error) = adapter.resume_session(
        "provider-session".into(),
        "/tmp".into(),
        ResumeOptions {
            approval_gate: Some(true),
            ..ResumeOptions::default()
        },
    ) else {
        panic!("a gated ACP resume must be refused before any spawn");
    };
    assert_refusal(error);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!marker.exists());
}

#[test]
#[allow(clippy::too_many_lines)]
fn acp_approval_facts_map_tool_kinds() {
    use interface_contracts::agents::ApprovalFacts;
    let (_directory, executable) = fake_agent("pass");
    let adapter = custom_adapter(executable);
    let unknown = |tool: &str| ApprovalFacts::Unknown { tool: tool.into() };
    let paths = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    let rows = [
        (
            "read",
            json!({"path": "/repo/a.rs"}),
            ApprovalFacts::Read {
                paths: paths(&["/repo/a.rs"]),
            },
        ),
        (
            "read",
            json!({"file_path": "/repo/a.rs", "abs_path": "/repo/b.rs", "paths": ["/repo/c.rs"]}),
            ApprovalFacts::Read {
                paths: paths(&["/repo/a.rs", "/repo/b.rs", "/repo/c.rs"]),
            },
        ),
        ("read", json!({}), unknown("read")),
        ("read", json!({"path": 5}), unknown("read")),
        ("read", json!({"paths": ["/repo/a.rs", 3]}), unknown("read")),
        ("read", json!("/repo/a.rs"), unknown("read")),
        (
            "edit",
            json!({"file_path": "/repo/a.rs", "old_string": "a"}),
            ApprovalFacts::Write {
                paths: paths(&["/repo/a.rs"]),
            },
        ),
        ("edit", json!({"content": "x"}), unknown("edit")),
        (
            "move",
            json!({"paths": ["/repo/a.rs", "/repo/b.rs"]}),
            ApprovalFacts::Write {
                paths: paths(&["/repo/a.rs", "/repo/b.rs"]),
            },
        ),
        (
            "delete",
            json!({"path": "/repo/old.rs"}),
            ApprovalFacts::Delete {
                paths: paths(&["/repo/old.rs"]),
            },
        ),
        ("delete", json!({}), unknown("delete")),
        (
            "search",
            json!({"pattern": "todo", "path": "/repo/src"}),
            ApprovalFacts::Search {
                paths: paths(&["/repo/src"]),
            },
        ),
        (
            "search",
            json!({"pattern": "todo"}),
            ApprovalFacts::Search { paths: Vec::new() },
        ),
        ("search", json!({"path": false}), unknown("search")),
        (
            "execute",
            json!({"command": "ls -la"}),
            ApprovalFacts::Shell {
                command: "ls -la".into(),
                cwd: None,
            },
        ),
        (
            "execute",
            json!({"command": ["git", "status"], "cwd": "/repo"}),
            ApprovalFacts::Shell {
                command: "git status".into(),
                cwd: Some("/repo".into()),
            },
        ),
        ("execute", json!({}), unknown("execute")),
        (
            "execute",
            json!({"command": ["git", "-c", "a.b='", "push", "origin", "HEAD:refs/heads/x'"]}),
            ApprovalFacts::Shell {
                command: r"git -c 'a.b='\''' push origin 'HEAD:refs/heads/x'\'''".into(),
                cwd: None,
            },
        ),
        (
            "execute",
            json!({"command": ["echo", "", "a b", "~/x", "$HOME", "ok_1@%+=:,./-"]}),
            ApprovalFacts::Shell {
                command: "echo '' 'a b' '~/x' '$HOME' ok_1@%+=:,./-".into(),
                cwd: None,
            },
        ),
        (
            "execute",
            json!({"command": ["git", 1]}),
            unknown("execute"),
        ),
        (
            "fetch",
            json!({"url": "https://example.com"}),
            ApprovalFacts::WebFetch {
                url: "https://example.com".into(),
                method: "GET".into(),
            },
        ),
        ("fetch", json!({"query": "x"}), unknown("fetch")),
        (
            "think",
            json!({}),
            ApprovalFacts::Internal {
                tool: "think".into(),
            },
        ),
        (
            "switch_mode",
            json!({"mode": "code"}),
            unknown("switch_mode"),
        ),
        ("other", json!({"path": "/repo"}), unknown("other")),
        ("tool", json!({}), unknown("tool")),
    ];
    for (tool, input, expected) in rows {
        assert_eq!(
            adapter.approval_facts(tool, &input),
            expected,
            "kind {tool} input {input}"
        );
    }
}

/// The thinking signals of one prompt, up to its idle status.
async fn turn_thinking_signals(events: &mut BoundedEventStream<EngineEvent>) -> Vec<bool> {
    let mut signals = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("ACP event timeout")
            .expect("ACP event stream failure")
            .expect("ACP event stream ended");
        match event.event {
            EngineEventKind::ThinkingSignal { active, .. } => signals.push(active),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } => return signals,
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_thought_emits_one_signal_even_when_the_turn_ends_mid_thought() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys, time
session = "session-thinking"
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None or ident is None:
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt":
        # More than the 15 frames the inbound queue once held, in one burst.
        for _ in range(40):
            print(json.dumps({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private"}}}}), flush=True)
        # Updates and the response travel separately; let the updates land first.
        time.sleep(0.2)
        result = {"stopReason":"end_turn"}
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "think".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let mut events = handle.take_events().unwrap();
    handle.ready().await.unwrap();
    // No visible answer ends the thought: the prompt's settlement does.
    assert_eq!(turn_thinking_signals(&mut events).await, [true, false]);
    assert!(handle.send("think again".into()).await.is_confirmed());
    assert_eq!(turn_thinking_signals(&mut events).await, [true, false]);
    handle.close().await.unwrap();
}

#[tokio::test]
async fn a_prompt_no_frame_can_carry_is_refused_before_any_provider_effect() {
    let (directory, executable) = fake_agent(
        r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
open(os.path.join(here, "started"), "a").close()
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None or ident is None:
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{}}
    elif method == "session/new":
        result = {"sessionId":"frame-session"}
    elif method == "session/prompt":
        open(os.path.join(here, "prompts"), "a").write("prompt\n")
        result = {"stopReason":"end_turn"}
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
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
    assert_eq!(
        error.message,
        "ACP launch prompt must be non-empty and bounded"
    );
    assert!(!error.retryable);
    assert_eq!(
        error.certainty,
        interface_engine::EffectCertainty::DefinitelyUnsent
    );
    assert!(
        !directory.path().join("started").exists(),
        "no session was created"
    );

    let handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "hi".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    handle.ready().await.unwrap();
    match handle.send(unframable).await {
        DeliveryOutcome::DefinitelyUnsent(error) => {
            assert_eq!(error.message, "ACP prompt must be non-empty and bounded");
            assert!(!error.retryable);
        }
        other => panic!("an unframable prompt must be refused unsent: {other:?}"),
    }
    handle.close().await.unwrap();
    // Only the small launch prompt ever reached the agent.
    let prompts = fs::read_to_string(directory.path().join("prompts")).unwrap_or_default();
    assert!(prompts.lines().count() <= 1, "{prompts:?}");
}

#[tokio::test]
async fn concurrent_approvals_stay_waiting_until_last_permission_is_confirmed() {
    let (_directory, executable) = fake_agent(
        r#"
import json, sys
session = "session-concurrent-permissions"
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method is None:
        if ident == "permission-1":
            print(json.dumps({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"first"}}}}), flush=True)
        elif ident == "permission-2":
            print(json.dumps({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"second"}}}}), flush=True)
        continue
    if method == "initialize":
        result = {"protocolVersion":1,"agentInfo":{"version":"fake-1"},"agentCapabilities":{"loadSession":True}}
    elif method == "session/new":
        result = {"sessionId":session}
    elif method == "session/prompt":
        print(json.dumps({"id":"permission-1","method":"session/request_permission","params":{"sessionId":session,"toolCall":{"kind":"edit","title":"Edit A","rawInput":{"path":"/tmp/a"}},"options":[{"optionId":"allow-1","kind":"allow_once"},{"optionId":"deny-1","kind":"reject_once"}]}}), flush=True)
        print(json.dumps({"id":"permission-2","method":"session/request_permission","params":{"sessionId":session,"toolCall":{"kind":"edit","title":"Edit B","rawInput":{"path":"/tmp/b"}},"options":[{"optionId":"allow-2","kind":"allow_once"},{"optionId":"deny-2","kind":"reject_once"}]}}), flush=True)
        continue
    elif method == "session/cancel":
        continue
    else:
        result = {}
    print(json.dumps({"id":ident,"result":result}), flush=True)
"#,
    );
    let instance = custom_adapter(executable);
    let mut handle = instance
        .create_session(LaunchOptions {
            cwd: "/tmp".into(),
            prompt: "edit both".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    assert_eq!(
        handle.ready().await.unwrap().session_id,
        "session-concurrent-permissions"
    );
    let mut events = handle.take_events().unwrap();
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
    let mut saw_first = false;
    while let Ok(event) = timeout(Duration::from_millis(200), events.recv()).await {
        let event = event.unwrap().unwrap();
        status = interface_contracts::engine::next_status(status, &event);
        saw_first |= matches!(event.event, EngineEventKind::MessageDelta { .. });
    }
    assert!(saw_first);
    assert_eq!(
        status,
        SessionStatus::Waiting,
        "answering the first of two concurrent ACP permissions must keep Waiting"
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

/// Team tools (protocol 57): every ACP flavor takes stdio MCP servers, so an
/// ordinary code session's `session/new` carries `iface_team` beside Grok's
/// collaboration server, confirmed for the id the agent gives; `session/load`
/// re-attaches it, confirmed at once; a session one hop down (depth 1, as a
/// started thread runs) never asks.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Grok and a custom profile: a launch, a load, then depth one.
async fn every_acp_flavor_gets_team_tools_in_session_new_and_load_exactly_when_it_should() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("opens.jsonl");
    let log_literal = serde_json::to_string(&log.to_string_lossy()).unwrap();
    let script = format!(
        r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {{"protocolVersion":1,"agentCapabilities":{{"loadSession":True}}}}
    elif method in ("session/new", "session/load"):
        with open({log_literal}, "a") as out:
            out.write(json.dumps({{"method": method, "servers": msg["params"].get("mcpServers")}}) + "\n")
        result = {{"sessionId": msg["params"].get("sessionId") or "agent-final"}}
    else:
        result = {{}}
    if msg.get("id") is not None:
        print(json.dumps({{"id":msg.get("id"),"result":result}}), flush=True)
"#
    );
    let opens = || -> Vec<Value> {
        fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let team = json!({"name":"iface_team","command":"/usr/bin/nc",
        "args":["-U","/tmp/iface-tt-test/s.sock"],"env":[]});
    for grok in [true, false] {
        let _ = fs::remove_file(&log);
        let (_agent, executable) = fake_agent(&script);
        let state = Arc::new(CollaborationTestState::default());
        let collaboration = Arc::new(CollaborationTestState::default());
        let factory = TeamToolsTestFactory::shared(&state, "/tmp/iface-tt-test/s.sock", true);
        let instance = if grok {
            grok_adapter(executable)
                .with_collaboration_factory(CollaborationTestFactory::shared(
                    &collaboration,
                    COLLABORATION_SOCKET,
                ))
                .unwrap()
                .with_team_tools(factory)
        } else {
            custom_adapter(executable).with_team_tools(factory)
        };
        let engine = if grok { "acp:grok" } else { "acp:fake" };
        let handle = instance
            .create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: "hello".into(),
                ..LaunchOptions::default()
            })
            .unwrap();
        timeout(Duration::from_secs(30), handle.ready())
            .await
            .unwrap()
            .unwrap();
        let servers = opens()[0]["servers"].as_array().unwrap().clone();
        assert!(servers.contains(&team), "{servers:?}");
        assert_eq!(servers.len(), if grok { 2 } else { 1 });
        assert_eq!(
            lock(&state.confirmed).as_slice(),
            [format!("{engine}#agent-final")]
        );
        // The agent's own process is named to the binding (the host
        // answers only it and the processes it starts).
        let providers = lock(&state.providers).clone();
        assert_eq!(providers.len(), 1, "{providers:?}");
        assert!(providers[0] != 0 && providers[0] != std::process::id());
        handle.close().await.unwrap();
        assert_eq!(
            state.disposed.load(Ordering::Acquire),
            1,
            "revoked at close"
        );

        let resumed = instance
            .resume_session("stored-one".into(), "/tmp".into(), ResumeOptions::default())
            .unwrap();
        timeout(Duration::from_secs(30), resumed.ready())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(opens()[1]["method"], "session/load");
        assert!(opens()[1]["servers"].as_array().unwrap().contains(&team));
        assert_eq!(
            lock(&state.confirmed).last().cloned(),
            Some(format!("{engine}#stored-one"))
        );
        assert_eq!(lock(&state.providers).len(), 2, "the loaded agent too");
        resumed.close().await.unwrap();

        let asked = lock(&state.requests).len();
        let peer = instance
            .create_session(LaunchOptions {
                cwd: "/tmp".into(),
                prompt: "hello".into(),
                collaboration_depth: Some(1),
                ..LaunchOptions::default()
            })
            .unwrap();
        timeout(Duration::from_secs(30), peer.ready())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            lock(&state.requests).len(),
            asked,
            "one hop down never asks"
        );
        assert!(!opens()[2]["servers"].as_array().unwrap().contains(&team));
        peer.close().await.unwrap();
    }
}

/// Several accounts per provider (protocol 60): an added Grok account's
/// adapter starts its agent with that account's `GROK_HOME`; the Default
/// account's sets nothing new, so its agent sees what the host inherited.
#[tokio::test]
async fn a_grok_account_adapter_starts_its_agent_with_its_grok_home() {
    let record = tempfile::tempdir().unwrap();
    let log = record.path().join("homes.txt");
    let (_directory, executable) = fake_agent(&format!(
        r#"
import json, os, sys
with open({log:?}, "a") as out:
    out.write(os.environ.get("GROK_HOME", "<unset>") + "\n")
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {{"protocolVersion":1,"agentInfo":{{"version":"grok-1"}},"agentCapabilities":{{"loadSession":True}}}}
    elif method == "session/new":
        result = {{"sessionId":"grok-session"}}
    elif method == "session/prompt":
        result = {{"stopReason":"end_turn"}}
    else:
        result = {{}}
    if msg.get("id") is not None:
        print(json.dumps({{"id":msg.get("id"),"result":result}}), flush=True)
"#,
        log = log.to_str().unwrap()
    ));
    let account = record.path().join("accounts/grok/acct-work");
    let default = grok_adapter(executable.clone());
    let mut config =
        AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, executable).expect("Grok config");
    config.transport.request_timeout = Duration::from_secs(30);
    config.grok_sessions_root = account.join("sessions");
    config.environment = vec![(
        std::ffi::OsString::from("GROK_HOME"),
        account.clone().into_os_string(),
    )];
    let work = AcpAdapter::new(config).expect("Grok account adapter");
    for instance in [&default, &work] {
        let handle = instance
            .create_session(LaunchOptions {
                cwd: record.path().to_string_lossy().into_owned(),
                prompt: "hello".into(),
                ..LaunchOptions::default()
            })
            .unwrap();
        timeout(Duration::from_secs(30), handle.ready())
            .await
            .expect("ready in time")
            .unwrap();
        handle.close().await.unwrap();
    }
    let inherited = std::env::var("GROK_HOME").unwrap_or_else(|_| "<unset>".to_owned());
    assert_eq!(
        fs::read_to_string(&log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [inherited.as_str(), account.to_str().unwrap()]
    );
}

#[test]
fn resolve_grok_launch_rejects_leading_hyphen_flags() {
    let (_directory, executable) = fake_agent("pass");
    let adapter = grok_adapter(executable);

    let model_injection = adapter.create_session(LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        model: Some("--always-approve".into()),
        ..LaunchOptions::default()
    });
    assert!(
        model_injection.is_err(),
        "leading '-' model must be refused"
    );

    let effort_injection = adapter.create_session(LaunchOptions {
        cwd: "/tmp".into(),
        prompt: "hello".into(),
        effort: Some("--always-approve".into()),
        ..LaunchOptions::default()
    });
    assert!(
        effort_injection.is_err(),
        "leading '-' effort must be refused"
    );

    let config = SessionConfig {
        permission: "default".into(),
        spawn_permission: "default".into(),
        model: Some("-x".into()),
        effort: None,
    };
    assert!(resolve_grok_launch(&config).is_err());
}
