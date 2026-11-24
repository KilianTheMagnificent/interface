use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{SessionSummary, session_key},
    ipc::{
        CHANNELS, ManagedWorktreeCleanup, ManagedWorktreeInfo, ManagedWorktreeKeptReason,
        ManagedWorktreeState,
    },
};
use interface_desktop_services::DesktopService;
use interface_registry::{
    ManagedWorktreeRecord, ManagedWorktreeRegistration, RemoveKept, Worktree,
};
use interface_runtime::{RuntimeError, RuntimeErrorKind, SessionRuntime};
use interface_terminal::{
    ApprovedTerminalRoots, ExternalTerminalBackend, TerminalError, open_external_terminal,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    collaboration::CollaborationSessionHooks,
    dispatcher::{
        DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE, OUTCOME_UNKNOWN_CODE,
        runtime_dispatch_error,
    },
    payload,
    protocol::PushFrame,
    registry_access,
    registry_push::{RegistryPushCoordinator, SharedRegistryPushes, registry_push_frame},
    worktree::{ManagedWorktreeInventory, WorktreeServiceError},
};

pub const NATIVE_SERVICE_OPERATIONS: [&str; 4] = [
    CHANNELS.open_terminal,
    CHANNELS.list_managed_worktrees,
    CHANNELS.open_managed_worktree,
    CHANNELS.cleanup_managed_worktree,
];

/// Narrow runtime projection used by native desktop services.
///
/// It deliberately exposes no provider command path. Terminal authority and
/// worktree cleanup only need durable registry/history state.
pub trait NativeRuntimeView: Send + Sync + 'static {
    /// Reports whether the provider effect may still own this session.
    ///
    /// # Errors
    ///
    /// Returns a durable-history access failure.
    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError>;

    /// Exact visible-session roots plus archived roots whose effect remains live.
    ///
    /// # Errors
    ///
    /// Returns a registry or durable-history access failure.
    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError>;

    /// Redirects a removed worktree session to its durable source directory.
    ///
    /// # Errors
    ///
    /// Returns a registry access or durability failure.
    fn restore_worktree_home(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<bool, RuntimeError>;

    /// Current legacy-visible registry snapshot for `iface:registry`.
    ///
    /// # Errors
    ///
    /// Returns a registry access failure.
    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError>;
}

impl NativeRuntimeView for SessionRuntime {
    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError> {
        self.session_is_live(session_key)
    }

    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        // Many sessions share a project, and every root is canonicalized per
        // pane open, so collect each distinct cwd once without cloning rows.
        let (mut roots, archived) = registry_access::read(self, |registry| {
            let mut roots = BTreeSet::new();
            let mut archived = Vec::new();
            for summary in registry
                .summaries()
                .filter(|summary| terminal_session_root_from_cwd(&summary.cwd).is_some())
            {
                if summary.archived {
                    archived.push((
                        session_key(&summary.engine, &summary.session_id),
                        summary.cwd.clone(),
                    ));
                } else {
                    roots.insert(summary.cwd.clone());
                }
            }
            (roots, archived)
        })?;
        // An archived session still grants its root while it is live.
        let live = self.live_session_keys()?;
        roots.extend(
            archived
                .into_iter()
                .filter(|(key, _)| live.contains(key))
                .map(|(_, cwd)| cwd),
        );
        Ok(roots.into_iter().map(PathBuf::from).collect())
    }

    fn restore_worktree_home(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<bool, RuntimeError> {
        let checkout_cwd = worktree.session_cwd.to_string_lossy();
        let home = if worktree.source_cwd.as_os_str().is_empty() {
            worktree.repo_root.to_string_lossy().into_owned()
        } else {
            worktree.source_cwd.to_string_lossy().into_owned()
        };
        registry_access::write(self, |registry, revision| {
            let Some(mut summary) = registry.get(session_key).cloned() else {
                return Ok(None);
            };
            if summary.cwd != checkout_cwd {
                return Ok(None);
            }
            summary.cwd = home;
            let engine = summary.engine.clone();
            registry
                .upsert_all(revision, &engine, [summary])
                .map(Some)
                .map_err(|error| {
                    RuntimeError::definitely_unsent(
                        RuntimeErrorKind::Registry,
                        error.to_string(),
                        false,
                    )
                })
        })
        .map(|written| written.is_some())
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        registry_access::sorted_visible(self)
    }
}

