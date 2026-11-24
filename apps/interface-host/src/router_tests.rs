use std::{fs, path::PathBuf, sync::Mutex};

use interface_contracts::ipc::{ALL_CHANNELS, EXTRA_CHANNELS};
use interface_desktop_services::{DesktopServiceConfig, DesktopServiceReviewPorts};
use serde_json::json;
use tempfile::TempDir;

use super::*;

struct RecordingDispatcher {
    owner: &'static str,
    calls: Mutex<Vec<String>>,
}

impl RecordingDispatcher {
    fn new(owner: &'static str) -> Self {
        Self {
            owner,
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[derive(Default)]
struct MutableSessionRoots {
    roots: Mutex<Vec<PathBuf>>,
}

impl MutableSessionRoots {
    fn replace(&self, roots: Vec<PathBuf>) {
        *self.roots.lock().expect("session roots lock") = roots;
    }
}

impl DesktopSessionRootProvider for MutableSessionRoots {
    fn current_session_roots(&self) -> Result<Vec<PathBuf>, DispatchError> {
        Ok(self.roots.lock().expect("session roots lock").clone())
    }
}

#[test]
fn production_session_runtime_satisfies_the_desktop_root_provider_boundary() {
    fn assert_provider<T: DesktopSessionRootProvider>() {}
    assert_provider::<interface_runtime::SessionRuntime>();
}

type RouterFixture = (
    OperationOwnerRouter,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
    Arc<RecordingDispatcher>,
);

#[async_trait]
impl HostDispatcher for RecordingDispatcher {
    async fn dispatch(&self, operation: &str, _payload: Value) -> Result<Value, DispatchError> {
        self.calls
            .lock()
            .expect("recording dispatcher lock")
            .push(operation.to_owned());
        Ok(Value::String(self.owner.into()))
    }
}

fn router() -> RouterFixture {
    let runtime = Arc::new(RecordingDispatcher::new("runtime"));
    let session_management = Arc::new(RecordingDispatcher::new("session-management"));
    let desktop = Arc::new(RecordingDispatcher::new("desktop"));
    let native = Arc::new(RecordingDispatcher::new("native"));
    let orchestration = Arc::new(RecordingDispatcher::new("orchestration"));
    let collaboration = Arc::new(RecordingDispatcher::new("collaboration"));
    let prompt_context = Arc::new(RecordingDispatcher::new("prompt-context"));
    let terminal = Arc::new(RecordingDispatcher::new("terminal"));
    let updater = Arc::new(RecordingDispatcher::new("updater"));
    let voice = Arc::new(RecordingDispatcher::new("voice"));
    let runtime_owner: Arc<dyn HostDispatcher> = runtime.clone();
    let session_management_owner: Arc<dyn HostDispatcher> = session_management.clone();
    let desktop_owner: Arc<dyn HostDispatcher> = desktop.clone();
    let native_owner: Arc<dyn HostDispatcher> = native.clone();
    let orchestration_owner: Arc<dyn HostDispatcher> = orchestration.clone();
    let collaboration_owner: Arc<dyn HostDispatcher> = collaboration.clone();
    let prompt_context_owner: Arc<dyn HostDispatcher> = prompt_context.clone();
    let terminal_owner: Arc<dyn HostDispatcher> = terminal.clone();
    let updater_owner: Arc<dyn HostDispatcher> = updater.clone();
    let voice_owner: Arc<dyn HostDispatcher> = voice.clone();
    let router = OperationOwnerRouter::new(
        runtime_owner,
        session_management_owner,
        desktop_owner,
        native_owner,
        orchestration_owner,
        collaboration_owner,
        prompt_context_owner,
        terminal_owner,
        updater_owner,
        voice_owner,
    )
    .expect("valid owners");
    (
        router,
        runtime,
        session_management,
        desktop,
        native,
        orchestration,
        collaboration,
        prompt_context,
        terminal,
        updater,
        voice,
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_migrated_operation_routes_to_exactly_one_owner() {
    let (
        router,
        runtime,
        session_management,
        desktop,
        native,
        orchestration,
        collaboration,
        prompt_context,
        terminal,
        updater,
        voice,
    ) = router();

    for operation in RUNTIME_BRIDGE_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("runtime operation"),
            "runtime"
        );
    }
    for operation in SESSION_MANAGEMENT_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("session-management operation"),
            "session-management"
        );
    }
    for operation in COVERED_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("desktop operation"),
            "desktop"
        );
    }
    for operation in INTERNAL_DESKTOP_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("internal desktop operation"),
            "desktop"
        );
    }
    for operation in NATIVE_SERVICE_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("native operation"),
            "native"
        );
    }
    for operation in ORCHESTRATION_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("orchestration operation"),
            "orchestration"
        );
    }
    for operation in COLLABORATION_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("collaboration operation"),
            "collaboration"
        );
    }
    for operation in PROMPT_CONTEXT_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("prompt-context operation"),
            "prompt-context"
        );
    }
    for operation in TERMINAL_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("terminal operation"),
            "terminal"
        );
    }
    for operation in UPDATER_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("updater operation"),
            "updater"
        );
    }
    for operation in INTERNAL_UPDATER_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("native updater operation"),
            "updater"
        );
    }
    for operation in VOICE_OPERATIONS {
        assert_eq!(
            router
                .dispatch(operation, Value::Null)
                .await
                .expect("voice operation"),
            "voice"
        );
    }

    assert_eq!(runtime.calls.lock().expect("runtime calls").len(), 13);
    assert_eq!(
        session_management
            .calls
            .lock()
            .expect("session-management calls")
            .len(),
        15
    );
    assert_eq!(desktop.calls.lock().expect("desktop calls").len(), 30);
    assert_eq!(native.calls.lock().expect("native calls").len(), 4);
    assert_eq!(
        orchestration
            .calls
            .lock()
            .expect("orchestration calls")
            .len(),
        23
    );
    assert_eq!(
        prompt_context
            .calls
            .lock()
            .expect("prompt-context calls")
            .len(),
        4
    );
    assert_eq!(
        collaboration
            .calls
            .lock()
            .expect("collaboration calls")
            .len(),
        9
    );
    assert_eq!(updater.calls.lock().expect("updater calls").len(), 6);
    assert_eq!(terminal.calls.lock().expect("terminal calls").len(), 14);
    assert_eq!(voice.calls.lock().expect("voice calls").len(), 4);
    assert_eq!(COMPOSITE_OWNED_OPERATION_COUNT, 127);
}

