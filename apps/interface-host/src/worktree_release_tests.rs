use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{EngineId, SessionStatus},
    ipc::CHANNELS,
};
use interface_desktop_services::{DesktopServiceConfig, DesktopServiceReviewPorts};
use interface_registry::{RemoveResult, Worktree};
use interface_runtime::RuntimeError;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::worktree::WorktreeServiceError;

struct FakeRuntime {
    active: Mutex<bool>,
    sessions: Mutex<Vec<SessionSummary>>,
    restored: Mutex<Vec<(String, Worktree)>>,
}

impl NativeRuntimeView for FakeRuntime {
    fn session_is_active(&self, _session_key: &str) -> Result<bool, RuntimeError> {
        Ok(*self.active.lock().expect("active lock"))
    }

    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        Ok(self
            .sessions
            .lock()
            .expect("sessions lock")
            .iter()
            .map(|session| PathBuf::from(&session.cwd))
            .collect())
    }

    fn restore_worktree_home(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<bool, RuntimeError> {
        self.restored
            .lock()
            .expect("restored lock")
            .push((session_key.to_owned(), worktree.clone()));
        let mut sessions = self.sessions.lock().expect("sessions lock");
        let Some(session) = sessions
            .iter_mut()
            .find(|session| format!("{}#{}", session.engine, session.session_id) == session_key)
        else {
            return Ok(false);
        };
        session.cwd = worktree.source_cwd.to_string_lossy().into_owned();
        Ok(true)
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        Ok(self.sessions.lock().expect("sessions lock").clone())
    }
}

struct FakeInventory {
    records: Mutex<Vec<ManagedWorktreeRecord>>,
    registration: Mutex<ManagedWorktreeRegistration>,
    removal: Mutex<RemoveResult>,
    removals: AtomicUsize,
}

#[async_trait]
impl ManagedWorktreeInventory for FakeInventory {
    fn records(&self) -> Result<Vec<ManagedWorktreeRecord>, WorktreeServiceError> {
        Ok(self.records.lock().expect("records lock").clone())
    }

    async fn verify(
        &self,
        _record: ManagedWorktreeRecord,
    ) -> Result<ManagedWorktreeRegistration, WorktreeServiceError> {
        Ok(*self.registration.lock().expect("registration lock"))
    }