/// Owner for the phase-two native desktop slice.
pub struct NativeHostDispatcher {
    runtime: Arc<dyn NativeRuntimeView>,
    desktop: Arc<DesktopService>,
    worktrees: Arc<dyn ManagedWorktreeInventory>,
    terminal: Arc<dyn ExternalTerminalBackend>,
    pushes: Option<mpsc::Sender<PushFrame>>,
    registry_pushes: Option<RegistryPushCoordinator>,
    collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
}

impl NativeHostDispatcher {
    #[must_use]
    pub fn new(
        runtime: Arc<dyn NativeRuntimeView>,
        desktop: Arc<DesktopService>,
        worktrees: Arc<dyn ManagedWorktreeInventory>,
        terminal: Arc<dyn ExternalTerminalBackend>,
    ) -> Self {
        Self {
            runtime,
            desktop,
            worktrees,
            terminal,
            pushes: None,
            registry_pushes: None,
            collaboration: None,
        }
    }

    #[must_use]
    pub fn with_pushes(mut self, pushes: mpsc::Sender<PushFrame>) -> Self {
        self.pushes = Some(pushes);
        self
    }

    /// Sends the post-cleanup registry through the host's shared coalescer.
    #[must_use]
    pub fn with_shared_registry_pushes(mut self, shared: SharedRegistryPushes) -> Self {
        self.registry_pushes = Some(shared.0);
        self
    }

    #[must_use]
    pub fn with_collaboration_hooks(
        mut self,
        collaboration: Arc<dyn CollaborationSessionHooks>,
    ) -> Self {
        self.collaboration = Some(collaboration);
        self
    }