#[test]
fn duplicate_ownership_is_rejected_before_serving() {
    let error = validate_operation_owners(&[
        &["iface:listSessions", "iface:getAppearance"],
        &["iface:getAppearance"],
    ])
    .expect_err("duplicate owner");
    assert_eq!(error.operation, "iface:getAppearance");
    validate_operation_owners(&[
        &RUNTIME_BRIDGE_OPERATIONS,
        &SESSION_MANAGEMENT_OPERATIONS,
        &COVERED_OPERATIONS,
        &INTERNAL_DESKTOP_OPERATIONS,
        &NATIVE_SERVICE_OPERATIONS,
        &ORCHESTRATION_OPERATIONS,
        &COLLABORATION_OPERATIONS,
        &PROMPT_CONTEXT_OPERATIONS,
        &TERMINAL_OPERATIONS,
        &UPDATER_OPERATIONS,
        &INTERNAL_UPDATER_OPERATIONS,
        &AWAY_OPERATIONS,
        &VOICE_OPERATIONS,
        &REMOTE_OPERATIONS,
        &THREAD_OPERATIONS,
        &DIAGNOSTICS_OPERATIONS,
        &crate::provider_accounts::PROVIDER_ACCOUNTS_OPERATIONS,
    ])
    .expect("production ownership is disjoint");
}

