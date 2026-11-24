//! Host ownership for integrated terminal lifecycle and binary I/O.
//!
//! The JSON bridge controls terminal identity and lifecycle. Exact PTY bytes
//! are forwarded through [`crate::terminal_transport`] so shell output cannot
//! consume the bounded control-plane queue.

mod authority;
mod create;
mod dismiss;
mod handoff;
mod io;
mod lifecycle;
mod restart;
mod shutdown;
mod spawn;

#[cfg(test)]
use {create::recover_legacy_claude_binding, dismiss::finish_dismiss_authority_rollback};

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use interface_contracts::{
    ipc::CHANNELS,
    terminal::{
        CreateTerminalRequest, DismissTerminalRequest, RenameTerminalRequest,
        ResizeTerminalRequest, TerminalGeneration, TerminalGenerationRequest, TerminalId,
        TerminalLaunchProfile, TerminalLaunchProfileId, TerminalLaunchSessionV1,
        TerminalLifecycleEvent, TerminalSnapshot, TerminalState, TerminalSummary,
        TerminalWorkspaceTerminal,
    },
    workspace_tools::OpenWorkspaceToolV1,
};
use interface_desktop_services::{
    DesktopService, DesktopServiceError, SavedTerminalProviderSessionPersistence,
    TerminalReopenAuthority, TerminalReopenAuthorityCompareExchange, TerminalReopenAuthoritySlot,
    TerminalReopenAuthorityState,
};
use interface_terminal::{
    ApprovedTerminalRoots, SystemTerminalPtyBackend, SystemWorkspaceToolLauncher,
    TerminalBackendError, TerminalError, TerminalLaunchCatalog, TerminalLeaseRequest,
    TerminalLifecycleSink, TerminalProviderSessionLookup, TerminalProviderSessionPersistence,
    TerminalPtyBackend, TerminalService, TerminalServiceConfig, TerminalServiceParts,
    TerminalWorkspaceLease, TerminalWorkspaceLeaseProvider, WorkspaceToolError,
    open_workspace_tool,
};
use interface_voice::TailBuffer;
use serde::Serialize;
use serde_json::Value;
use tokio::{
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    dispatcher::{
        AutomaticWorktreeRelease, DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE,
        ManagedWorkspaceVerification, OPERATION_NOT_MIGRATED_CODE, OUTCOME_UNKNOWN_CODE,
        VerifiedManagedWorkspace, runtime_dispatch_error,
    },
    native::{NativeRuntimeView, canonical_terminal_roots, decode_empty, decode_payload},
    protocol::{BridgeError, PushFrame},
    provider_capture::ProviderCaptureLookup,
    terminal_transport::{
        TerminalDataDirection, TerminalDataFrame, TerminalDataKind, TerminalDataReadError,
        TerminalDataSender, TerminalTransportFailure,
    },
};

pub const TERMINAL_OPERATIONS: [&str; 14] = [
    CHANNELS.open_workspace_tool,
    CHANNELS.preview_output,
    CHANNELS.inspect_terminal_servers,
    CHANNELS.list_terminal_launch_profiles,
    CHANNELS.list_terminals,
    CHANNELS.read_terminal_delegation,
    CHANNELS.advance_terminal_controls,
    CHANNELS.create_terminal,
    CHANNELS.rename_terminal,
    CHANNELS.resize_terminal,
    CHANNELS.set_terminal_workspace_activity,
    CHANNELS.restart_terminal,
    CHANNELS.close_terminal,
    CHANNELS.dismiss_terminal,
];

// close() drains every permit to fence orphaned blocking workers, so the
// semaphore size is derived from the permit count, never kept in sync by hand.
const TERMINAL_BLOCKING_CONCURRENCY_PERMITS: u32 = 4;
const TERMINAL_BLOCKING_CONCURRENCY: usize = TERMINAL_BLOCKING_CONCURRENCY_PERMITS as usize;

type TerminalStreamKey = (TerminalId, TerminalGeneration);

/// Re-marks an error from a step that may already have taken effect as not
/// replay-safe, keeping its code, message and retryability.
fn not_replay_safe(error: DispatchError) -> DispatchError {
    DispatchError::new(error.code, error.message, error.retryable, false)
}

/// A desktop-service refusal as a dispatch error. `replay_safe` says whether
/// the step that failed could not have taken effect.
fn desktop_error(error: DesktopServiceError, replay_safe: bool) -> DispatchError {
    DispatchError::new(error.code, error.message, error.retryable, replay_safe)
}

/// Every host-confirmed project candidate: saved terminal workspaces, the
/// roots of live terminals and approved session roots, in that order.
fn workspace_root_candidates(
    desktop: &DesktopService,
    service: &TerminalService,
    runtime: &dyn NativeRuntimeView,
) -> Result<Vec<PathBuf>, DispatchError> {
    let mut roots = desktop.terminal_workspace_roots();
    roots.extend(service.active_workspace_roots());
    roots.extend(
        runtime
            .approved_session_roots()
            .map_err(runtime_dispatch_error)?,
    );
    Ok(roots)
}

/// Runs one blocking cleanup step, reporting its error or a lost worker as
/// text for the retry log.
async fn blocking_step<T, E>(
    step: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<(), String>
where
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    match tokio::task::spawn_blocking(step).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("worker failed: {error}")),
    }
}

/// Paces a background rollback that must never give up: 100 ms, doubling to
/// 5 s, with a warning on the first failure and then every tenth.
#[derive(Default)]
struct CleanupRetry {
    attempts: u32,
}

impl CleanupRetry {
    const INITIAL: Duration = Duration::from_millis(100);
    const MAX: Duration = Duration::from_secs(5);
    const WARN_EVERY: u32 = 10;

    async fn fail(&mut self, id: TerminalId, what: &str, failure: &str) {
        if self.attempts.is_multiple_of(Self::WARN_EVERY) {
            tracing::warn!(id = %id, failure, attempts = self.attempts + 1, "{what}");
        }
        self.wait().await;
    }

    async fn wait(&mut self) {
        let delay = Self::INITIAL
            .saturating_mul(1 << self.attempts.min(6))
            .min(Self::MAX);
        self.attempts = self.attempts.saturating_add(1);
        tokio::time::sleep(delay).await;
    }
}

