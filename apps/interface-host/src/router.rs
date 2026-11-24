use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use interface_contracts::ipc::CHANNELS;
use interface_desktop_services::{
    COVERED_OPERATIONS, DesktopService, DesktopServiceError, INTERNAL_DESKTOP_OPERATIONS,
};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::collaboration::COLLABORATION_OPERATIONS;
use crate::diagnostics::DIAGNOSTICS_OPERATIONS;
use crate::dispatcher::{
    AutomaticWorktreeRelease, DispatchError, HostDispatcher, OPERATION_NOT_MIGRATED_CODE,
    RUNTIME_BRIDGE_OPERATIONS,
};
use crate::native::{NATIVE_SERVICE_OPERATIONS, NativeRuntimeView};
use crate::orchestration::ORCHESTRATION_OPERATIONS;
use crate::project_away::AWAY_OPERATIONS;
use crate::prompt_context::PROMPT_CONTEXT_OPERATIONS;
use crate::provider_accounts::PROVIDER_ACCOUNTS_OPERATIONS;
use crate::remote::{REMOTE_OPERATIONS, RemoteHostDispatcher};
use crate::session_management::SESSION_MANAGEMENT_OPERATIONS;
use crate::terminal::TERMINAL_OPERATIONS;
use crate::threads::THREAD_OPERATIONS;
use crate::updater::{INTERNAL_UPDATER_OPERATIONS, UPDATER_OPERATIONS};
use crate::voice::VOICE_OPERATIONS;

pub const COMPOSITE_OWNED_OPERATION_COUNT: usize = COVERED_OPERATIONS.len()
    + RUNTIME_BRIDGE_OPERATIONS.len()
    + SESSION_MANAGEMENT_OPERATIONS.len()
    + NATIVE_SERVICE_OPERATIONS.len()
    + ORCHESTRATION_OPERATIONS.len()
    + COLLABORATION_OPERATIONS.len()
    + PROMPT_CONTEXT_OPERATIONS.len()
    + TERMINAL_OPERATIONS.len()
    + UPDATER_OPERATIONS.len()
    + AWAY_OPERATIONS.len()
    + VOICE_OPERATIONS.len()
    + REMOTE_OPERATIONS.len()
    + THREAD_OPERATIONS.len()
    + DIAGNOSTICS_OPERATIONS.len()
    + PROVIDER_ACCOUNTS_OPERATIONS.len();

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
#[error("operation {operation} has more than one host owner")]
pub struct DuplicateOperationOwner {
    pub operation: String,
}

/// Exhaustive owner router for the currently migrated service slices.
///
/// There is deliberately no next/fallback dispatcher. A frozen operation has
/// one exact owner or fails with `operation-not-migrated`.
pub struct OperationOwnerRouter {
    runtime: Arc<dyn HostDispatcher>,
    session_management: Arc<dyn HostDispatcher>,
    desktop: Arc<dyn HostDispatcher>,
    native: Arc<dyn HostDispatcher>,
    orchestration: Arc<dyn HostDispatcher>,
    collaboration: Arc<dyn HostDispatcher>,
    prompt_context: Arc<dyn HostDispatcher>,
    terminal: Arc<dyn HostDispatcher>,
    updater: Arc<dyn HostDispatcher>,
    voice: Arc<dyn HostDispatcher>,
    away: Option<Arc<dyn HostDispatcher>>,
    remote: Arc<dyn HostDispatcher>,
    agents: Option<Arc<dyn HostDispatcher>>,
    threads: Option<Arc<dyn HostDispatcher>>,
    diagnostics: Option<Arc<dyn HostDispatcher>>,
    provider_accounts: Option<Arc<dyn HostDispatcher>>,
}

