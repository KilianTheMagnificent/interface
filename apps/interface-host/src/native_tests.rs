use std::sync::Mutex;

use interface_contracts::engine::{EngineId, SessionStatus};
use interface_desktop_services::{DesktopServiceConfig, DesktopServiceReviewPorts};
use interface_registry::RemoveResult;
use interface_terminal::TerminalBackendError;
use serde_json::json;

use super::*;

#[derive(Default)]
struct FakeRuntime {
    active: Mutex<BTreeSet<String>>,
    roots: Mutex<Vec<PathBuf>>,
    sessions: Mutex<Vec<SessionSummary>>,
    restored: Mutex<Vec<(String, Worktree)>>,
}

impl NativeRuntimeView for FakeRuntime {
    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError> {
        Ok(self
            .active
            .lock()
            .expect("active session lock")
            .contains(session_key))
    }

    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        Ok(self.roots.lock().expect("session root lock").clone())
    }

    fn restore_worktree_home(
        &self,
        requested_key: &str,
        worktree: &Worktree,
    ) -> Result<bool, RuntimeError> {
        self.restored
            .lock()
            .expect("restored worktree lock")
            .push((requested_key.to_owned(), worktree.clone()));
        let checkout = worktree.session_cwd.to_string_lossy();
        let home = worktree.source_cwd.to_string_lossy().into_owned();
        let mut sessions = self.sessions.lock().expect("session summary lock");
        let Some(summary) = sessions.iter_mut().find(|summary| {
            session_key(&summary.engine, &summary.session_id) == requested_key
                && summary.cwd == checkout
        }) else {
            return Ok(false);
        };
        summary.cwd = home;
        Ok(true)
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        Ok(self.sessions.lock().expect("session summary lock").clone())
    }
}

struct FakeWorktrees {
    records: Mutex<Vec<ManagedWorktreeRecord>>,
    verified: bool,
    removal: Mutex<RemoveResult>,
}

#[async_trait]
impl ManagedWorktreeInventory for FakeWorktrees {
    fn records(&self) -> Result<Vec<ManagedWorktreeRecord>, WorktreeServiceError> {
        Ok(self.records.lock().expect("worktree records").clone())
    }

    async fn verify(
        &self,
        _record: ManagedWorktreeRecord,
    ) -> Result<ManagedWorktreeRegistration, WorktreeServiceError> {
        Ok(if self.verified {
            ManagedWorktreeRegistration::Active
        } else {
            ManagedWorktreeRegistration::Absent
        })
    }