struct TerminalOutputPump {
    task: std::thread::JoinHandle<()>,
    admission: Option<oneshot::Sender<()>>,
}

#[derive(Clone, Copy)]
enum ReopenAuthoritySource {
    SavedRoster,
    LiveUnsaved,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReopenAuthorityTransactionPhase {
    Pending,
    RollingBack,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReopenAuthorityTransactionKind {
    Create,
    Restart {
        prior_generation: TerminalGeneration,
    },
    Dismiss,
}

#[derive(Clone)]
struct ReopenAuthorityTransaction {
    kind: ReopenAuthorityTransactionKind,
    phase: ReopenAuthorityTransactionPhase,
    prior: TerminalReopenAuthoritySlot,
    attempted: Option<TerminalReopenAuthoritySlot>,
}

/// Which terminal generations accept input, read on every keystroke without
/// the lifecycle lock (which publish holds across durable sidecar writes).
type InputAdmission = Arc<Mutex<BTreeMap<TerminalId, TerminalGeneration>>>;

/// Each visible terminal's input lane. A lane's thread ends once its sender
/// is dropped and its queue drains, so a lane is removed when its terminal
/// leaves visibility. Lock order: lifecycle state, then lanes, then
/// admission; the input path takes lanes, then admission.
type InputLanes = Mutex<BTreeMap<TerminalId, mpsc::Sender<TerminalDataFrame>>>;

#[derive(Default)]
struct HostTerminalLifecycleState {
    /// Add or remove only through `begin_transaction`/`end_transaction`, so
    /// the input admission mirror stays exact.
    transactions: BTreeMap<TerminalId, ReopenAuthorityTransaction>,
    /// At most one coalesced event per uncommitted terminal. Host delivery
    /// revisions are assigned only when an event becomes visible, so one
    /// stuck rollback cannot freeze or unboundedly accumulate other panes.
    queued_events: BTreeMap<TerminalId, TerminalLifecycleEvent>,
    delivered_revision: u64,
    /// Add or remove only through `show`/`hide`, like `transactions`.
    visible_terminals: BTreeMap<TerminalId, TerminalSummary>,
    /// Exactly each visible terminal's generation while it has no pending
    /// transaction. Updated under this state's lock (lifecycle, then
    /// admission); the input path takes only the admission lock.
    admission: InputAdmission,
}

struct HostTerminalLifecycleSink {
    pushes: mpsc::Sender<PushFrame>,
    desktop: Arc<DesktopService>,
    /// Phone replay retention follows delivered visibility, so no request
    /// polls the lifecycle state for it.
    remote_hub: Arc<interface_remote::RemoteHub>,
    pending_managed_reopen_proofs: Arc<Mutex<BTreeMap<TerminalId, VerifiedManagedWorkspace>>>,
    state: Mutex<HostTerminalLifecycleState>,
    /// The same map as `state.admission`, for the input path.
    admission: InputAdmission,
    input_lanes: InputLanes,
    /// Each generation's readable tail. It follows delivered visibility, not
    /// the output pump: an exited pane that is still shown keeps its output
    /// readable by `read_pane`.
    output_tails: Mutex<BTreeMap<TerminalStreamKey, Arc<Mutex<TailBuffer>>>>,
    /// Protocol 50: each visible clean agent exit, for the shell handoff
    /// worker. Unbounded and non-blocking: delivery runs on a process waiter
    /// thread under the lifecycle lock, where nothing may wait.
    shell_handoffs:
        std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<(TerminalId, TerminalGeneration)>>,
    /// Test seam: runs in `publish` under the lifecycle lock, right before
    /// an Upsert's durable sidecar write.
    #[cfg(test)]
    durable_write_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct DesktopTerminalLeaseProvider {
    desktop: Arc<DesktopService>,
    worktree_release: Arc<dyn AutomaticWorktreeRelease>,
}

impl TerminalWorkspaceLeaseProvider for DesktopTerminalLeaseProvider {
    fn acquire(
        &self,
        request: &TerminalLeaseRequest,
    ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError> {
        let owner = format!("terminal:{}", request.id);
        let root = self
            .desktop
            .acquire_workspace_lease(&request.root, &owner)
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        Ok(Box::new(DesktopTerminalLease {
            desktop: Arc::clone(&self.desktop),
            worktree_release: Arc::clone(&self.worktree_release),
            root,
            owner,
        }))
    }
}

struct DesktopTerminalLease {
    desktop: Arc<DesktopService>,
    worktree_release: Arc<dyn AutomaticWorktreeRelease>,
    root: PathBuf,
    owner: String,
}

impl TerminalWorkspaceLease for DesktopTerminalLease {}

impl Drop for DesktopTerminalLease {
    fn drop(&mut self) {
        if let Err(error) = self
            .desktop
            .release_workspace_lease(&self.root, &self.owner)
        {
            tracing::error!(%error.message, root = %self.root.display(), "terminal workspace lease did not release");
            return;
        }
        self.worktree_release
            .schedule_workspace_release(self.root.to_string_lossy().into_owned());
    }
}

/// Short lease held while a saved worktree proof is asynchronously rechecked.
/// The normal terminal lease is installed before this one is released, so an
/// automatic cleanup cannot enter between verification and PTY ownership.
struct ColdReopenWorkspaceLease {
    desktop: Arc<DesktopService>,
    root: PathBuf,
    owner: String,
}

impl ColdReopenWorkspaceLease {
    fn acquire(
        desktop: &Arc<DesktopService>,
        root: &Path,
        terminal_id: TerminalId,
    ) -> Result<Self, DispatchError> {
        let owner = format!("cold-reopen:{terminal_id}");
        let root = desktop
            .acquire_workspace_lease(root, &owner)
            .map_err(|error| desktop_error(error, true))?;
        Ok(Self {
            desktop: Arc::clone(desktop),
            root,
            owner,
        })
    }
}

impl Drop for ColdReopenWorkspaceLease {
    fn drop(&mut self) {
        if let Err(error) = self
            .desktop
            .release_workspace_lease(&self.root, &self.owner)
        {
            tracing::error!(
                code = %error.code,
                root = %self.root.display(),
                "cold-reopen workspace lease did not release"
            );
        }
    }
}

/// The single host owner for all integrated-terminal control operations.
struct DurableTerminalTitles(Mutex<interface_desktop_services::TerminalTitleStore>);

impl interface_terminal::TerminalTitlePersistence for DurableTerminalTitles {
    fn user_title(&self, id: TerminalId) -> Result<Option<String>, TerminalBackendError> {
        self.0
            .lock()
            .map(|store| store.get(id))
            .map_err(|_| TerminalBackendError::new("The pane name store is unavailable"))
    }
    fn pin_title(&self, id: TerminalId, title: &str) -> Result<(), TerminalBackendError> {
        self.0
            .lock()
            .map_err(|_| TerminalBackendError::new("The pane name store is unavailable"))?
            .pin(id, title)
            .map_err(|_| TerminalBackendError::new("The pane name could not be durably saved"))
    }
}

impl DurableTerminalTitles {
    /// At startup, before any pane exists, the saved roster is every pane that
    /// can ever be reopened: dismissed ids are never recreated and new panes
    /// get fresh ids. Names outside it are unreachable, so they are dropped
    /// to keep the store under its cap. An unavailable roster proves nothing
    /// and keeps every name; a failed prune changes nothing visible either.
    fn forget_unsaved_panes(&self, desktop: &DesktopService) {
        let Some(saved) = desktop.saved_terminal_cwds() else {
            return;
        };
        let saved = saved.into_iter().map(|(id, _)| id).collect();
        if let Ok(mut store) = self.0.lock() {
            let _ = store.retain(&saved);
        }
    }
}

pub struct TerminalHostDispatcher {
    service: TerminalService,
    pub(crate) remote_hub: Arc<interface_remote::RemoteHub>,
    runtime: Arc<dyn NativeRuntimeView>,
    pub(crate) desktop: Arc<DesktopService>,
    data_sender: Option<TerminalDataSender>,
    blocking_gate: Arc<Semaphore>,
    output_pumps: Mutex<BTreeMap<TerminalStreamKey, TerminalOutputPump>>,
    input_pump: Mutex<Option<JoinHandle<()>>>,
    failure_monitor: Mutex<Option<JoinHandle<()>>>,
    lifecycle_dispatch_gate: Arc<tokio::sync::Mutex<()>>,
    claude_projects_root: Option<PathBuf>,
    worktree_release: Arc<dyn AutomaticWorktreeRelease>,
    lifecycle_sink: Arc<HostTerminalLifecycleSink>,
    pending_managed_reopen_proofs: Arc<Mutex<BTreeMap<TerminalId, VerifiedManagedWorkspace>>>,
    transport_available: AtomicBool,
    closed: AtomicBool,
    provider_refresh: std::sync::OnceLock<Arc<crate::provider_refresh::ProviderRefresh>>,
    prompt_context: std::sync::OnceLock<Arc<interface_prompt_context::PromptContextService>>,
    /// Protocol 51: the provider CLI drift canary, production only.
    provider_canary: std::sync::OnceLock<Arc<crate::provider_canary::ProviderCanaryWatch>>,
    /// Each launch profile's last `--version`, by the file it came from.
    profile_versions: Mutex<BTreeMap<TerminalLaunchProfileId, Arc<interface_engine::VersionCache>>>,
    /// Protocol 50: the worker that hands a cleanly exited agent's pane a shell.
    shell_handoff: Mutex<Option<JoinHandle<()>>>,
    /// The Claude capture lookup, which also asks added accounts' stores
    /// once the host installs its provider accounts (protocol 60).
    capture_accounts: Option<Arc<crate::provider_capture::accounts::AccountCaptureLookup>>,
}

/// How long a CLI's `--version` may take for Settings' Terminal tools.
const PROFILE_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The version a CLI printed as Settings shows it: the first non-empty line,
/// without control characters, at most 60 characters.
fn shown_version(answer: &str) -> Option<String> {
    let line = answer
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let clean = line
        .chars()
        .filter(|character| !character.is_control())
        .take(60)
        .collect::<String>();
    (!clean.is_empty()).then_some(clean)
}

impl TerminalHostDispatcher {
    /// Each found CLI's `--version`, first line, kept while its file is
    /// unchanged; one that does not answer within five seconds has none.
    async fn fill_profile_versions(&self, profiles: &mut [TerminalLaunchProfile]) {
        let reads =
            self.service
                .launch_profile_executables()
                .into_iter()
                .map(|(id, executable)| {
                    let cache = Arc::clone(
                        interface_common::sync::lock(&self.profile_versions)
                            .entry(id)
                            .or_default(),
                    );
                    async move {
                        let version = cache
                            .get_or_probe(
                                &executable,
                                interface_engine::cli_version(
                                    &executable,
                                    interface_terminal::profile_label(id),
                                    "no version",
                                    PROFILE_VERSION_TIMEOUT,
                                ),
                            )
                            .await
                            .ok()
                            .and_then(|answer| shown_version(&answer));
                        (id, version)
                    }
                });
        for (id, version) in futures::future::join_all(reads).await {
            if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == id) {
                profile.version = version;
            }
        }
    }

    /// `listTerminalLaunchProfiles`: refresh (41), versions (43) and the
    /// pane tools switch (47) first, as the payload asks.
    async fn list_launch_profiles(&self, payload: &Value) -> Result<Value, DispatchError> {
        let request = crate::provider_refresh::launch_profiles_request(payload, || {
            DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "That launch-profile request is not one Interface understands.",
                false,
                true,
            )
        })?;
        // Protocol 41: the launcher's Try again finds a CLI installed since start.
        if request.refresh
            && let Some(refresh) = self.provider_refresh.get()
            && let Err(error) = refresh.refresh().await
        {
            tracing::warn!(%error, "provider CLIs could not be looked up again");
        }
        // Protocol 48: Settings turns one CLI's use of Interface's tools on or off.
        if let Some(change) = request.pane_tools {
            self.change_pane_tools(change).await?;
        }
        let mut profiles = self.service.launch_profiles();
        // Protocol 43: Settings' Terminal tools show each found CLI's version.
        if request.details || request.pane_tools.is_some() {
            self.fill_profile_versions(&mut profiles).await;
            self.fill_pane_tools(&mut profiles).await;
            // Protocol 51: and what changed in each since it was last checked.
            self.fill_tool_changes(&mut profiles, crate::provider_canary::FirstSight::Defer)
                .await;
        }
        encode_result(profiles)
    }

    /// Protocol 51: check each found CLI whose version changed since the
    /// provider canary last checked it, and say what changed. A CLI never
    /// checked before is checked in the background when `first_sight` defers.
    async fn fill_tool_changes(
        &self,
        profiles: &mut [TerminalLaunchProfile],
        first_sight: crate::provider_canary::FirstSight,
    ) {
        let Some(watch) = self.provider_canary.get() else {
            return;
        };
        let found = self
            .service
            .launch_profile_executables()
            .into_iter()
            .filter_map(|(id, executable)| {
                let version = profiles
                    .iter()
                    .find(|profile| profile.id == id)?
                    .version
                    .clone()?;
                Some((id, executable, version))
            })
            .collect::<Vec<_>>();
        let (mut changes, deferred) = watch.observe(found, first_sight).await;
        if !deferred.is_empty() {
            let watch = Arc::clone(watch);
            tokio::spawn(async move {
                let _ = watch
                    .observe(deferred, crate::provider_canary::FirstSight::Check)
                    .await;
            });
        }
        for profile in profiles {
            profile.changed_since_last_check = changes.remove(&profile.id);
        }
    }

    /// Protocol 51: keep the provider canary's record with this owner, and
    /// check every found CLI once, `delay` after the host starts, so Settings
    /// usually has the answer before it is opened.
    pub fn start_provider_canary(
        self: &Arc<Self>,
        watch: Arc<crate::provider_canary::ProviderCanaryWatch>,
        delay: Duration,
    ) {
        if self.provider_canary.set(watch).is_err() {
            return;
        }
        let owner = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(owner) = owner.upgrade() else {
                return;
            };
            if owner.closed.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let mut profiles = owner.service.launch_profiles();
            owner.fill_profile_versions(&mut profiles).await;
            owner
                .fill_tool_changes(&mut profiles, crate::provider_canary::FirstSight::Check)
                .await;
        });
    }