    async fn remove(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<RemoveResult, WorktreeServiceError> {
        self.removals.fetch_add(1, Ordering::SeqCst);
        let result = self.removal.lock().expect("removal lock").clone();
        if result.removed {
            self.records
                .lock()
                .expect("records lock")
                .retain(|current| current != &record);
        }
        Ok(result)
    }
}

fn summary(cwd: &Path) -> SessionSummary {
    SessionSummary {
        engine: EngineId::new("codex").expect("engine"),
        session_id: "managed".into(),
        title: "Managed".into(),
        cwd: cwd.to_string_lossy().into_owned(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: "2026-08-27T00:00:00Z".into(),
        first_seen: "2026-08-27T00:00:00Z".into(),
        archived: false,
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
    }
}

type Fixture = (
    TempDir,
    Arc<DesktopService>,
    Arc<FakeRuntime>,
    Arc<FakeInventory>,
    AutomaticWorktreeReleaseService,
    mpsc::Receiver<PushFrame>,
    ManagedWorktreeRecord,
);

fn fixture(removal: RemoveResult) -> Fixture {
    let temporary = TempDir::new().expect("temporary root");
    let repo = temporary.path().join("repo");
    let checkout = repo.join(".interface-worktrees/managed");
    let session_cwd = checkout.join("src");
    let source = repo.join("src");
    std::fs::create_dir_all(&session_cwd).expect("checkout session cwd");
    std::fs::create_dir_all(&source).expect("source");
    let record = ManagedWorktreeRecord {
        key: "codex#managed".into(),
        path: checkout.clone(),
        session_cwd,
        branch: "iface/managed".into(),
        repo_root: repo,
        source_cwd: source,
        dirty_source: false,
    };
    let desktop = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: temporary.path().join("appearance.json"),
                acp_profiles_file: temporary.path().join("acp-profiles.json"),
                terminal_workspaces_file: temporary.path().join("terminal-workspaces.json"),
                home_directory: temporary.path().to_path_buf(),
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("desktop service"),
    );
    desktop
        .grant_workspace_root(&checkout)
        .expect("worktree grant");
    let runtime = Arc::new(FakeRuntime {
        active: Mutex::new(false),
        sessions: Mutex::new(vec![summary(&checkout)]),
        restored: Mutex::new(Vec::new()),
    });
    let inventory = Arc::new(FakeInventory {
        records: Mutex::new(vec![record.clone()]),
        registration: Mutex::new(ManagedWorktreeRegistration::Active),
        removal: Mutex::new(removal),
        removals: AtomicUsize::new(0),
    });
    let (pushes, receiver) = mpsc::channel(8);
    let runtime_view: Arc<dyn NativeRuntimeView> = runtime.clone();
    let worktrees: Arc<dyn ManagedWorktreeInventory> = inventory.clone();
    let service = AutomaticWorktreeReleaseService::new(
        runtime_view,
        desktop.clone(),
        worktrees,
        pushes,
        None,
    );
    (
        temporary, desktop, runtime, inventory, service, receiver, record,
    )
}

#[test]
fn unrelated_stale_ledger_record_does_not_block_workspace_classification() {
    let temporary = TempDir::new().expect("temporary root");
    let ordinary = temporary.path().join("ordinary");
    let active = temporary.path().join("active");
    let active_cwd = active.join("src");
    std::fs::create_dir(&ordinary).expect("ordinary root");
    std::fs::create_dir_all(&active_cwd).expect("active worktree");
    let records = vec![
        ManagedWorktreeRecord {
            key: "codex#stale".into(),
            path: temporary.path().join("missing-worktree"),
            session_cwd: temporary.path().join("missing-worktree/src"),
            branch: "iface/stale".into(),
            repo_root: temporary.path().join("repo"),
            source_cwd: temporary.path().join("repo/src"),
            dirty_source: false,
        },
        ManagedWorktreeRecord {
            key: "codex#active".into(),
            path: active,
            session_cwd: active_cwd,
            branch: "iface/active".into(),
            repo_root: temporary.path().join("repo"),
            source_cwd: temporary.path().join("repo/src"),
            dirty_source: false,
        },
    ];

    assert!(matches!(
        managed_record_for_workspace(&records, &ordinary),
        ManagedWorkspaceLookup::NotManaged
    ));
    assert!(matches!(
        managed_record_for_workspace(&records, &records[0].path),
        ManagedWorkspaceLookup::Unavailable
    ));
}

#[cfg(test)]
impl AutomaticWorktreeReleaseService {
    fn retained_key_gates(&self) -> usize {
        self.inner
            .key_gates
            .lock()
            .map(|gates| gates.len())
            .unwrap_or(0)
    }

    fn retained_tasks(&self) -> usize {
        self.inner
            .tasks
            .lock()
            .map(|tasks| tasks.len())
            .unwrap_or(0)
    }

    fn unfinished_tasks(&self) -> usize {
        self.inner
            .tasks
            .lock()
            .map(|tasks| tasks.iter().filter(|task| !task.is_finished()).count())
            .unwrap_or(0)
    }
}

#[tokio::test]
async fn terminal_release_removes_clean_checkout_redirects_home_and_announces_both_edges() {
    let (_temporary, desktop, runtime, inventory, service, mut pushes, record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });

    service.schedule_session_release(record.key.clone());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.restored.lock().expect("restored lock").as_slice(),
        &[(record.key.clone(), record.worktree())]
    );
    assert!(!desktop.workspace_roots().contains(&record.path));
    let notice = pushes.recv().await.expect("removed notice");
    assert_eq!(notice.operation, WORKTREE_PUSH);
    assert_eq!(notice.payload["state"], "removed");
    assert_eq!(
        pushes.recv().await.expect("home registry").operation,
        CHANNELS.registry
    );
}

#[tokio::test]
async fn nested_terminal_workspace_release_resolves_the_owning_managed_record() {
    let (_temporary, _desktop, _runtime, inventory, service, _pushes, record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });

    service.schedule_workspace_release(record.session_cwd.to_string_lossy().into_owned());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert!(inventory.records.lock().expect("records lock").is_empty());
}