impl OperationOwnerRouter {
    /// Constructs the router after proving that ownership tables do not overlap.
    ///
    /// # Errors
    ///
    /// Returns the duplicated operation if ownership tables overlap.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime: Arc<dyn HostDispatcher>,
        session_management: Arc<dyn HostDispatcher>,
        desktop: Arc<dyn HostDispatcher>,
        native: Arc<dyn HostDispatcher>,
        orchestration: Arc<dyn HostDispatcher>,
        collaboration: Arc<dyn HostDispatcher>,
        prompt_context: Arc<dyn HostDispatcher>,
        terminal: Arc<dyn HostDispatcher>,
        updater: Arc<dyn HostDispatcher>,
        voice: Arc<dyn HostDispatcher>,
    ) -> Result<Self, DuplicateOperationOwner> {
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
            &VOICE_OPERATIONS,
            &AWAY_OPERATIONS,
            &REMOTE_OPERATIONS,
            &THREAD_OPERATIONS,
            &DIAGNOSTICS_OPERATIONS,
            &PROVIDER_ACCOUNTS_OPERATIONS,
        ])?;
        Ok(Self {
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
            away: None,
            remote: Arc::new(RemoteHostDispatcher::default()),
            agents: None,
            threads: None,
            diagnostics: None,
            provider_accounts: None,
        })
    }
    #[must_use]
    pub fn with_away(mut self, away: Arc<dyn HostDispatcher>) -> Self {
        self.away = Some(away);
        self
    }
    #[must_use]
    pub fn with_remote(mut self, remote: Arc<dyn HostDispatcher>) -> Self {
        self.remote = remote;
        self
    }
    /// Serves `iface:agentControl` ahead of the desktop dispatcher, which it forwards to.
    #[must_use]
    pub fn with_agents(mut self, agents: Arc<dyn HostDispatcher>) -> Self {
        self.agents = Some(agents);
        self
    }
    /// Serves `iface:threadControl` ahead of the desktop dispatcher. Without
    /// it the operation answers `operation-not-migrated`.
    #[must_use]
    pub fn with_threads(mut self, threads: Arc<dyn HostDispatcher>) -> Self {
        self.threads = Some(threads);
        self
    }
    /// Serves `iface:diagnostics`. Without it the operation answers
    /// `operation-not-migrated`.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn HostDispatcher>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }
    /// Serves `iface:providerAccounts` (protocol 60). Without it the
    /// operation answers `operation-not-migrated`.
    #[must_use]
    pub fn with_provider_accounts(mut self, accounts: Arc<dyn HostDispatcher>) -> Self {
        self.provider_accounts = Some(accounts);
        self
    }
}

#[async_trait]
impl HostDispatcher for OperationOwnerRouter {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if RUNTIME_BRIDGE_OPERATIONS.contains(&operation) {
            return self.runtime.dispatch(operation, payload).await;
        }
        if SESSION_MANAGEMENT_OPERATIONS.contains(&operation) {
            return self.session_management.dispatch(operation, payload).await;
        }
        if operation == CHANNELS.agent_control
            && let Some(agents) = &self.agents
        {
            return agents.dispatch(operation, payload).await;
        }
        if THREAD_OPERATIONS.contains(&operation)
            && let Some(threads) = &self.threads
        {
            return threads.dispatch(operation, payload).await;
        }
        if COVERED_OPERATIONS.contains(&operation)
            || INTERNAL_DESKTOP_OPERATIONS.contains(&operation)
        {
            return self.desktop.dispatch(operation, payload).await;
        }
        if NATIVE_SERVICE_OPERATIONS.contains(&operation) {
            return self.native.dispatch(operation, payload).await;
        }
        if ORCHESTRATION_OPERATIONS.contains(&operation) {
            return self.orchestration.dispatch(operation, payload).await;
        }
        if COLLABORATION_OPERATIONS.contains(&operation) {
            return self.collaboration.dispatch(operation, payload).await;
        }
        if PROMPT_CONTEXT_OPERATIONS.contains(&operation) {
            return self.prompt_context.dispatch(operation, payload).await;
        }
        if TERMINAL_OPERATIONS.contains(&operation) {
            return self.terminal.dispatch(operation, payload).await;
        }
        if UPDATER_OPERATIONS.contains(&operation)
            || INTERNAL_UPDATER_OPERATIONS.contains(&operation)
        {
            return self.updater.dispatch(operation, payload).await;
        }
        if REMOTE_OPERATIONS.contains(&operation) {
            return self.remote.dispatch(operation, payload).await;
        }
        if AWAY_OPERATIONS.contains(&operation) {
            if let Some(away) = &self.away {
                return away.dispatch(operation, payload).await;
            }
        }
        if VOICE_OPERATIONS.contains(&operation) {
            return self.voice.dispatch(operation, payload).await;
        }
        if DIAGNOSTICS_OPERATIONS.contains(&operation)
            && let Some(diagnostics) = &self.diagnostics
        {
            return diagnostics.dispatch(operation, payload).await;
        }
        if PROVIDER_ACCOUNTS_OPERATIONS.contains(&operation)
            && let Some(accounts) = &self.provider_accounts
        {
            return accounts.dispatch(operation, payload).await;
        }
        Err(DispatchError::new(
            OPERATION_NOT_MIGRATED_CODE,
            "This Interface operation has not migrated to the Rust host.",
            false,
            true,
        ))
    }
}