    /// Protocol 48: whether Grok and Antigravity may use Interface's tools,
    /// read from each found CLI's own config, both at once.
    async fn fill_pane_tools(&self, profiles: &mut [TerminalLaunchProfile]) {
        let reads = self
            .service
            .launch_profile_executables()
            .into_iter()
            .filter_map(|(id, executable)| {
                let cli = interface_terminal::pane_tools::PaneToolsCli::for_profile(id)?;
                Some(async move {
                    let entry = tokio::task::spawn_blocking(move || {
                        crate::pane_mcp::read_pane_tools_entry(cli, &executable)
                    })
                    .await
                    .unwrap_or(interface_terminal::pane_tools::PaneToolsEntry::Unreadable);
                    (id, entry.state())
                })
            });
        for (id, state) in futures::future::join_all(reads).await {
            if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == id) {
                profile.pane_tools = Some(state);
            }
        }
    }

    /// Protocol 48: writes or removes Interface's `iface` entry in one CLI's
    /// user config, as the owner switched it in Settings.
    async fn change_pane_tools(
        &self,
        change: interface_contracts::terminal::TerminalPaneToolsChange,
    ) -> Result<(), DispatchError> {
        use crate::pane_mcp::{PaneToolsChangeError, set_pane_tools_entry};
        let Some(cli) =
            interface_terminal::pane_tools::PaneToolsCli::for_profile(change.profile_id)
        else {
            return Err(DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "Only Grok and Antigravity keep Interface's tools in their own config.",
                false,
                true,
            ));
        };
        let label = cli.label();
        let Some(executable) = self
            .service
            .launch_profile_executables()
            .into_iter()
            .find_map(|(id, executable)| (id == change.profile_id).then_some(executable))
        else {
            return Err(DispatchError::new(
                "terminal-pane-tools-unavailable",
                format!("{label} is not installed, so there is no config to change."),
                false,
                true,
            ));
        };
        let enabled = change.enabled;
        let outcome = tokio::task::spawn_blocking(move || {
            set_pane_tools_entry(cli, &executable, enabled)
        })
        .await
        .map_err(|_| {
            DispatchError::new(
                "terminal-pane-tools-unknown",
                format!("Interface lost track of {label}'s config change. Check Settings before trying again."),
                false,
                false,
            )
        })?;
        match outcome {
            Ok(_) => Ok(()),
            Err(PaneToolsChangeError::Unreadable) => Err(DispatchError::new(
                "terminal-pane-tools-unreadable",
                format!("Interface could not read {label}'s MCP servers, so it changed nothing."),
                true,
                true,
            )),
            Err(PaneToolsChangeError::Taken) => Err(DispatchError::new(
                "terminal-pane-tools-taken",
                format!(
                    "{label} already has its own MCP server named iface. Interface leaves it alone; rename or remove it in {label} first."
                ),
                false,
                true,
            )),
            Err(PaneToolsChangeError::Refused(detail)) => Err(DispatchError::new(
                "terminal-pane-tools-refused",
                format!("{label} did not change its MCP servers ({detail})."),
                true,
                true,
            )),
            Err(PaneToolsChangeError::Unknown(detail)) => Err(DispatchError::new(
                "terminal-pane-tools-unknown",
                format!(
                    "{label} did not answer in time ({detail}). Check Settings before trying again."
                ),
                false,
                false,
            )),
        }
    }

    /// Lets the launcher's Try again rebuild the provider catalog, and hands
    /// the refresher this owner's service to swap the new catalog into.
    pub fn attach_provider_refresh(&self, refresh: Arc<crate::provider_refresh::ProviderRefresh>) {
        refresh.attach_terminal(self.service.clone());
        let _ = self.provider_refresh.set(refresh);
    }

    /// Shares exact file grants and the pasted-image store with output preview.
    pub fn attach_prompt_context(
        &self,
        prompt_context: Arc<interface_prompt_context::PromptContextService>,
    ) {
        let _ = self.prompt_context.set(prompt_context);
    }

    /// The same host-confirmed project candidates used by workspace tools.
    pub(crate) fn away_project_roots(&self) -> Result<Vec<PathBuf>, DispatchError> {
        workspace_root_candidates(&self.desktop, &self.service, self.runtime.as_ref())
    }
    async fn preview_output(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<
            interface_contracts::output_review::OutputPreviewControlRequest,
        >(CHANNELS.preview_output, payload)?;
        request.validate().map_err(|error| {
            DispatchError::new("request-refused", error.to_string(), false, true)
        })?;
        let permit = Arc::clone(&self.blocking_gate)
            .acquire_owned()
            .await
            .map_err(|_| terminal_closed_error())?;
        let desktop = Arc::clone(&self.desktop);
        let runtime = Arc::clone(&self.runtime);
        let service = self.service.clone();
        let prompt_context = self.prompt_context.get().cloned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut roots = workspace_root_candidates(&desktop, &service, runtime.as_ref())?;
            let mut files = desktop.retained_prompt_paths();
            if let Some(prompt_context) = prompt_context {
                files.extend(prompt_context.exact_grants());
                if let Some(dir) = prompt_context.pasted_image_directory() {
                    roots.push(dir);
                }
            }
            let response = match request {
                interface_contracts::output_review::OutputPreviewControlRequest::Inspect(request) => {
                    if request.action == interface_contracts::output_preview::OutputPreviewAction::Read {
                        roots.extend(desktop.agent_artifact_roots());
                    }
                    let response = interface_desktop_services::read_output_preview_with_files(&request, &roots, &files)
                        .map_err(|error| DispatchError::new("output-preview-refused",error.to_string(),false,true))?;
                    serde_json::to_value(response)
                }
                interface_contracts::output_review::OutputPreviewControlRequest::Review(request) => {
                    let response = desktop.output_review(&request, &roots)
                        .map_err(|error| DispatchError::new(error.code,error.message,error.retryable,
                            error.certainty == interface_desktop_services::DesktopEffectCertainty::DefinitelyUnsent))?;
                    serde_json::to_value(response)
                }
            };
            response.map_err(|_| DispatchError::new("output-preview-unavailable","The preview could not be encoded.",false,false))
        })
        .await
        .map_err(|_| {
            DispatchError::new(
                "output-preview-unavailable",
                "The preview reader stopped. Try opening the file again.",
                true,
                true,
            )
        })?
    }

    async fn open_workspace_tool(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<OpenWorkspaceToolV1>(CHANNELS.open_workspace_tool, payload)?;
        request.validate().map_err(|error| {
            DispatchError::new("request-refused", error.to_string(), false, true)
        })?;
        let permit = Arc::clone(&self.blocking_gate)
            .acquire_owned()
            .await
            .map_err(|_| terminal_closed_error())?;
        let desktop = Arc::clone(&self.desktop);
        let runtime = Arc::clone(&self.runtime);
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let candidates = workspace_root_candidates(&desktop, &service, runtime.as_ref())?;
            let approved = canonical_terminal_roots(candidates)?;
            open_workspace_tool(&request, &approved, &SystemWorkspaceToolLauncher).map_err(|error| match error {
                WorkspaceToolError::Refused(message) => DispatchError::new("request-refused", message, false, true),
                WorkspaceToolError::OutcomeUnknown(message) => DispatchError::new(OUTCOME_UNKNOWN_CODE, message, false, false),
            })?;
            Ok(Value::Null)
        }).await.map_err(|_| DispatchError::new(OUTCOME_UNKNOWN_CODE,
            "The workspace tool launch worker stopped. Check whether the app opened before trying again.", false, false))?
    }

    /// The retired pane-delegation view (2026-10-14). `readTerminalDelegation`
    /// stays wire protocol: a current terminal generation is answered
    /// unavailable, a stale one is refused, and nothing else happens.
    fn read_delegation(
        &self,
        request: &interface_contracts::delegation::ReadTerminalDelegation,
    ) -> Result<interface_contracts::delegation::TerminalDelegationView, DispatchError> {
        if self
            .visible_terminal(request.id)?
            .is_none_or(|terminal| terminal.generation != request.generation)
        {
            return Err(DispatchError::new(
                "terminal-generation-mismatch",
                "That terminal generation is no longer available",
                false,
                true,
            ));
        }
        Ok(interface_contracts::delegation::TerminalDelegationView {
            version: 3,
            available: false,
            workflows: vec![],
            detail: None,
        })
    }

    /// Applies the host's provider accounts (protocol 60) to later launches.
    pub fn install_accounts(&self, accounts: Arc<crate::provider_accounts::ProviderAccounts>) {
        if let Some(capture) = &self.capture_accounts {
            capture.install_accounts(Arc::clone(&accounts));
        }
        self.service.install_accounts(accounts);
    }

    /// Builds the production PTY owner. A missing binary data sender leaves
    /// read-only listing available but refuses process creation explicitly.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal service limits are invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn production(
        runtime: Arc<dyn NativeRuntimeView>,
        desktop: Arc<DesktopService>,
        worktree_release: Arc<dyn AutomaticWorktreeRelease>,
        pushes: mpsc::Sender<PushFrame>,
        data_sender: Option<TerminalDataSender>,
        launches: TerminalLaunchCatalog,
        claude_projects_root: PathBuf,
        claude_terminal_session_receipts_root: PathBuf,
    ) -> Result<Self, DispatchError> {
        Self::with_backend_launches_and_claude_store(
            runtime,
            desktop,
            worktree_release,
            pushes,
            data_sender,
            TerminalServiceConfig::default(),
            Arc::new(SystemTerminalPtyBackend::default()),
            launches,
            Some(ClaudeTerminalSessionStore {
                projects_root: claude_projects_root,
                receipts_root: claude_terminal_session_receipts_root,
            }),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_backend_launches_and_claude_store(
        runtime: Arc<dyn NativeRuntimeView>,
        desktop: Arc<DesktopService>,
        worktree_release: Arc<dyn AutomaticWorktreeRelease>,
        pushes: mpsc::Sender<PushFrame>,
        data_sender: Option<TerminalDataSender>,
        config: TerminalServiceConfig,
        backend: Arc<dyn TerminalPtyBackend>,
        launches: TerminalLaunchCatalog,
        claude_store: Option<ClaudeTerminalSessionStore>,
        provider_sessions: Option<Arc<dyn TerminalProviderSessionLookup>>,
    ) -> Result<Self, DispatchError> {
        let pending_managed_reopen_proofs = Arc::new(Mutex::new(BTreeMap::new()));
        let lifecycle = HostTerminalLifecycleState::default();
        let remote_hub = Arc::new(interface_remote::RemoteHub::default());
        let events = Arc::new(HostTerminalLifecycleSink {
            pushes,
            desktop: Arc::clone(&desktop),
            remote_hub: Arc::clone(&remote_hub),
            pending_managed_reopen_proofs: Arc::clone(&pending_managed_reopen_proofs),
            admission: Arc::clone(&lifecycle.admission),
            state: Mutex::new(lifecycle),
            input_lanes: Mutex::new(BTreeMap::new()),
            output_tails: Mutex::new(BTreeMap::new()),
            shell_handoffs: std::sync::OnceLock::new(),
            #[cfg(test)]
            durable_write_hook: Mutex::new(None),
        });
        let leases = Arc::new(DesktopTerminalLeaseProvider {
            desktop: Arc::clone(&desktop),
            worktree_release: Arc::clone(&worktree_release),
        });
        let mut parts = TerminalServiceParts::new(config, backend, leases, events.clone());
        let titles = Arc::new(DurableTerminalTitles(Mutex::new(
            interface_desktop_services::TerminalTitleStore::open(&desktop.terminal_titles_file())
                .map_err(|_| {
                DispatchError::new(
                    "terminal-name-unavailable",
                    "Saved pane names could not be read safely.",
                    false,
                    true,
                )
            })?,
        )));
        parts.title_persistence = Arc::<DurableTerminalTitles>::clone(&titles);
        parts.launches = launches;
        let mut capture_accounts = None;
        if let Some(provider_sessions) = provider_sessions {
            parts.provider_sessions = provider_sessions;
        } else if let Some(store) = claude_store.as_ref() {
            let lookup = Arc::new(
                crate::provider_capture::accounts::AccountCaptureLookup::new(
                    ProviderCaptureLookup::new(&store.projects_root, &store.receipts_root)
                        .map_err(|_| {
                            DispatchError::new(
                                "terminal-session-history-unavailable",
                                "Claude terminal session metadata could not be configured safely.",
                                false,
                                true,
                            )
                        })?,
                    &store.receipts_root,
                ),
            );
            capture_accounts = Some(Arc::clone(&lookup));
            parts.provider_sessions = lookup;
        }
        let service =
            TerminalService::new(parts).map_err(|error| terminal_dispatch_error(&error))?;
        desktop
            .repair_terminal_titles(|terminal| {
                service
                    .saved_terminal_title(terminal)
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| desktop_error(error, false))?;
        titles.forget_unsaved_panes(&desktop);
        Ok(Self {
            service,
            remote_hub,
            runtime,
            desktop,
            transport_available: AtomicBool::new(data_sender.is_some()),
            data_sender,
            blocking_gate: Arc::new(Semaphore::new(TERMINAL_BLOCKING_CONCURRENCY)),
            output_pumps: Mutex::new(BTreeMap::new()),
            input_pump: Mutex::new(None),
            failure_monitor: Mutex::new(None),
            lifecycle_dispatch_gate: Arc::new(tokio::sync::Mutex::new(())),
            claude_projects_root: claude_store.map(|store| store.projects_root),
            worktree_release,
            lifecycle_sink: events,
            pending_managed_reopen_proofs,
            closed: AtomicBool::new(false),
            provider_refresh: std::sync::OnceLock::new(),
            prompt_context: std::sync::OnceLock::new(),
            provider_canary: std::sync::OnceLock::new(),
            profile_versions: Mutex::new(BTreeMap::new()),
            shell_handoff: Mutex::new(None),
            capture_accounts,
        })
    }

    /// Runs lifecycle-transaction work on the blocking pool. Publish holds
    /// the lifecycle lock across durable sidecar writes and a blocking push
    /// send, so an async worker never waits on that lock itself.
    async fn lifecycle_blocking<T>(
        &self,
        operation: impl FnOnce(&HostTerminalLifecycleSink) -> Result<T, DispatchError> + Send + 'static,
    ) -> Result<T, DispatchError>
    where
        T: Send + 'static,
    {
        let sink = Arc::clone(&self.lifecycle_sink);
        tokio::task::spawn_blocking(move || operation(&sink))
            .await
            .map_err(|error| terminal_worker_error(&error))?
    }

    /// Runs a desktop terminal-state call on the blocking pool: the desktop's
    /// terminal-state gate is held across durable roster and sidecar writes.
    async fn desktop_blocking<T>(
        &self,
        operation: impl FnOnce(&DesktopService) -> T + Send + 'static,
    ) -> Result<T, DispatchError>
    where
        T: Send + 'static,
    {
        let desktop = Arc::clone(&self.desktop);
        tokio::task::spawn_blocking(move || operation(&desktop))
            .await
            .map_err(|error| terminal_worker_error(&error))
    }

    /// One visible pane, cloned alone: pane tools resolve a pane id this way
    /// instead of round-tripping every pane through JSON.
    pub(crate) fn visible_terminal(
        &self,
        id: TerminalId,
    ) -> Result<Option<TerminalSummary>, DispatchError> {
        let lifecycle = self
            .lifecycle_sink
            .state
            .lock()
            .map_err(|_| terminal_state_error())?;
        Ok(lifecycle.visible_terminals.get(&id).cloned())
    }

    pub(crate) fn visible_terminal_snapshot(&self) -> Result<TerminalSnapshot, DispatchError> {
        let lifecycle = self
            .lifecycle_sink
            .state
            .lock()
            .map_err(|_| terminal_state_error())?;
        Ok(TerminalSnapshot {
            revision: lifecycle.delivered_revision,
            terminals: lifecycle.visible_terminals.values().cloned().collect(),
        })
    }

    fn ensure_transport(&self) -> Result<TerminalDataSender, DispatchError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(terminal_closed_error());
        }
        if !self.transport_available.load(Ordering::Acquire) {
            return Err(terminal_transport_error());
        }
        self.data_sender
            .clone()
            .ok_or_else(terminal_transport_error)
    }

    async fn run_blocking<T, F>(&self, operation: F) -> Result<T, DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(TerminalService) -> Result<T, TerminalError> + Send + 'static,
    {
        let permit = Arc::clone(&self.blocking_gate)
            .acquire_owned()
            .await
            .map_err(|_| terminal_closed_error())?;
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(service).map_err(|error| terminal_dispatch_error(&error))
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, "terminal worker task failed");
            DispatchError::new(
                "terminal-worker-failed",
                "The integrated terminal worker stopped unexpectedly.",
                false,
                false,
            )
        })?
    }

    async fn set_terminal_workspace_activity(
        &self,
        payload: Value,
    ) -> Result<Value, DispatchError> {
        let request = decode_payload::<
            interface_contracts::terminal::TerminalWorkspaceActivityRequest,
        >(CHANNELS.set_terminal_workspace_activity, payload)?;
        encode_result(
            self.run_blocking(move |service| service.set_workspace_activity(&request))
                .await?,
        )
    }

    async fn resize_terminal(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<ResizeTerminalRequest>(CHANNELS.resize_terminal, payload)?;
        encode_result(
            self.run_blocking(move |service| service.resize(request))
                .await?,
        )
    }

    /// The running pane whose child process `pid` descends from, for the pane
    /// MCP server to name its caller. Reads one `ps` listing; see
    /// `TerminalService::terminal_for_process_ancestor`.
    pub(crate) fn terminal_for_process_ancestor(
        &self,
        pid: u32,
    ) -> Option<(TerminalId, TerminalGeneration)> {
        self.service.terminal_for_process_ancestor(pid)
    }

    /// The provider session each running pane is bound to, from memory; see
    /// `TerminalService::bound_provider_sessions`.
    pub(crate) fn bound_provider_sessions(&self) -> Vec<String> {
        self.service.bound_provider_sessions()
    }

    pub(crate) async fn report_worker_conversation(
        &self,
        request: TerminalGenerationRequest,
        peer_pid: u32,
    ) -> Result<(TerminalLaunchProfileId, String), DispatchError> {
        self.run_blocking(move |service| service.report_worker_conversation(request, peer_pid))
            .await
    }

    pub(crate) async fn delegate_directory(
        &self,
        request: TerminalGenerationRequest,
        peer_pid: u32,
        requested: Option<PathBuf>,
    ) -> Result<PathBuf, DispatchError> {
        let candidate = self
            .run_blocking(move |service| {
                service.delegate_candidate(request, peer_pid, requested.as_deref())
            })
            .await?;
        let mut roots = self.approved_roots(None).await?;
        if roots.owning_root(&candidate).is_err() {
            let candidate = candidate.clone();
            let approved = roots.clone();
            let related = tokio::task::spawn_blocking(move || {
                interface_registry::delegate_worktree::related_worktree(&candidate, |repository| {
                    approved.owning_root(repository).is_ok()
                })
            })
            .await
            .map_err(|_| {
                DispatchError::new(
                    "delegate-directory-unavailable",
                    "Worktree authorization did not finish; no worker was started.",
                    true,
                    true,
                )
            })?
            .map_err(|error| {
                DispatchError::new("delegate-directory-unapproved", error, false, true)
            })?;
            // This exact transient root is backed by the approved repository's
            // live Git worktree registration. No saved workspace grant changes.
            roots = self.approved_roots(Some(related)).await?;
        }
        self.run_blocking(move |service| {
            service.delegate_directory(request, peer_pid, &roots, Some(&candidate))
        })
        .await
    }
}

