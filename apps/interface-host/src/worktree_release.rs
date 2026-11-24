use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use interface_contracts::engine::SessionSummary;
use interface_desktop_services::DesktopService;
use interface_registry::{ManagedWorktreeRecord, ManagedWorktreeRegistration, RemoveKept};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    collaboration::CollaborationSessionHooks,
    dispatcher::{
        AutomaticWorktreeRelease, ManagedWorkspaceVerification, VerifiedManagedWorkspace,
        settled_session_activity,
    },
    native::NativeRuntimeView,
    protocol::PushFrame,
    registry_push::{SharedRegistryPushes, registry_push_frame},
    worktree::{ManagedWorktreeInventory, WorktreeServiceError},
};

const WORKTREE_PUSH: &str = "iface:worktree";

/// Process-owned automatic release path for frozen managed worktrees.
///
/// Terminal session pumps, forget, and the final editor-close edge all feed
/// this same owner. Per-key gates make duplicate lifecycle requests harmless;
/// shutdown drains every spawned mutation before the ledger is released.
#[derive(Clone)]
pub struct AutomaticWorktreeReleaseService {
    inner: Arc<AutomaticWorktreeReleaseInner>,
    runtime_handle: tokio::runtime::Handle,
}

struct AutomaticWorktreeReleaseInner {
    runtime: Arc<dyn NativeRuntimeView>,
    desktop: Arc<DesktopService>,
    worktrees: Arc<dyn ManagedWorktreeInventory>,
    pushes: mpsc::Sender<PushFrame>,
    /// The host's shared registry coalescer; without it the frame is built
    /// and sent here.
    registry_pushes: std::sync::OnceLock<SharedRegistryPushes>,
    collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
    key_gates: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl AutomaticWorktreeReleaseService {
    /// # Panics
    ///
    /// Panics when constructed outside a Tokio runtime. Releases are scheduled
    /// from plain OS threads (terminal lease drops), so the handle has to be
    /// captured here rather than resolved at schedule time.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn NativeRuntimeView>,
        desktop: Arc<DesktopService>,
        worktrees: Arc<dyn ManagedWorktreeInventory>,
        pushes: mpsc::Sender<PushFrame>,
        collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
    ) -> Self {
        Self {
            inner: Arc::new(AutomaticWorktreeReleaseInner {
                runtime,
                desktop,
                worktrees,
                pushes,
                registry_pushes: std::sync::OnceLock::new(),
                collaboration,
                key_gates: Mutex::new(BTreeMap::new()),
                tasks: Mutex::new(Vec::new()),
            }),
            runtime_handle: tokio::runtime::Handle::current(),
        }
    }

    /// Sends post-release registry frames through the host's shared
    /// coalescer. Set once, before any release is scheduled.
    #[must_use]
    pub fn with_shared_registry_pushes(self, shared: SharedRegistryPushes) -> Self {
        let _ = self.inner.registry_pushes.set(shared);
        self
    }

    fn schedule_key(&self, session_key: String) {
        let inner = Arc::clone(&self.inner);
        self.track(self.runtime_handle.spawn(async move {
            inner.release(session_key).await;
        }));
    }

    /// Keeps a scheduled release where [`Self::close`] drains it.
    fn track(&self, task: JoinHandle<()>) {
        match self.inner.tasks.lock() {
            Ok(mut tasks) => {
                tasks.retain(|task| !task.is_finished());
                tasks.push(task);
            }
            Err(_) => task.abort(),
        }
    }

    /// Drains every automatic release already scheduled, including work that
    /// is queued while an earlier release is still completing.
    pub async fn close(&self) {
        loop {
            let tasks = match self.inner.tasks.lock() {
                Ok(mut tasks) => std::mem::take(&mut *tasks),
                Err(_) => return,
            };
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }
}

#[async_trait::async_trait]
impl AutomaticWorktreeRelease for AutomaticWorktreeReleaseService {
    fn schedule_session_release(&self, session_key: String) {
        self.schedule_key(session_key);
    }