    async fn open_terminal(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<OpenTerminalPayload>(CHANNELS.open_terminal, payload)?;
        if request.root.is_empty() {
            return Err(refused(
                "Could not open a terminal.",
                "Choose an open workspace first.",
            ));
        }
        let desktop = Arc::clone(&self.desktop);
        let runtime = Arc::clone(&self.runtime);
        let terminal = Arc::clone(&self.terminal);
        tokio::task::spawn_blocking(move || {
            let mut candidates = desktop.terminal_workspace_roots();
            candidates.extend(
                runtime
                    .approved_session_roots()
                    .map_err(runtime_dispatch_error)?,
            );
            let approved = canonical_terminal_roots(candidates)?;
            let result =
                open_external_terminal(Path::new(&request.root), &approved, terminal.as_ref())
                    .map_err(terminal_dispatch_error)?;
            encode_result(result, false)
        })
        .await
        .map_err(|error| {
            DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                format!("The terminal authority worker failed: {error}"),
                false,
                false,
            )
        })?
    }

    fn find_worktree(&self, session_key: &str) -> Result<ManagedWorktreeRecord, DispatchError> {
        self.worktrees
            .records()
            .map_err(worktree_state_error)?
            .into_iter()
            .find(|record| record.key == session_key)
            .ok_or_else(|| {
                refused(
                    "That worktree is not managed.",
                    "Interface has no durable record tying this checkout to that session.",
                )
            })
    }

    fn worktree_info(
        &self,
        record: &ManagedWorktreeRecord,
    ) -> Result<ManagedWorktreeInfo, DispatchError> {
        let active = self
            .runtime
            .session_is_active(&record.key)
            .map_err(runtime_dispatch_error)?;
        Ok(ManagedWorktreeInfo {
            key: record.key.clone(),
            path: path_text(&record.path)?,
            session_cwd: path_text(&record.session_cwd)?,
            branch: record.branch.clone(),
            repo_root: path_text(&record.repo_root)?,
            source_cwd: path_text(&record.source_cwd)?,
            dirty_source: record.dirty_source,
            state: if active {
                ManagedWorktreeState::Active
            } else {
                ManagedWorktreeState::Retained
            },
        })
    }

    fn list_managed_worktrees(&self) -> Result<Value, DispatchError> {
        let records = self.worktrees.records().map_err(worktree_state_error)?;
        let worktrees = records
            .iter()
            .map(|record| self.worktree_info(record))
            .collect::<Result<Vec<_>, _>>()?;
        encode_result(worktrees, true)
    }

    async fn open_managed_worktree(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<SessionPayload>(CHANNELS.open_managed_worktree, payload)?;
        let record = self.find_worktree(&request.session_key)?;
        let registration = self
            .worktrees
            .verify(record.clone())
            .await
            .map_err(worktree_state_error)?;
        if registration != ManagedWorktreeRegistration::Active {
            return Err(refused(
                "That retained worktree could not be verified.",
                match registration {
                    ManagedWorktreeRegistration::Absent => {
                        "Git no longer identifies this exact path and branch as the checkout Interface recorded."
                    }
                    ManagedWorktreeRegistration::Unknown => {
                        "Git could not inspect this worktree registration. Try again."
                    }
                    ManagedWorktreeRegistration::Active => unreachable!(),
                },
            ));
        }
        self.desktop
            .grant_workspace_root(&record.path)
            .map_err(|_| {
                refused(
                    "That worktree changed.",
                    "Interface did not authorize a checkout whose path no longer matches its record.",
                )
            })?;
        encode_result(self.worktree_info(&record)?, true)
    }

    #[allow(clippy::too_many_lines)]
    async fn cleanup_managed_worktree(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<SessionPayload>(CHANNELS.cleanup_managed_worktree, payload)?;
        if let Some(collaboration) = &self.collaboration {
            collaboration
                .guard_session_mutation(&request.session_key, "worktree cleanup", false)
                .await?;
        }
        let record = self.find_worktree(&request.session_key)?;
        let before = self.worktree_info(&record)?;
        if before.state == ManagedWorktreeState::Active {
            return kept(
                before,
                ManagedWorktreeKeptReason::Active,
                "Close the live provider session before cleaning up its checkout.",
            );
        }
        if self.desktop.workspace_is_open(&record.path) {
            return kept(
                before,
                ManagedWorktreeKeptReason::Open,
                "Close files from this checkout before cleaning it up.",
            );
        }
        let _removal = match self.desktop.reserve_workspace_removal(&record.path) {
            Ok(Some(reservation)) => reservation,
            Ok(None) => {
                return kept(
                    before,
                    ManagedWorktreeKeptReason::Open,
                    "Close files and saved terminals from this checkout before cleaning it up.",
                );
            }
            Err(error) => {
                return Err(DispatchError::new(
                    error.code,
                    error.message,
                    error.retryable,
                    true,
                ));
            }
        };
        if !std::fs::symlink_metadata(&record.path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            let registration = self
                .worktrees
                .verify(record.clone())
                .await
                .map_err(worktree_state_error)?;
            if registration != ManagedWorktreeRegistration::Active {
                return kept(
                    before,
                    ManagedWorktreeKeptReason::Failed,
                    match registration {
                        ManagedWorktreeRegistration::Absent => {
                            "Git no longer identifies this exact path and branch as the managed worktree."
                        }
                        ManagedWorktreeRegistration::Unknown => {
                            "Git could not inspect this managed worktree. Interface kept it so cleanup can be retried safely."
                        }
                        ManagedWorktreeRegistration::Active => unreachable!(),
                    },
                );
            }
        }
        let removed = self
            .worktrees
            .remove(record.clone())
            .await
            .map_err(worktree_state_error)?;
        if removed.removed {
            self.desktop.revoke_workspace_root(&record.path);
            let changed = self
                .runtime
                .restore_worktree_home(&record.key, &record.worktree())
                .map_err(|error| {
                    DispatchError::new(
                        OUTCOME_UNKNOWN_CODE,
                        format!(
                            "The managed checkout was removed, but its session could not be redirected: {}",
                            error.message
                        ),
                        false,
                        false,
                    )
                })?;
            if changed {
                if let Some(collaboration) = &self.collaboration {
                    collaboration.observe_registry_change(&record.key)?;
                }
                self.emit_registry_snapshot().await?;
            }
        }
        // Ignored files are the owner's work like uncommitted changes, so
        // they cross as the existing `dirty` reason with a detail saying which.
        let detail = match removed.kept {
            Some(RemoveKept::Ignored) => Some(IGNORED_FILES_KEPT.to_owned()),
            _ => removed.detail,
        };
        let result = ManagedWorktreeCleanup {
            worktree: before,
            removed: removed.removed,
            kept: removed.kept.map(|kept| match kept {
                RemoveKept::Dirty | RemoveKept::Ignored => ManagedWorktreeKeptReason::Dirty,
                RemoveKept::Failed => ManagedWorktreeKeptReason::Failed,
            }),
            changes: removed
                .changes
                .map(|changes| u64::try_from(changes).unwrap_or(u64::MAX)),
            detail,
        };
        encode_result(result, !removed.removed)
    }

    async fn emit_registry_snapshot(&self) -> Result<(), DispatchError> {
        let unreachable = || {
            DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                "The worktree was removed, but its registry update could not reach the desktop.",
                false,
                false,
            )
        };
        if let Some(registry_pushes) = &self.registry_pushes {
            return if registry_pushes.flush().await {
                Ok(())
            } else {
                Err(unreachable())
            };
        }
        let Some(pushes) = &self.pushes else {
            return Ok(());
        };
        let sessions = self
            .runtime
            .visible_sessions()
            .map_err(runtime_dispatch_error)?;
        let frame = registry_push_frame(&sessions)?;
        pushes.send(frame).await.map_err(|_| unreachable())
    }
}