struct ClaudeTerminalSessionStore {
    projects_root: PathBuf,
    receipts_root: PathBuf,
}

#[async_trait]
impl HostDispatcher for TerminalHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation == CHANNELS.preview_output {
            return self.preview_output(payload).await;
        }
        if operation == CHANNELS.open_workspace_tool {
            return self.open_workspace_tool(payload).await;
        }
        self.prune_output_pumps()?;
        let _lifecycle_dispatch = if operation == CHANNELS.create_terminal
            || operation == CHANNELS.restart_terminal
            || operation == CHANNELS.close_terminal
            || operation == CHANNELS.dismiss_terminal
        {
            Some(self.lifecycle_dispatch_gate.lock().await)
        } else {
            None
        };
        if operation == CHANNELS.list_terminal_launch_profiles {
            return self.list_launch_profiles(&payload).await;
        }
        if operation == CHANNELS.inspect_terminal_servers {
            let request = decode_payload::<TerminalGenerationRequest>(operation, payload)?;
            return encode_result(
                self.run_blocking(move |service| service.server_links(request))
                    .await?,
            );
        }
        if operation == CHANNELS.list_terminals {
            decode_empty(operation, &payload)?;
            return encode_result(self.visible_terminal_snapshot()?);
        }
        if operation == CHANNELS.advance_terminal_controls {
            self.ensure_transport()?;
            let request = decode_payload::<
                interface_contracts::terminal_controls::TerminalControlsRequestV1,
            >(operation, payload)?;
            return encode_result(
                self.run_blocking(move |service| service.advance_controls(&request))
                    .await?,
            );
        }
        if operation == CHANNELS.read_terminal_delegation {
            let request: interface_contracts::delegation::ReadTerminalDelegation =
                decode_payload(operation, payload)?;
            return encode_result(self.read_delegation(&request)?);
        }
        if operation == CHANNELS.create_terminal {
            return self.create_terminal(payload).await;
        }
        if operation == CHANNELS.rename_terminal {
            let request = decode_payload::<RenameTerminalRequest>(operation, payload)?;
            return encode_result(
                self.run_blocking(move |service| service.rename(request))
                    .await?,
            );
        }
        if operation == CHANNELS.set_terminal_workspace_activity {
            return self.set_terminal_workspace_activity(payload).await;
        }
        if operation == CHANNELS.resize_terminal {
            return self.resize_terminal(payload).await;
        }
        if operation == CHANNELS.restart_terminal {
            return self.restart_terminal(payload).await;
        }
        if operation == CHANNELS.close_terminal {
            let request = decode_payload::<TerminalGenerationRequest>(operation, payload)?;
            self.ensure_reopen_authority_idle(request.id).await?;
            return encode_result(
                self.run_blocking(move |service| service.close(request))
                    .await?,
            );
        }
        if operation == CHANNELS.dismiss_terminal {
            return self.dismiss_terminal(payload).await;
        }
        Err(DispatchError::new(
            OPERATION_NOT_MIGRATED_CODE,
            "That operation is not owned by the integrated terminal service.",
            false,
            true,
        ))
    }
}