    fn schedule_workspace_release(&self, workspace: String) {
        // Callers are lease drops and desktop dispatches; the lookup behind
        // the release belongs to the tracked task, not to them.
        let inner = Arc::clone(&self.inner);
        self.track(self.runtime_handle.spawn(async move {
            match inner.lookup_workspace(PathBuf::from(workspace)).await {
                Ok(ManagedWorkspaceLookup::Verified(record, _)) => {
                    inner.release(record.key).await;
                }
                Ok(ManagedWorkspaceLookup::NotManaged | ManagedWorkspaceLookup::Unavailable) => {}
                Err(error) => {
                    tracing::warn!(%error.message, "could not inspect managed worktrees after editor close");
                }
            }
        }));
    }

    fn managed_workspace_root(&self, workspace: &Path) -> Option<std::path::PathBuf> {
        let records = self.inner.worktrees.records().ok()?;
        match managed_record_for_workspace(&records, workspace) {
            ManagedWorkspaceLookup::Verified(_, root) => Some(root),
            ManagedWorkspaceLookup::NotManaged | ManagedWorkspaceLookup::Unavailable => None,
        }
    }

    async fn verify_managed_workspace(
        &self,
        workspace: &Path,
        expected_key: Option<&str>,
    ) -> ManagedWorkspaceVerification {
        let (record, root) = match self.inner.lookup_workspace(workspace.to_path_buf()).await {
            Ok(ManagedWorkspaceLookup::Verified(record, root)) => (record, root),
            Ok(ManagedWorkspaceLookup::NotManaged) => {
                return ManagedWorkspaceVerification::NotManaged;
            }
            Ok(ManagedWorkspaceLookup::Unavailable) | Err(_) => {
                return ManagedWorkspaceVerification::Unavailable;
            }
        };
        if expected_key.is_some_and(|expected| record.key != expected) {
            return ManagedWorkspaceVerification::NotManaged;
        }
        match self.inner.worktrees.verify(record.clone()).await {
            Ok(ManagedWorktreeRegistration::Active) => {}
            Ok(ManagedWorktreeRegistration::Absent | ManagedWorktreeRegistration::Unknown) => {
                // A matching durable record remains managed authority even
                // when Git can no longer prove it. Never downgrade that path
                // to keyless ordinary-workspace authority.
                return ManagedWorkspaceVerification::Unavailable;
            }
            Err(_) => return ManagedWorkspaceVerification::Unavailable,
        }
        let (current_record, current_root) =
            match self.inner.lookup_workspace(workspace.to_path_buf()).await {
                Ok(ManagedWorkspaceLookup::Verified(record, root)) => (record, root),
                Ok(ManagedWorkspaceLookup::NotManaged | ManagedWorkspaceLookup::Unavailable)
                | Err(_) => {
                    return ManagedWorkspaceVerification::Unavailable;
                }
            };
        if current_record != record || current_root != root {
            return ManagedWorkspaceVerification::Unavailable;
        }
        ManagedWorkspaceVerification::Verified(VerifiedManagedWorkspace {
            root,
            key: record.key,
        })
    }
}

enum ManagedWorkspaceLookup {
    Verified(ManagedWorktreeRecord, std::path::PathBuf),
    NotManaged,
    Unavailable,
}

fn managed_record_for_workspace(
    records: &[ManagedWorktreeRecord],
    workspace: &Path,
) -> ManagedWorkspaceLookup {
    let Ok(workspace) = std::fs::canonicalize(workspace) else {
        return ManagedWorkspaceLookup::Unavailable;
    };
    let mut matched = None;
    for record in records {
        let root = match std::fs::canonicalize(&record.path) {
            Ok(root) => root,
            Err(_) if workspace.starts_with(&record.path) => {
                return ManagedWorkspaceLookup::Unavailable;
            }
            Err(_) => continue,
        };
        if workspace != root && !workspace.starts_with(&root) {
            continue;
        }
        let Ok(session_cwd) = std::fs::canonicalize(&record.session_cwd) else {
            return ManagedWorkspaceLookup::Unavailable;
        };
        if !session_cwd.starts_with(&root) {
            return ManagedWorkspaceLookup::Unavailable;
        }
        if matched.is_some() {
            return ManagedWorkspaceLookup::Unavailable;
        }
        matched = Some((record.clone(), root));
    }
    matched.map_or(ManagedWorkspaceLookup::NotManaged, |(record, root)| {
        ManagedWorkspaceLookup::Verified(record, root)
    })
}

impl AutomaticWorktreeReleaseInner {
    /// Reads the ledger and finds the record owning `workspace` on the
    /// blocking pool: the lookup canonicalizes the workspace and every
    /// ledger record's paths. A failed worker reads as unavailable.
    async fn lookup_workspace(
        &self,
        workspace: PathBuf,
    ) -> Result<ManagedWorkspaceLookup, WorktreeServiceError> {
        let worktrees = Arc::clone(&self.worktrees);
        tokio::task::spawn_blocking(move || {
            let records = worktrees.records()?;
            Ok(managed_record_for_workspace(&records, &workspace))
        })
        .await
        .unwrap_or(Ok(ManagedWorkspaceLookup::Unavailable))
    }