#[tokio::test]
async fn every_other_invokable_protocol_operation_fails_explicitly() {
    let (
        router,
        runtime,
        session_management,
        desktop,
        native,
        orchestration,
        collaboration,
        prompt_context,
        terminal,
        updater,
        voice,
    ) = router();
    let mut remaining = 0;
    for channel in ALL_CHANNELS.iter().chain(EXTRA_CHANNELS) {
        let request = crate::protocol::BridgeRequest {
            version: crate::protocol::PROTOCOL_VERSION,
            id: "router-test".into(),
            operation: channel.value.into(),
            payload: Value::Null,
        };
        if request.validate().is_err()
            || RUNTIME_BRIDGE_OPERATIONS.contains(&channel.value)
            || SESSION_MANAGEMENT_OPERATIONS.contains(&channel.value)
            || COVERED_OPERATIONS.contains(&channel.value)
            || NATIVE_SERVICE_OPERATIONS.contains(&channel.value)
            || ORCHESTRATION_OPERATIONS.contains(&channel.value)
            || COLLABORATION_OPERATIONS.contains(&channel.value)
            || PROMPT_CONTEXT_OPERATIONS.contains(&channel.value)
            || TERMINAL_OPERATIONS.contains(&channel.value)
            || UPDATER_OPERATIONS.contains(&channel.value)
            || AWAY_OPERATIONS.contains(&channel.value)
            || VOICE_OPERATIONS.contains(&channel.value)
            || REMOTE_OPERATIONS.contains(&channel.value)
            || THREAD_OPERATIONS.contains(&channel.value)
            || DIAGNOSTICS_OPERATIONS.contains(&channel.value)
            || crate::provider_accounts::PROVIDER_ACCOUNTS_OPERATIONS.contains(&channel.value)
        {
            continue;
        }
        let error = router
            .dispatch(channel.value, Value::Null)
            .await
            .expect_err("unowned operation");
        assert_eq!(error.code, OPERATION_NOT_MIGRATED_CODE, "{}", channel.value);
        assert!(error.replay_safe, "{}", channel.value);
        remaining += 1;
    }
    assert_eq!(remaining, 0);
    assert!(runtime.calls.lock().expect("runtime calls").is_empty());
    assert!(
        session_management
            .calls
            .lock()
            .expect("session-management calls")
            .is_empty()
    );
    assert!(desktop.calls.lock().expect("desktop calls").is_empty());
    assert!(native.calls.lock().expect("native calls").is_empty());
    assert!(
        orchestration
            .calls
            .lock()
            .expect("orchestration calls")
            .is_empty()
    );
    assert!(
        collaboration
            .calls
            .lock()
            .expect("collaboration calls")
            .is_empty()
    );
    assert!(
        prompt_context
            .calls
            .lock()
            .expect("prompt-context calls")
            .is_empty()
    );
    assert!(terminal.calls.lock().expect("terminal calls").is_empty());
    assert!(updater.calls.lock().expect("updater calls").is_empty());
    assert!(voice.calls.lock().expect("voice calls").is_empty());
}

#[tokio::test]
async fn desktop_authority_reconciles_cold_start_archive_and_forget_roots() {
    let temporary = TempDir::new().expect("temporary desktop graph");
    let state = temporary.path().join("state");
    let home = temporary.path().join("home");
    let default = home.join("Desktop/Projects");
    let active = temporary.path().join("active-session");
    let archived_live = temporary.path().join("archived-live-session");
    let picked = temporary.path().join("picked-workspace");
    fs::create_dir_all(&default).expect("default directory");
    fs::create_dir(&active).expect("active session directory");
    fs::create_dir(&archived_live).expect("archived live session directory");
    fs::create_dir(&picked).expect("picked directory");
    fs::write(active.join("active.txt"), "active").expect("active file");
    fs::write(archived_live.join("live.txt"), "still live").expect("archived live file");
    fs::write(picked.join("picked.txt"), "picked").expect("picked file");

    let service = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp-profiles.json"),
                terminal_workspaces_file: state.join("terminal-workspaces.json"),
                home_directory: home,
                customization_user_root: None,
                review_checkpoint_store_path: Some(state.join("review-checkpoints")),
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("fresh desktop service"),
    );
    assert!(
        service.workspace_roots().is_empty(),
        "cold start has no picker/default grants"
    );

    let session_roots = Arc::new(MutableSessionRoots::default());
    session_roots.replace(vec![active.clone(), archived_live.clone()]);
    let dispatcher = DesktopHostDispatcher::new(service.clone())
        .with_session_root_provider(session_roots.clone());

    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.read_file,
                json!({"path": active.join("active.txt").to_string_lossy()}),
            )
            .await
            .expect("cold-start session cwd is authorized"),
        "active"
    );
    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.read_file,
                json!({"path": archived_live.join("live.txt").to_string_lossy()}),
            )
            .await
            .expect("archived live handle keeps its cwd authorized"),
        "still live"
    );
    // Harness commands (Context, Presets, Capture, RunChecks) read the same
    // session roots; without them a restored session's folder was refused.
    let presets = dispatcher
        .dispatch(
            CHANNELS.harness_control,
            json!({"kind": "presets", "root": active.to_string_lossy()}),
        )
        .await
        .expect("harness presets for a session root");
    assert_eq!(presets["presets"][0]["id"], "inspect");

    // Once the archived row is hidden and its live handle ends, the
    // read-only provider drops it and the next request narrows authority.
    session_roots.replace(vec![active.clone()]);
    let archived_error = dispatcher
        .dispatch(
            CHANNELS.read_file,
            json!({"path": archived_live.join("live.txt").to_string_lossy()}),
        )
        .await
        .expect_err("ended archived session root is revoked");
    assert_eq!(archived_error.code, "request-refused");

    // Forget removes the remaining visible row. It must not retain a
    // monotonic session grant from the successful read above.
    session_roots.replace(Vec::new());
    let forgotten_error = dispatcher
        .dispatch(
            CHANNELS.read_file,
            json!({"path": active.join("active.txt").to_string_lossy()}),
        )
        .await
        .expect_err("forgotten session root is revoked");
    assert_eq!(forgotten_error.code, "request-refused");

    dispatcher
        .dispatch(
            CHANNELS.pick_directory,
            json!({"selectedPath": picked.to_string_lossy()}),
        )
        .await
        .expect("explicit picker grant");
    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.read_file,
                json!({"path": picked.join("picked.txt").to_string_lossy()}),
            )
            .await
            .expect("picker grant survives session reconciliation"),
        "picked"
    );
    assert_eq!(service.workspace_roots().len(), 1);
}