fn terminal_lifecycle_id(event: &TerminalLifecycleEvent) -> TerminalId {
    match event {
        TerminalLifecycleEvent::Upsert { terminal, .. } => terminal.id,
        TerminalLifecycleEvent::Removed { id, .. } => *id,
    }
}

fn encode_result<T>(value: T) -> Result<Value, DispatchError>
where
    T: Serialize,
{
    serde_json::to_value(value).map_err(|error| {
        tracing::error!(%error, "failed to encode an integrated terminal result");
        DispatchError::new(
            "terminal-result-encode-failed",
            "The Rust host could not encode the integrated terminal result.",
            false,
            true,
        )
    })
}

fn terminal_dispatch_error(error: &TerminalError) -> DispatchError {
    let (code, retryable, replay_safe) = match error {
        TerminalError::InvalidSize
        | TerminalError::InvalidTitle
        | TerminalError::InputTooLarge
        | TerminalError::UnsupportedLaunchVersion(_)
        | TerminalError::UnsupportedLaunchSessionVersion(_)
        | TerminalError::InvalidLaunchSessionId => (INVALID_PAYLOAD_CODE, false, true),
        TerminalError::UnavailableLaunchProfile(_)
        | TerminalError::UnbindableLaunchProfile(_)
        | TerminalError::NoSignIn(_)
        | TerminalError::AccountUnavailable(_) => {
            ("terminal-launch-profile-unavailable", false, true)
        }
        TerminalError::ProviderSessionLookupFailed(_) => {
            ("terminal-session-history-unavailable", true, true)
        }
        TerminalError::ProviderSessionCaptureUnavailable(_) => {
            // Capture can fail after the provider process has spawned. The
            // caller must not blindly replay an operation whose side effect
            // may already be running.
            ("terminal-session-capture-unavailable", false, false)
        }
        TerminalError::HandoffSourceUnavailable => {
            ("terminal-handoff-source-unavailable", false, true)
        }
        TerminalError::HandoffUnsupportedProfile(_) => {
            ("terminal-handoff-unsupported-profile", false, true)
        }
        TerminalError::InvalidHandoff => ("terminal-handoff-invalid", false, true),
        TerminalError::ShellHandoffUnavailable => {
            ("terminal-shell-handoff-unavailable", false, true)
        }
        TerminalError::UnsafeCwd | TerminalError::CwdChanged => {
            ("terminal-cwd-changed", false, true)
        }
        TerminalError::UnauthorizedCwd => ("terminal-cwd-not-approved", false, true),
        TerminalError::NotFound(_) => ("terminal-not-found", false, true),
        TerminalError::StaleGeneration { .. } => ("terminal-stale-generation", false, true),
        TerminalError::CreateConflict(_) | TerminalError::RetiredId(_) => {
            ("terminal-id-conflict", false, true)
        }
        TerminalError::NotRunning(_)
        | TerminalError::StillRunning(_)
        | TerminalError::StillRunningForRestart(_) => ("terminal-state-conflict", false, true),
        TerminalError::GenerationExhausted(_) => ("terminal-generation-exhausted", false, true),
        TerminalError::LimitReached(_) => ("terminal-limit-reached", true, true),
        TerminalError::OutputAlreadyClaimed(_) => ("terminal-output-unavailable", false, true),
        TerminalError::InvalidConfiguration => ("terminal-unavailable", false, true),
        TerminalError::Lease(_) => ("terminal-workspace-unavailable", true, true),
        TerminalError::Backend(_) => ("terminal-backend-failed", true, false),
    };
    DispatchError::new(code, error.to_string(), retryable, replay_safe)
}