    async fn release(&self, session_key: String) {
        let gate = match self.key_gates.lock() {
            Ok(mut gates) => Arc::clone(
                gates
                    .entry(session_key.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            ),
            Err(_) => return,
        };
        {
            let _key = gate.lock().await;
            self.release_inner(&session_key).await;
        }
        drop(gate);
        if let Ok(mut gates) = self.key_gates.lock() {
            gates.retain(|_, held| Arc::strong_count(held) > 1);
        }
    }

    async fn release_inner(&self, session_key: &str) {
        let record = match self.worktrees.records() {
            Ok(records) => records.into_iter().find(|record| record.key == session_key),
            Err(error) => {
                tracing::warn!(%error.message, "could not read managed worktree inventory for release");
                return;
            }
        };
        let Some(record) = record else {
            return;
        };
        if !matches!(
            settled_session_activity(
                || self.runtime.session_is_active(session_key).map(Some),
                session_key,
                "could not prove that a worktree session is inactive",
            )
            .await,
            Some(false)
        ) {
            return;
        }
        if self.desktop.workspace_is_open(&record.path) {
            self.emit_notice(
                &record,
                "kept",
                Some(format!(
                    "Interface kept {} because a file from it is still open in the editor. Close those tabs before removing the worktree.",
                    record.path.display()
                )),
            )
            .await;
            return;
        }
        let _removal = match self.desktop.reserve_workspace_removal(&record.path) {
            Ok(Some(reservation)) => reservation,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    code = %error.code,
                    path = %record.path.display(),
                    "could not reserve managed worktree for safe release"
                );
                return;
            }
        };
        // The reservation stays held through a kept notice and, after a
        // removal, through the whole redirect below.
        if let Err(detail) = self.try_remove(&record).await {
            self.emit_notice(&record, "kept", Some(detail)).await;
            return;
        }

        self.desktop.revoke_workspace_root(&record.path);
        let changed = match self
            .runtime
            .restore_worktree_home(&record.key, &record.worktree())
        {
            Ok(changed) => changed,
            Err(error) => {
                tracing::error!(session_key, %error, "removed worktree session could not be redirected home");
                false
            }
        };
        if changed
            && let Some(collaboration) = &self.collaboration
            && let Err(error) = collaboration.observe_registry_change(&record.key)
        {
            tracing::error!(code = %error.code, "worktree-home collaboration projection failed");
        }
        let home = if record.source_cwd.as_os_str().is_empty() {
            &record.repo_root
        } else {
            &record.source_cwd
        };
        self.emit_notice(
            &record,
            "removed",
            Some(format!(
                "Anything the agent committed is on the branch {}, which was not deleted. The session now points at {}.",
                record.branch,
                home.display()
            )),
        )
        .await;
        if changed {
            self.emit_registry_snapshot().await;
        }
    }