#[test]
fn desktop_failures_carry_conservative_replay_certainty() {
    let invalid = DesktopServiceError::new("invalid-payload", "bad payload", false);
    assert!(desktop_replay_is_safe("iface:saveFile", &invalid));
    assert!(desktop_replay_is_safe("iface:createProject", &invalid));

    let mut uncertain = DesktopServiceError::new(
        "request-refused",
        "Save outcome is uncertain. Interface may have replaced the file.",
        false,
    )
    .outcome_unknown();
    assert!(!desktop_replay_is_safe("iface:saveFile", &uncertain));
    uncertain.message = "Replacement copy without any safety keywords.".into();
    assert!(!desktop_replay_is_safe("iface:saveFile", &uncertain));
    assert!(!desktop_replay_is_safe("iface:readFile", &uncertain));
    assert!(!desktop_replay_is_safe("iface:createProject", &uncertain));
    assert!(!desktop_replay_is_safe(
        "iface:createProject",
        &DesktopServiceError::new(
            "project-create-failed",
            "The folder may have been created.",
            false,
        )
    ));

    let settings_write = DesktopServiceError::new(
        "desktop-state-write-failed",
        "could not prove the settings write",
        false,
    );
    assert!(!desktop_replay_is_safe(
        "iface:setAcpProfiles",
        &settings_write
    ));
}

#[test]
fn desktop_replay_is_safe_for_appearance_unavailable() {
    let error = DesktopServiceError::new(
        "appearance-unavailable",
        "The appearance file is unavailable; the theme was not saved.",
        false,
    );
    assert!(desktop_replay_is_safe("iface:setAppearance", &error));
}

#[tokio::test]
async fn away_has_one_explicit_owner_and_fails_closed_when_unavailable() {
    let (router, ..) = router();
    assert!(
        router
            .dispatch(CHANNELS.project_away, Value::Null)
            .await
            .is_err()
    );
    let owner = Arc::new(RecordingDispatcher::new("away"));
    let router = router.with_away(owner.clone());
    assert_eq!(
        router
            .dispatch(CHANNELS.project_away, Value::Null)
            .await
            .unwrap(),
        "away"
    );
    assert_eq!(
        owner.calls.lock().unwrap().as_slice(),
        &[CHANNELS.project_away]
    );
}

#[tokio::test]
async fn diagnostics_have_one_explicit_owner() {
    let (router, _, _, desktop, ..) = router();
    let error = router
        .dispatch(CHANNELS.diagnostics, json!({"kind": "notices"}))
        .await
        .expect_err("no diagnostics owner yet");
    assert_eq!(error.code, OPERATION_NOT_MIGRATED_CODE);
    assert!(error.replay_safe);
    let temporary = TempDir::new().expect("temporary");
    let router = router.with_diagnostics(Arc::new(crate::diagnostics::DiagnosticsHost::new(
        crate::diagnostics::DiagnosticsHostConfig {
            data_root: temporary.path().to_path_buf(),
            home: temporary.path().to_path_buf(),
            product_version: "0.0.0".into(),
            app_version: None,
            probes: Arc::new(NoProbes),
            notices: Arc::default(),
            revealer: Arc::new(crate::diagnostics::FinderRevealer),
        },
    )));
    assert_eq!(
        router
            .dispatch(CHANNELS.diagnostics, json!({"kind": "notices"}))
            .await
            .expect("notices"),
        json!({"notices": []})
    );
    assert!(desktop.calls.lock().expect("desktop calls").is_empty());
}