#[async_trait]
impl HostDispatcher for NativeHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation == CHANNELS.open_terminal {
            return self.open_terminal(payload).await;
        }
        if operation == CHANNELS.list_managed_worktrees {
            decode_empty(operation, &payload)?;
            return self.list_managed_worktrees();
        }
        if operation == CHANNELS.open_managed_worktree {
            return self.open_managed_worktree(payload).await;
        }
        if operation == CHANNELS.cleanup_managed_worktree {
            return self.cleanup_managed_worktree(payload).await;
        }
        Err(DispatchError::new(
            "operation-not-owned",
            "That operation belongs to another Rust host service.",
            false,
            true,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenTerminalPayload {
    root: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionPayload {
    session_key: String,
}

/// Session cwds that may own a terminal. The filesystem root is excluded:
/// a `/usage` transcript records `cwd: "/"`, and admitting that path would
/// authorize every later pass-off and reopen under `/`.
pub(crate) fn terminal_session_root_from_cwd(cwd: &str) -> Option<PathBuf> {
    let path = Path::new(cwd);
    if cwd.is_empty() || path.parent().is_none() {
        None
    } else {
        Some(path.to_path_buf())
    }
}

/// Keeps only the candidates that canonicalize to a root the terminal
/// authority accepts, then builds the exact approved set.
///
/// # Errors
///
/// Returns a terminal-authority failure when the surviving set is not a valid
/// approved-root set.
pub(crate) fn canonical_terminal_roots(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<ApprovedTerminalRoots, DispatchError> {
    // Identical candidates canonicalize identically; probe each one once.
    let candidates: BTreeSet<PathBuf> = candidates.into_iter().collect();
    let mut unique = BTreeSet::new();
    for candidate in candidates {
        if let Ok(canonical) = std::fs::canonicalize(candidate)
            && ApprovedTerminalRoots::new([canonical.clone()]).is_ok()
        {
            unique.insert(canonical);
        }
    }
    ApprovedTerminalRoots::new(unique).map_err(|error| {
        DispatchError::new(
            "terminal-authority-unavailable",
            error.to_string(),
            false,
            true,
        )
    })
}

pub(crate) fn decode_empty(operation: &str, payload: &Value) -> Result<(), DispatchError> {
    payload::require_empty(payload, || invalid_payload(operation))
}

pub(crate) fn decode_payload<T>(operation: &str, payload: Value) -> Result<T, DispatchError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(payload).map_err(|_| invalid_payload(operation))
}

pub(crate) fn invalid_payload(operation: &str) -> DispatchError {
    DispatchError::new(
        INVALID_PAYLOAD_CODE,
        format!("The {operation} request payload was invalid."),
        false,
        true,
    )
}

/// A cleanup that kept the checkout before touching it: replay-safe.
const IGNORED_FILES_KEPT: &str = "Kept the checkout: it holds files git ignores (such as .env, node_modules or build output), which removing it would delete. Move what you need, then delete the folder yourself.";

fn kept(
    worktree: ManagedWorktreeInfo,
    reason: ManagedWorktreeKeptReason,
    detail: &str,
) -> Result<Value, DispatchError> {
    encode_result(
        ManagedWorktreeCleanup {
            worktree,
            removed: false,
            kept: Some(reason),
            changes: None,
            detail: Some(detail.to_owned()),
        },
        true,
    )
}

fn encode_result<T>(value: T, replay_safe: bool) -> Result<Value, DispatchError>
where
    T: Serialize,
{
    serde_json::to_value(value).map_err(|error| {
        tracing::error!(%error, "failed to encode a native desktop result");
        DispatchError::new(
            "legacy-result-encode-failed",
            "The Rust host could not encode the desktop result.",
            false,
            replay_safe,
        )
    })
}

fn worktree_state_error(error: WorktreeServiceError) -> DispatchError {
    DispatchError::new(
        if error.replay_safe {
            "managed-worktree-state-unavailable"
        } else {
            OUTCOME_UNKNOWN_CODE
        },
        error.message,
        false,
        error.replay_safe,
    )
}

fn terminal_dispatch_error(error: TerminalError) -> DispatchError {
    match error {
        TerminalError::UnauthorizedCwd => refused(
            "Open the workspace root.",
            "Terminal launch accepts an exact current session or workspace root, not an arbitrary subfolder.",
        ),
        TerminalError::UnsafeCwd => refused(
            "Could not open a terminal.",
            "That workspace is missing, linked, or no longer a stable directory.",
        ),
        TerminalError::CwdChanged => DispatchError::new(
            OUTCOME_UNKNOWN_CODE,
            "The workspace changed while Interface was opening the terminal; the app may already have launched.",
            false,
            false,
        ),
        TerminalError::Backend(message) => DispatchError::new(
            "terminal-launch-failed",
            format!("Could not open a terminal.\n{message}"),
            true,
            true,
        ),
        // These variants belong to the integrated PTY service. The legacy
        // external-launch owner never produces them, but the public terminal
        // error type is shared by both services.
        other => DispatchError::new(
            "terminal-launch-failed",
            format!("Could not open a terminal.\n{other}"),
            false,
            true,
        ),
    }
}

fn refused(lead: &str, detail: &str) -> DispatchError {
    DispatchError::new("request-refused", format!("{lead}\n{detail}"), false, true)
}

fn path_text(path: &Path) -> Result<String, DispatchError> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        DispatchError::new(
            "managed-worktree-state-unavailable",
            "A managed worktree path is not valid UTF-8.",
            false,
            true,
        )
    })
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