/// Host-facing adapter for the transport-neutral desktop service slice.
pub struct DesktopHostDispatcher {
    service: Arc<DesktopService>,
    blocking_gate: Arc<Semaphore>,
    session_root_provider: Option<Arc<dyn DesktopSessionRootProvider>>,
    worktree_release: Option<Arc<dyn AutomaticWorktreeRelease>>,
}

const DESKTOP_BLOCKING_CONCURRENCY: usize = 4;
const DESKTOP_WORKSPACE_OPERATIONS: [&str; 15] = [
    CHANNELS.harness_control,
    CHANNELS.read_file,
    CHANNELS.read_file_snapshot,
    CHANNELS.save_file,
    CHANNELS.list_dir,
    CHANNELS.list_project_files,
    CHANNELS.search_project,
    CHANNELS.list_customization,
    CHANNELS.customization_control,
    CHANNELS.read_repository_review,
    CHANNELS.discover_review_checks,
    CHANNELS.run_review_checks,
    CHANNELS.create_review_checkpoint,
    CHANNELS.list_review_checkpoints,
    CHANNELS.restore_review_checkpoint,
];

/// Read-only projection of the exact session roots authorized at dispatch
/// time. Implementations must include visible rows and archived rows whose
/// provider effect remains live, and must omit rows that have disappeared.
pub trait DesktopSessionRootProvider: Send + Sync + 'static {
    /// Returns the current session-root snapshot.
    ///
    /// # Errors
    ///
    /// Fails closed when registry or durable live-handle state is unavailable.
    fn current_session_roots(&self) -> Result<Vec<PathBuf>, DispatchError>;
}

impl<T> DesktopSessionRootProvider for T
where
    T: NativeRuntimeView,
{
    fn current_session_roots(&self) -> Result<Vec<PathBuf>, DispatchError> {
        self.approved_session_roots().map_err(|error| {
            tracing::error!(kind = ?error.kind, "could not read current desktop session roots");
            DispatchError::new(
                "desktop-state-unavailable",
                "Interface could not read the current session workspaces.",
                true,
                true,
            )
        })
    }
}

impl DesktopHostDispatcher {
    #[must_use]
    pub fn new(service: Arc<DesktopService>) -> Self {
        Self {
            service,
            blocking_gate: Arc::new(Semaphore::new(DESKTOP_BLOCKING_CONCURRENCY)),
            session_root_provider: None,
            worktree_release: None,
        }
    }

    #[must_use]
    pub fn with_session_root_provider<T>(mut self, provider: Arc<T>) -> Self
    where
        T: DesktopSessionRootProvider,
    {
        self.session_root_provider = Some(provider);
        self
    }

    #[must_use]
    pub fn with_worktree_release(
        mut self,
        worktree_release: Arc<dyn AutomaticWorktreeRelease>,
    ) -> Self {
        self.worktree_release = Some(worktree_release);
        self
    }
}