    /// Verifies and removes a reserved managed worktree, or returns why it
    /// was kept: Git could not verify it, no longer registers it, could not
    /// remove it, or found work in it.
    async fn try_remove(&self, record: &ManagedWorktreeRecord) -> Result<(), String> {
        if !std::fs::symlink_metadata(&record.path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            let registration = match self.worktrees.verify(record.clone()).await {
                Ok(registration) => registration,
                Err(error) => {
                    return Err(format!(
                        "Kept {}: Interface could not verify it ({}). Inspect the checkout before removing it.",
                        record.path.display(),
                        error.message
                    ));
                }
            };
            match registration {
                ManagedWorktreeRegistration::Active => {}
                ManagedWorktreeRegistration::Absent => {
                    return Err(
                        "Git no longer identifies this exact path and branch as the managed worktree."
                            .into(),
                    );
                }
                ManagedWorktreeRegistration::Unknown => {
                    return Err(format!(
                        "Kept {} because Git could not inspect its worktree registration.",
                        record.path.display()
                    ));
                }
            }
        }
        let removed = match self.worktrees.remove(record.clone()).await {
            Ok(removed) => removed,
            Err(error) => {
                return Err(format!(
                    "Kept {}: Interface could not remove it ({}). Inspect the checkout before removing it.",
                    record.path.display(),
                    error.message
                ));
            }
        };
        if !removed.removed {
            return Err(match removed.kept {
                Some(RemoveKept::Dirty) => format!(
                    "Kept {}: it has {} uncommitted change{}. Commit or discard that work, then remove the worktree from Session Library.",
                    record.path.display(),
                    removed.changes.unwrap_or_default(),
                    if removed.changes == Some(1) { "" } else { "s" }
                ),
                Some(RemoveKept::Ignored) => format!(
                    "Kept {}: it holds files git ignores (such as .env, node_modules or build output), which removing it would delete. Move what you need, then delete the folder yourself.",
                    record.path.display()
                ),
                Some(RemoveKept::Failed) | None => format!(
                    "Kept {}: Interface could not remove it{}. Inspect the checkout before removing it.",
                    record.path.display(),
                    removed
                        .detail
                        .as_deref()
                        .filter(|detail| !detail.is_empty())
                        .map_or_else(String::new, |detail| format!(" ({detail})"))
                ),
            });
        }
        Ok(())
    }

    async fn emit_notice(
        &self,
        record: &ManagedWorktreeRecord,
        state: &str,
        detail: Option<String>,
    ) {
        let mut payload = serde_json::json!({
            "key": record.key,
            "path": record.path,
            "branch": record.branch,
            "state": state,
        });
        if let Some(detail) = detail {
            payload["detail"] = serde_json::Value::String(detail);
        }
        let Ok(frame) = PushFrame::new(WORKTREE_PUSH, payload) else {
            return;
        };
        let _ = self.pushes.send(frame).await;
    }

    async fn emit_registry_snapshot(&self) {
        if let Some(registry_pushes) = self.registry_pushes.get() {
            let _ = registry_pushes.0.flush().await;
            return;
        }
        let sessions: Vec<SessionSummary> = match self.runtime.visible_sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::error!(%error, "could not read registry after automatic worktree release");
                return;
            }
        };
        let Ok(frame) = registry_push_frame(&sessions) else {
            return;
        };
        let _ = self.pushes.send(frame).await;
    }
}

#[cfg(test)]
#[path = "worktree_release_tests.rs"]
mod tests;