#[tokio::test]
async fn queued_release_cannot_remove_a_worktree_referenced_by_a_saved_terminal() {
    let (_temporary, desktop, _runtime, inventory, service, _pushes, record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });
    desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved-worktree",
                    "name": "Saved worktree",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": uuid::Uuid::new_v4(),
                        "title": "Claude",
                        "cwd": record.session_cwd
                    }]
                }]
            }),
        )
        .expect("save terminal reference");

    service.schedule_session_release(record.key.clone());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 0);
    assert_eq!(
        inventory.records.lock().expect("records lock").as_slice(),
        &[record]
    );
}

#[tokio::test]
async fn open_editor_keeps_checkout_and_final_close_retries_the_same_safe_release() {
    let (_temporary, desktop, _runtime, inventory, service, mut pushes, record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });
    desktop
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root": record.path, "open": true}),
        )
        .expect("protect workspace");
    service.schedule_session_release(record.key.clone());
    service.close().await;
    assert_eq!(inventory.removals.load(Ordering::SeqCst), 0);
    assert_eq!(
        pushes.recv().await.expect("kept notice").payload["state"],
        "kept"
    );

    desktop
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root": record.path, "open": false}),
        )
        .expect("release workspace");
    service.schedule_workspace_release(record.path.to_string_lossy().into_owned());
    service.close().await;
    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert_eq!(
        pushes.recv().await.expect("removed notice").payload["state"],
        "removed"
    );
}

#[tokio::test]
async fn dirty_checkout_is_retained_with_its_durable_record_and_actionable_notice() {
    let (_temporary, _desktop, runtime, inventory, service, mut pushes, record) =
        fixture(RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Dirty),
            changes: Some(2),
            detail: None,
        });

    service.schedule_session_release(record.key.clone());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert_eq!(
        inventory.records.lock().expect("records lock").as_slice(),
        &[record]
    );
    assert!(runtime.restored.lock().expect("restored lock").is_empty());
    let notice = pushes.recv().await.expect("dirty kept notice");
    assert_eq!(notice.payload["state"], "kept");
    assert!(
        notice.payload["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("2 uncommitted changes"))
    );
    assert!(matches!(
        pushes.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn checkout_with_ignored_files_is_retained_once_with_a_notice_naming_them() {
    let (_temporary, _desktop, runtime, inventory, service, mut pushes, record) =
        fixture(RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Ignored),
            changes: None,
            detail: None,
        });

    service.schedule_session_release(record.key.clone());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert_eq!(
        inventory.records.lock().expect("records lock").as_slice(),
        &[record]
    );
    assert!(runtime.restored.lock().expect("restored lock").is_empty());
    let notice = pushes.recv().await.expect("ignored kept notice");
    assert_eq!(notice.payload["state"], "kept");
    assert!(
        notice.payload["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("files git ignores"))
    );
    assert!(matches!(
        pushes.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn deleted_checkout_is_released_even_when_verification_would_fail() {
    let (_temporary, _desktop, runtime, inventory, service, mut pushes, record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });
    std::fs::remove_dir_all(&record.path).expect("delete checkout folder");
    *inventory.registration.lock().expect("registration lock") =
        ManagedWorktreeRegistration::Unknown;

    service.schedule_session_release(record.key.clone());
    service.close().await;

    assert_eq!(inventory.removals.load(Ordering::SeqCst), 1);
    assert!(inventory.records.lock().expect("records lock").is_empty());
    assert_eq!(
        runtime.restored.lock().expect("restored lock").as_slice(),
        &[(record.key.clone(), record.worktree())]
    );
    assert_eq!(
        pushes.recv().await.expect("removed notice").payload["state"],
        "removed"
    );
}

#[tokio::test]
async fn finished_release_tasks_and_unheld_key_gates_are_pruned() {
    let (_temporary, _desktop, _runtime, _inventory, service, _pushes, _record) =
        fixture(RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        });

    service.schedule_session_release("missing".into());
    for _ in 0..10_000 {
        if service.unfinished_tasks() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(service.unfinished_tasks(), 0);
    assert_eq!(service.retained_tasks(), 1);

    service.schedule_session_release("missing-again".into());
    assert_eq!(service.retained_tasks(), 1);

    service.close().await;
    assert_eq!(service.retained_tasks(), 0);
    assert_eq!(service.retained_key_gates(), 0);
}