fn terminal_worker_error(error: &tokio::task::JoinError) -> DispatchError {
    tracing::error!(%error, "terminal worker task failed");
    DispatchError::new(
        "terminal-worker-failed",
        "The integrated terminal worker stopped unexpectedly.",
        false,
        false,
    )
}

fn terminal_state_error() -> DispatchError {
    DispatchError::new(
        "terminal-state-unavailable",
        "Interface could not access its integrated terminal state.",
        false,
        true,
    )
}

fn terminal_transport_error() -> DispatchError {
    DispatchError::new(
        "terminal-data-unavailable",
        "The integrated terminal byte transport is unavailable.",
        true,
        true,
    )
}

fn terminal_closed_error() -> DispatchError {
    DispatchError::new(
        "terminal-host-closed",
        "The integrated terminal host is shutting down.",
        false,
        true,
    )
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;

impl interface_remote::RemoteBackend for TerminalHostDispatcher {
    fn workspaces(
        &self,
    ) -> Result<
        Vec<interface_contracts::terminal::TerminalWorkspaceRecord>,
        interface_remote::RemoteError,
    > {
        self.desktop.remote_terminal_workspaces().map_err(|_| {
            interface_remote::RemoteError::new(
                "workspaces-unavailable",
                "The workspace list is unavailable.",
                true,
            )
        })
    }
    fn snapshot(&self) -> Result<TerminalSnapshot, interface_remote::RemoteError> {
        self.visible_terminal_snapshot().map_err(|_| {
            interface_remote::RemoteError::new(
                "terminals-unavailable",
                "The terminal list is unavailable.",
                true,
            )
        })
    }

    fn enqueue(
        &self,
        key: interface_remote::replay::StreamKey,
        bytes: Vec<u8>,
    ) -> Result<(), interface_remote::RemoteError> {
        let frame = TerminalDataFrame::new(
            key.0,
            key.1,
            0,
            TerminalDataDirection::ElectronToHost,
            TerminalDataKind::Data,
            bytes,
        )
        .map_err(|_| {
            interface_remote::RemoteError::new(
                "input-invalid",
                "The terminal input is invalid.",
                true,
            )
        })?;
        self.enqueue_input_frame(frame).map_err(|_| {
            interface_remote::RemoteError::new(
                "input-refused",
                "The terminal changed or its input queue is full. No input was queued.",
                true,
            )
        })
    }
}

/// Code panes hold the provider sessions they run: the runtime and Threads
/// never drive one of those too (review C1).
impl interface_runtime::TerminalSessionBindings for TerminalHostDispatcher {
    fn bound_provider_sessions(&self) -> Vec<String> {
        self.service.bound_provider_sessions()
    }
}