    async fn remove(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<RemoveResult, WorktreeServiceError> {
        let result = self.removal.lock().expect("worktree removal").clone();
        if result.removed {
            self.records
                .lock()
                .expect("worktree records")
                .retain(|candidate| candidate != &record);
        }
        Ok(result)
    }
}

#[derive(Default)]
struct RecordingTerminal {
    ghostty: bool,
    launches: Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl ExternalTerminalBackend for RecordingTerminal {
    fn ghostty_installed(&self) -> bool {
        self.ghostty
    }

    fn spawn_detached(
        &self,
        executable: &Path,
        args: &[String],
    ) -> Result<(), TerminalBackendError> {
        self.launches
            .lock()
            .expect("terminal launch lock")
            .push((executable.to_path_buf(), args.to_vec()));
        Ok(())
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    dispatcher: NativeHostDispatcher,
    runtime: Arc<FakeRuntime>,
    worktrees: Arc<FakeWorktrees>,
    terminal: Arc<RecordingTerminal>,
    desktop: Arc<DesktopService>,
    record: ManagedWorktreeRecord,
}

fn fixture() -> Fixture {
    let temporary = tempfile::tempdir().expect("temporary host state");
    let state = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    let source = temporary.path().join("repo");
    let checkout = temporary.path().join("checkout");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory");
    std::fs::create_dir_all(source.join("src")).expect("source cwd");
    std::fs::create_dir_all(checkout.join("src")).expect("checkout cwd");
    let desktop = Arc::new(
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
        .expect("desktop service"),
    );
    let record = ManagedWorktreeRecord {
        key: "codex#session-1".into(),
        path: checkout.clone(),
        session_cwd: checkout.join("src"),
        branch: "interface/session-1".into(),
        repo_root: source.clone(),
        source_cwd: source.join("src"),
        dirty_source: false,
    };
    let runtime = Arc::new(FakeRuntime {
        active: Mutex::new(BTreeSet::new()),
        roots: Mutex::new(Vec::new()),
        sessions: Mutex::new(vec![summary(&record)]),
        restored: Mutex::new(Vec::new()),
    });
    let worktrees = Arc::new(FakeWorktrees {
        records: Mutex::new(vec![record.clone()]),
        verified: true,
        removal: Mutex::new(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        }),
    });
    let terminal = Arc::new(RecordingTerminal::default());
    let dispatcher = NativeHostDispatcher::new(
        runtime.clone(),
        desktop.clone(),
        worktrees.clone(),
        terminal.clone(),
    );
    Fixture {
        _temporary: temporary,
        dispatcher,
        runtime,
        worktrees,
        terminal,
        desktop,
        record,
    }
}

fn summary(record: &ManagedWorktreeRecord) -> SessionSummary {
    SessionSummary {
        engine: EngineId::new("codex").expect("engine id"),
        session_id: "session-1".into(),
        title: "Session 1".into(),
        cwd: record.session_cwd.to_string_lossy().into_owned(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: "2026-08-27T00:00:00.000Z".into(),
        first_seen: "2026-08-27T00:00:00.000Z".into(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: None,
        project_root: Some(record.repo_root.to_string_lossy().into_owned()),
        worktree_home: Some(record.source_cwd.to_string_lossy().into_owned()),
        worktree_path: Some(record.session_cwd.to_string_lossy().into_owned()),
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

#[tokio::test]
async fn terminal_opens_only_an_exact_approved_root_with_direct_argv() {
    let fixture = fixture();
    fixture
        .runtime
        .roots
        .lock()
        .expect("session root lock")
        .push(fixture.record.repo_root.clone());
    let root = std::fs::canonicalize(&fixture.record.repo_root)
        .expect("canonical session root")
        .to_string_lossy()
        .into_owned();

    let opened = fixture
        .dispatcher
        .dispatch(CHANNELS.open_terminal, json!({"root": root}))
        .await
        .expect("terminal opened");
    assert_eq!(opened["root"], root);
    assert_eq!(opened["app"], "Terminal");
    assert_eq!(
        *fixture.terminal.launches.lock().expect("terminal launches"),
        vec![(
            PathBuf::from("/usr/bin/open"),
            vec!["-a".into(), "Terminal".into(), root.clone()]
        )]
    );

    let nested = fixture
        .dispatcher
        .dispatch(
            CHANNELS.open_terminal,
            json!({"root": std::fs::canonicalize(&fixture.record.source_cwd)
                .expect("canonical nested path")}),
        )
        .await
        .expect_err("nested path is not an exact approved root");
    assert_eq!(nested.code, "request-refused");
    assert!(
        nested
            .message
            .contains("exact current session or workspace root")
    );
    assert_eq!(
        fixture
            .terminal
            .launches
            .lock()
            .expect("terminal launches")
            .len(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn explicit_worktree_ops_share_inventory_obey_editor_guard_and_push_home() {
    let mut fixture = fixture();
    let (push_sender, mut pushes) = mpsc::channel(4);
    fixture.dispatcher = fixture.dispatcher.with_pushes(push_sender);

    let opened = fixture
        .dispatcher
        .dispatch(
            CHANNELS.open_managed_worktree,
            json!({"sessionKey": fixture.record.key}),
        )
        .await
        .expect("opened managed worktree");
    assert_eq!(opened["key"], fixture.record.key);
    assert_eq!(
        opened["sessionCwd"],
        fixture.record.session_cwd.to_string_lossy().as_ref()
    );
    assert_eq!(opened["state"], "retained");
    assert!(
        fixture
            .desktop
            .workspace_roots()
            .contains(&std::fs::canonicalize(&fixture.record.path).expect("canonical checkout"))
    );

    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root": fixture.record.path, "open": true}),
        )
        .expect("protect editor workspace");
    let kept = fixture
        .dispatcher
        .dispatch(
            CHANNELS.cleanup_managed_worktree,
            json!({"sessionKey": fixture.record.key}),
        )
        .await
        .expect("cleanup kept open worktree");
    assert_eq!(
        kept,
        json!({
            "worktree": opened,
            "removed": false,
            "kept": "open",
            "detail": "Close files from this checkout before cleaning it up."
        })
    );
    assert_eq!(
        fixture
            .worktrees
            .records
            .lock()
            .expect("worktree records")
            .len(),
        1
    );

    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root": fixture.record.path, "open": false}),
        )
        .expect("release editor workspace");
    let removed = fixture
        .dispatcher
        .dispatch(
            CHANNELS.cleanup_managed_worktree,
            json!({"sessionKey": fixture.record.key}),
        )
        .await
        .expect("cleanup removed worktree");
    assert_eq!(removed["removed"], true);
    assert!(removed.get("kept").is_none());
    assert!(
        fixture
            .worktrees
            .records
            .lock()
            .expect("worktree records")
            .is_empty()
    );
    assert_eq!(
        fixture
            .runtime
            .restored
            .lock()
            .expect("restored worktree")
            .len(),
        1
    );
    let push = pushes.recv().await.expect("registry push");
    assert_eq!(push.operation, CHANNELS.registry);
    assert_eq!(
        push.payload[0]["cwd"],
        fixture.record.source_cwd.to_string_lossy().as_ref()
    );
    assert!(
        !fixture.desktop.workspace_roots().contains(
            &std::fs::canonicalize(&fixture.record.path)
                .unwrap_or_else(|_| { fixture.record.path.clone() })
        )
    );
}

#[tokio::test]
async fn cleanup_sends_its_registry_through_the_shared_coalescer() {
    let mut fixture = fixture();
    let (push_sender, mut pushes) = mpsc::channel(4);
    let runtime = Arc::clone(&fixture.runtime);
    let shared = SharedRegistryPushes(crate::registry_push::RegistryPushCoordinator::start(
        move || crate::registry_push::registry_push_round(runtime.visible_sessions().map(Some)),
        push_sender,
    ));
    fixture.dispatcher = fixture.dispatcher.with_shared_registry_pushes(shared);
    let removed = fixture
        .dispatcher
        .dispatch(
            CHANNELS.cleanup_managed_worktree,
            json!({"sessionKey": fixture.record.key}),
        )
        .await
        .expect("cleanup removed worktree");
    assert_eq!(removed["removed"], true);
    // The flush resolved before the response, so the frame is already queued.
    let push = pushes.try_recv().expect("registry push");
    assert_eq!(push.operation, CHANNELS.registry);
    assert_eq!(
        push.payload[0]["cwd"],
        fixture.record.source_cwd.to_string_lossy().as_ref()
    );
}

#[tokio::test]
async fn cleanup_removes_deleted_checkout_without_verifying_missing_directory() {
    let mut fixture = fixture();
    // Simulate a checkout folder the owner deleted after moving ignored files:
    // `verify` would fail to canonicalize the missing checkout (`verified: false`).
    std::fs::remove_dir_all(&fixture.record.path).expect("delete checkout folder");
    let worktrees = Arc::new(FakeWorktrees {
        records: Mutex::new(vec![fixture.record.clone()]),
        verified: false,
        removal: Mutex::new(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        }),
    });
    fixture.dispatcher = NativeHostDispatcher::new(
        fixture.runtime.clone(),
        fixture.desktop.clone(),
        worktrees.clone(),
        fixture.terminal.clone(),
    );

    let removed = fixture
        .dispatcher
        .dispatch(
            CHANNELS.cleanup_managed_worktree,
            json!({"sessionKey": fixture.record.key}),
        )
        .await
        .expect("cleanup removed deleted checkout");
    assert_eq!(removed["removed"], true);
    assert!(removed.get("kept").is_none());
    assert!(
        worktrees
            .records
            .lock()
            .expect("worktree records")
            .is_empty()
    );
    assert_eq!(
        fixture
            .runtime
            .restored
            .lock()
            .expect("restored worktree")
            .as_slice(),
        &[(fixture.record.key.clone(), fixture.record.worktree())]
    );
}

#[test]
fn slash_command_session_cwd_is_not_a_terminal_root() {
    assert_eq!(terminal_session_root_from_cwd("/"), None);
    assert_eq!(terminal_session_root_from_cwd(""), None);
    assert_eq!(
        terminal_session_root_from_cwd("/Users/me/project"),
        Some(PathBuf::from("/Users/me/project"))
    );
}

#[test]
fn approved_session_roots_list_each_project_once() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("temporary runtime root");
    let private = |path: &Path| {
        std::fs::create_dir(path).expect("runtime directory");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("private runtime directory");
    };
    let runtime_root = temporary.path().join("runtime");
    private(&runtime_root);
    let history_root = runtime_root.join("history");
    private(&history_root);
    let runtime = SessionRuntime::builder(interface_runtime::RuntimeServices::new(
        interface_registry::SessionRegistry::open(&runtime_root).expect("registry"),
        interface_runtime::EngineHistoryStore::open(
            &history_root,
            interface_runtime::HistoryLimits::default(),
        )
        .expect("history"),
    ))
    .build()
    .expect("runtime");
    let engine = EngineId::new("claude").expect("engine");
    let session = |id: &str, cwd: &str, archived: bool| SessionSummary {
        engine: engine.clone(),
        session_id: id.to_owned(),
        title: id.to_owned(),
        cwd: cwd.to_owned(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: "2026-09-23T12:00:00.000Z".to_owned(),
        first_seen: "2026-09-23T12:00:00.000Z".to_owned(),
        archived,
        size: None,
        title_from_summary: None,
        origin: None,
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    };
    {
        let registry = runtime.services().session_registry();
        let mut registry = registry.lock().expect("registry lock");
        let revision = registry.revision();
        registry
            .upsert_all(
                revision,
                &engine,
                [
                    session("one", "/tmp/project-b", false),
                    session("two", "/tmp/project-a", false),
                    session("three", "/tmp/project-b", false),
                    session("four", "/tmp/archived", true),
                    session("five", "", false),
                ],
            )
            .expect("sessions");
    }

    assert_eq!(
        NativeRuntimeView::approved_session_roots(&runtime).expect("roots"),
        [
            PathBuf::from("/tmp/project-a"),
            PathBuf::from("/tmp/project-b")
        ],
        "each distinct live root once; archived sessions only while live"
    );
}