struct NoProbes;

#[async_trait::async_trait]
impl crate::diagnostics::ProviderProbes for NoProbes {
    async fn probes(&self) -> Vec<interface_contracts::ipc::EngineProbe> {
        Vec::new()
    }
}

#[tokio::test]
async fn threads_have_one_explicit_owner_ahead_of_the_desktop_dispatcher() {
    let (router, _, _, desktop, ..) = router();
    let error = router
        .dispatch(CHANNELS.thread_control, json!({"kind": "read"}))
        .await
        .expect_err("no thread owner yet");
    assert_eq!(error.code, OPERATION_NOT_MIGRATED_CODE);

    let (threads, _store) = crate::threads::tests::router_host();
    let router = router.with_threads(Arc::new(threads));
    let response: interface_contracts::threads::ThreadControlResponse = serde_json::from_value(
        router
            .dispatch(CHANNELS.thread_control, json!({"kind": "read"}))
            .await
            .expect("read answers"),
    )
    .expect("a thread control response");
    assert_eq!(response.view.version, 1);
    assert!(
        response.view.revision > 0,
        "a fresh store starts at its open time"
    );
    assert!(response.view.threads.is_empty());
    assert!(response.prepared.is_none() && response.key.is_none());
    let refused = router
        .dispatch(
            CHANNELS.thread_control,
            json!({"kind": "pin", "key": "codex#019a3c5e"}),
        )
        .await
        .expect_err("no such thread");
    assert_eq!(
        refused.code,
        interface_settings::threads::THREADS_UNKNOWN_CODE
    );
    assert!(refused.replay_safe);
    assert!(desktop.calls.lock().expect("desktop calls").is_empty());
    assert!(!desktop_replay_is_safe(
        CHANNELS.thread_control,
        &DesktopServiceError::new("invalid-payload", "bad payload", false)
    ));
}

#[derive(Default)]
struct RecordingWorktreeRelease {
    released: Mutex<Vec<String>>,
}

#[async_trait]
impl AutomaticWorktreeRelease for RecordingWorktreeRelease {
    fn schedule_session_release(&self, _session_key: String) {}

    fn schedule_workspace_release(&self, workspace: String) {
        self.released
            .lock()
            .expect("released workspaces")
            .push(workspace);
    }

    fn managed_workspace_root(&self, workspace: &std::path::Path) -> Option<PathBuf> {
        Some(workspace.to_path_buf())
    }
}

#[tokio::test]
async fn dropping_a_saved_terminal_schedules_its_worktree_release() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&state).expect("state directory");
    fs::create_dir(&checkout).expect("checkout directory");
    fs::create_dir(temporary.path().join("home")).expect("home directory");
    let service = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp-profiles.json"),
                terminal_workspaces_file: state.join("terminal-workspaces.json"),
                home_directory: temporary.path().join("home"),
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("desktop service"),
    );
    let release = Arc::new(RecordingWorktreeRelease::default());
    let dispatcher = DesktopHostDispatcher::new(service).with_worktree_release(release.clone());
    let saved = |terminals: serde_json::Value| {
        json!({"workspaces": [{
            "id": "workspace",
            "name": "Workspace",
            "createdAt": "2026-09-23T00:00:00Z",
            "updatedAt": "2026-09-23T00:00:00Z",
            "terminals": terminals
        }]})
    };

    dispatcher
        .dispatch(
            CHANNELS.set_terminal_workspaces,
            saved(json!([{"id": uuid::Uuid::new_v4(), "title": "Shell", "cwd": checkout}])),
        )
        .await
        .expect("save a terminal in the checkout");
    assert!(release.released.lock().expect("released").is_empty());

    dispatcher
        .dispatch(CHANNELS.set_terminal_workspaces, saved(json!([])))
        .await
        .expect("drop the terminal");
    assert_eq!(
        *release.released.lock().expect("released"),
        [checkout.to_string_lossy().into_owned()]
    );
}