#[async_trait]
impl HostDispatcher for DesktopHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let released_workspace = (operation == CHANNELS.set_workspace_open
            && payload.get("open").and_then(Value::as_bool) == Some(false))
        .then(|| {
            payload
                .get("root")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .flatten();
        let track_saved_terminals =
            operation == CHANNELS.set_terminal_workspaces && self.worktree_release.is_some();
        let permit = Arc::clone(&self.blocking_gate)
            .acquire_owned()
            .await
            .map_err(|_| {
                DispatchError::new(
                    "desktop-worker-unavailable",
                    "The bounded desktop worker pool is unavailable.",
                    true,
                    true,
                )
            })?;
        let session_roots = if DESKTOP_WORKSPACE_OPERATIONS.contains(&operation) {
            self.session_root_provider.as_ref().map_or_else(
                || Ok(Vec::new()),
                |provider| provider.current_session_roots(),
            )?
        } else {
            Vec::new()
        };
        let service = Arc::clone(&self.service);
        let worker_operation = operation.to_owned();
        let (result, saved_terminals) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // The saved-terminal reads wait on the terminal state gate that
            // writers hold across persistence, so they run on this worker,
            // around this operation's write, rather than on the async executor.
            let previous = if track_saved_terminals {
                service.saved_terminal_cwds()
            } else {
                None
            };
            let result = service.dispatch_blocking_with_session_roots(
                &worker_operation,
                payload,
                &session_roots,
            );
            let saved_terminals = previous
                .filter(|_| result.is_ok())
                .and_then(|previous| Some((previous, service.saved_terminal_cwds()?)));
            (result, saved_terminals)
        })
        .await
        .map_err(|error| {
            DispatchError::new(
                "desktop-worker-failed",
                format!("The bounded desktop worker failed: {error}"),
                false,
                false,
            )
        })?;
        let result = result.map_err(|error| desktop_dispatch_error(operation, error))?;
        if let Some(workspace) = released_workspace
            && let Some(worktree_release) = &self.worktree_release
        {
            worktree_release.schedule_workspace_release(workspace);
        }
        if let Some((previous, current)) = saved_terminals
            && let Some(worktree_release) = &self.worktree_release
        {
            let released = previous
                .into_iter()
                .filter(|(id, cwd)| {
                    !current
                        .iter()
                        .any(|(current_id, current_cwd)| current_id == id && current_cwd == cwd)
                })
                .map(|(_, cwd)| cwd)
                .collect::<Vec<_>>();
            if !released.is_empty() {
                // Each lookup canonicalizes every managed-worktree path.
                let worktree_release = Arc::clone(worktree_release);
                let _ = tokio::task::spawn_blocking(move || {
                    for cwd in released {
                        if let Some(root) = worktree_release.managed_workspace_root(&cwd) {
                            worktree_release
                                .schedule_workspace_release(root.to_string_lossy().into_owned());
                        }
                    }
                })
                .await;
            }
        }
        Ok(result)
    }
}

fn validate_operation_owners(owners: &[&[&str]]) -> Result<(), DuplicateOperationOwner> {
    let mut operations = BTreeSet::new();
    for owner in owners {
        for operation in *owner {
            if !operations.insert(*operation) {
                return Err(DuplicateOperationOwner {
                    operation: (*operation).to_owned(),
                });
            }
        }
    }
    Ok(())
}

fn desktop_dispatch_error(operation: &str, error: DesktopServiceError) -> DispatchError {
    let replay_safe = desktop_replay_is_safe(operation, &error);
    DispatchError::new(error.code, error.message, error.retryable, replay_safe)
}

fn desktop_replay_is_safe(operation: &str, error: &DesktopServiceError) -> bool {
    use interface_desktop_services::DesktopEffectCertainty;
    if operation == CHANNELS.agent_control
        || operation == CHANNELS.harness_control
        || operation == CHANNELS.thread_control
    {
        return false;
    }
    match error.certainty {
        DesktopEffectCertainty::DefinitelyUnsent => return true,
        DesktopEffectCertainty::OutcomeUnknown => return false,
        DesktopEffectCertainty::OperationDefault => {}
    }
    ![
        "iface:setAppearance",
        "iface:setAcpProfiles",
        "iface:saveFile",
        "iface:createProject",
        "iface:runReviewChecks",
        "iface:createReviewCheckpoint",
        "iface:restoreReviewCheckpoint",
    ]
    .contains(&operation)
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
