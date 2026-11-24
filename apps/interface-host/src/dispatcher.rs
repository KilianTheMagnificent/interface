use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, EngineId, LaunchOptions, ResumeOptions, SessionKind,
        SessionStatus, SessionSummary,
    },
    ipc::{CHANNELS, EngineProbe, HistoryPage, HistoryReadOptions, history_read_limit, parse_key},
};
use interface_engine::{BoundedEventStream, EffectCertainty};
use interface_registry::Worktree;
use interface_runtime::{
    BridgeApprovalResponse, BridgeApprovalVerdict, ProviderReadiness, RuntimeBridgeCommand,
    RuntimeBridgeResponse, RuntimeClock, RuntimeDeliveryDisposition, RuntimeDeliveryReceipt,
    RuntimeError, RuntimeErrorKind, RuntimeEventFrame, SessionOpenOutcome, SessionRuntime,
    SystemRuntimeClock,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{sync::mpsc, time::Instant};

pub use crate::registry_push::{RegistryRoundObserver, SharedRegistryPushes};
use crate::{
    collaboration::{CollaborationSessionHooks, ProjectedSendOutcome},
    payload,
    protocol::{BridgeError, PushFrame},
    registry_access::{self, runtime_registry_error},
    registry_push::{
        RegistryPushCoordinator, emit_registry_from_runtime, registry_push_frame,
        registry_push_round,
    },
    worktree::WorktreeService,
};

pub const OPERATION_NOT_MIGRATED_CODE: &str = "operation-not-migrated";
pub const INVALID_PAYLOAD_CODE: &str = "invalid-payload";
pub const DEFINITELY_UNSENT_CODE: &str = "effect-definitely-unsent";
pub const DEFINITELY_REJECTED_CODE: &str = "effect-definitely-rejected";
pub const OUTCOME_UNKNOWN_CODE: &str = "effect-outcome-unknown";
const WORKTREE_PUSH: &str = "iface:worktree";
const EVENT_PUMP_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a desktop pump waits for the runtime to evict a session whose
/// event queue overflowed. The runtime cancels a turn parked on the provider
/// before it closes, so this covers the provider ending that turn.
pub(crate) const OVERFLOW_DETACH_WAIT: Duration = Duration::from_secs(15);
pub const RUNTIME_BRIDGE_OPERATIONS: [&str; 13] = [
    CHANNELS.list_sessions,
    CHANNELS.probe_engines,
    CHANNELS.create_session,
    CHANNELS.resume_session,
    CHANNELS.send,
    CHANNELS.interrupt,
    CHANNELS.close_session,
    CHANNELS.respond,
    CHANNELS.set_model,
    CHANNELS.set_effort,
    CHANNELS.set_fast_mode,
    CHANNELS.set_permission_mode,
    CHANNELS.read_history,
];

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct DispatchError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub replay_safe: bool,
}

impl DispatchError {
    #[must_use]
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        replay_safe: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            replay_safe,
        }
    }
}

impl From<DispatchError> for BridgeError {
    fn from(error: DispatchError) -> Self {
        Self::new(
            error.code,
            error.message,
            error.retryable,
            error.replay_safe,
        )
    }
}

#[async_trait]
pub trait HostDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError>;
}

/// Injection seam between the frozen desktop protocol and the native runtime.
///
/// Tests provide a fake implementation; production uses [`SessionRuntime`].
#[async_trait]
pub trait RuntimeCommandHandler: Send + Sync + 'static {
    async fn handle_runtime_command(&self, command: RuntimeBridgeCommand) -> RuntimeBridgeResponse;

    /// Reads the provider-owned transcript rather than the Rust effect audit.
    ///
    /// # Errors
    ///
    /// Returns a replay-safe read failure without exposing provider-native paths.
    async fn read_provider_history(
        &self,
        session_key: &str,
        limit: u64,
    ) -> Result<HistoryPage, RuntimeError>;

    /// Reads one canonical registry record without contacting a provider.
    ///
    /// # Errors
    ///
    /// Returns a registry error when the durable session registry cannot be read.
    fn session_summary(&self, session_key: &str) -> Result<Option<SessionSummary>, RuntimeError>;

    /// Reads the canonical registry snapshot without contacting a provider.
    ///
    /// # Errors
    ///
    /// Returns a registry error when the durable session registry cannot be read.
    fn session_summaries(&self) -> Result<Vec<SessionSummary>, RuntimeError>;

    /// The registry's content revision for the two-rate registry push. `None`
    /// (a runtime without one) keeps every registry round at 50 ms.
    fn registry_content_revision(&self) -> Option<u64> {
        None
    }

    /// Reports whether the runtime currently owns an attachment for the key.
    ///
    /// # Errors
    ///
    /// Returns a history error when the durable attachment state cannot be read.
    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError>;

    /// Applies a host-projected lifecycle event to the canonical registry.
    ///
    /// # Errors
    ///
    /// Returns a registry error when the durable projection cannot be saved.
    fn project_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), RuntimeError>;

    /// Adds canonical worktree identity to a just-registered session.
    ///
    /// # Errors
    ///
    /// Returns a registry error when the durable projection cannot be saved.
    fn associate_worktree(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<(), RuntimeError>;

    async fn take_runtime_events(
        &self,
        session_key: &str,
    ) -> Result<BoundedEventStream<RuntimeEventFrame>, RuntimeError>;

    /// Pins a Chat launch onto this runtime's chat folder.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent invalid-input error when Chat is requested
    /// and this runtime has no chat folder configured.
    fn pin_chat_launch(&self, options: &mut LaunchOptions) -> Result<(), RuntimeError>;
}

/// Schedules the frozen best-effort managed-worktree release after a session
/// has definitively left the live runtime. The concrete owner also services
/// forget and editor-close retries; the runtime pump only needs this narrow
/// non-blocking seam.
#[async_trait]
pub trait AutomaticWorktreeRelease: Send + Sync + 'static {
    fn schedule_session_release(&self, session_key: String);

    fn schedule_workspace_release(&self, _workspace: String) {}

    /// Returns the exact durable managed-worktree root that owns `workspace`.
    /// The default grants no cold-restart authority; production derives this
    /// only from the host-owned managed-worktree ledger.
    fn managed_workspace_root(&self, _workspace: &std::path::Path) -> Option<std::path::PathBuf> {
        None
    }

    /// Re-proves the managed-worktree registration and re-reads the exact
    /// durable record before returning cold-restart authority. When an expected
    /// key is supplied, path reuse by a different managed session is refused.
    async fn verify_managed_workspace(
        &self,
        _workspace: &std::path::Path,
        _expected_key: Option<&str>,
    ) -> ManagedWorkspaceVerification {
        ManagedWorkspaceVerification::NotManaged
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedManagedWorkspace {
    pub root: std::path::PathBuf,
    pub key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManagedWorkspaceVerification {
    Verified(VerifiedManagedWorkspace),
    NotManaged,
    Unavailable,
}

#[async_trait]
impl RuntimeCommandHandler for SessionRuntime {
    async fn handle_runtime_command(&self, command: RuntimeBridgeCommand) -> RuntimeBridgeResponse {
        self.handle_bridge_command(command).await
    }

    async fn read_provider_history(
        &self,
        session_key: &str,
        limit: u64,
    ) -> Result<HistoryPage, RuntimeError> {
        SessionRuntime::read_provider_history(self, session_key, limit).await
    }

    fn session_summary(&self, session_key: &str) -> Result<Option<SessionSummary>, RuntimeError> {
        registry_access::read(self, |registry| registry.get(session_key).cloned())
    }

    fn session_summaries(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        registry_access::sorted_visible(self)
    }

    fn registry_content_revision(&self) -> Option<u64> {
        registry_access::content_revision(self)
    }

    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError> {
        self.session_is_live(session_key)
    }

    fn project_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), RuntimeError> {
        registry_access::apply_event(self, session_key, event)
    }

    fn associate_worktree(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<(), RuntimeError> {
        registry_access::write(self, |registry, revision| {
            let mut summary = registry.get(session_key).cloned().ok_or_else(|| {
                RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Registry,
                    format!("new session {session_key} is absent from the registry"),
                    false,
                )
            })?;
            summary.worktree_home = Some(worktree.source_cwd.to_string_lossy().into_owned());
            summary.worktree_path = Some(worktree.session_cwd.to_string_lossy().into_owned());
            summary.project_root = Some(worktree.repo_root.to_string_lossy().into_owned());
            let engine = summary.engine.clone();
            registry
                .upsert_all(revision, &engine, [summary])
                .map(Some)
                .map_err(|error| runtime_registry_error(&error))
        })
        .map(|_| ())
    }

    async fn take_runtime_events(
        &self,
        session_key: &str,
    ) -> Result<BoundedEventStream<RuntimeEventFrame>, RuntimeError> {
        self.take_session_events(session_key).await
    }

    fn pin_chat_launch(&self, options: &mut LaunchOptions) -> Result<(), RuntimeError> {
        SessionRuntime::pin_chat_launch(self, options)
    }
}

/// Read-only tap on the live session event stream, called by the event pump
/// after the collaboration projection and before the frozen desktop push.
pub trait SessionEventObserver: Send + Sync + 'static {
    fn observe_session_event(&self, session_key: &str, event: &EngineEvent);
}

/// How a session may be resumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeRequirement {
    /// No launch requirement: resume with the registry's options.
    Unrestricted,
    /// Resume behind the approval gate with exactly these extra folders (empty means none).
    Gated(Vec<String>),
    /// Resume in the agent's own provider mode with exactly these extra folders.
    Access(interface_contracts::engine::AgentAccess, Vec<String>),
    /// Never resume; the message says why. Refused before any provider contact.
    Refused(String),
}

/// Launch requirements a session keeps across every resume this dispatcher
/// performs, including the implicit resume before a send or setting change on
/// a detached session.
pub trait ResumeRequirements: Send + Sync + 'static {
    /// `registry_cwd` is the working folder the session registry recorded.
    fn resume_requirement(&self, session_key: &str, registry_cwd: &str) -> ResumeRequirement;
}

/// The permission mode the owner chose for each session outside the agent
/// roster, kept across restarts so that a resume starts in it rather than in
/// the provider's default. Agents keep theirs through [`ResumeRequirements`].
pub trait SessionAccessMemory: Send + Sync + 'static {
    /// The mode last chosen for the session; `None` is its default.
    fn mode(&self, session_key: &str) -> Option<String>;
    /// Records the mode a launch or a change gave the session; `None` or
    /// `default` forgets it. Blocking: it writes the store.
    fn remember(&self, session_key: &str, mode: Option<&str>);
    /// Protocol 68: a counter that moves whenever a remembered mode may have
    /// changed, so the registry push that carries the modes is sent again.
    fn revision(&self) -> u64 {
        0
    }
}

/// The threads team tools started (protocol 57). They launch at collaboration
/// depth 1, as collaboration peers do, and every resume keeps them there.
pub trait TeamChildren: Send + Sync + 'static {
    fn is_team_child(&self, session_key: &str) -> bool;
}

/// Work the host does around a send it forwards to a provider. The thread
/// host uses it to recreate a missing worktree and checkpoint and record the
/// turn before anything is resumed or sent.
#[async_trait]
pub trait SendHooks: Send + Sync + 'static {
    /// Runs before this dispatcher resumes or sends anything for
    /// `session_key`, including the implicit resume of a cold session. An
    /// error refuses the send and must be definitely unsent.
    async fn before_send(
        &self,
        session_key: &str,
        text: &str,
    ) -> Result<Option<Box<dyn SendTicket>>, DispatchError>;

    /// An interrupt for `session_key` is about to reach its provider.
    fn interrupt_requested(&self, _session_key: &str) {}
}

/// A send [`SendHooks::before_send`] prepared for. The dispatcher settles
/// every ticket once the send's delivery is known, whatever the outcome.
pub trait SendTicket: Send {
    fn settle(self: Box<Self>, settlement: SendSettlement);
    /// The host itself sent this prompt (an Away loop's round), so no window
    /// echoes it: the desktop is shown it as a live `user.message`, before
    /// the send, as a window shows its own prompt above the answer.
    fn echoes_prompt(&self) -> bool {
        false
    }
}

/// What became of a send, as its [`SendTicket`] is told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendSettlement {
    /// The provider got it, or may have (an unknown outcome).
    Delivered,
    /// It never reached the provider.
    Unsent,
    /// The provider refused it. A provider that answers a prompt only when
    /// its turn ends (ACP's `session/prompt`) may have worked on it first.
    Rejected,
}

/// History for a session whose provider keeps none: the thread journal
/// answers `readHistory` when the provider says its history is unavailable.
#[async_trait]
pub trait HistoryFallback: Send + Sync + 'static {
    /// The page to show instead, or `None` to keep the provider's answer.
    async fn history(&self, session_key: &str, limit: u64) -> Option<HistoryPage>;

    /// The page to show without asking the provider at all: a thread whose
    /// provider was switched reads as one conversation from its journal
    /// (T3 section 5). `None` asks the provider as usual.
    async fn chain_history(&self, _session_key: &str, _limit: u64) -> Option<HistoryPage> {
        None
    }

    /// A provider's own page with the visuals Interface showed in it (T3
    /// section 8), which no provider records.
    async fn with_visuals(&self, _session_key: &str, page: HistoryPage) -> HistoryPage {
        page
    }
}

/// Optional gate fields a `resumeSession` payload may carry.
#[derive(Default)]
struct RequestedResumeGate {
    approval_gate: Option<bool>,
    additional_directories: Option<Vec<String>>,
}

/// Dispatcher for lifecycle operations that have canonical runtime commands.
///
/// Request payloads are strict camel-case objects. The retained Electron preload
/// translates legacy positional calls into these objects before writing NDJSON.
pub struct RuntimeHostDispatcher<R> {
    runtime: Arc<R>,
    pushes: Option<mpsc::Sender<PushFrame>>,
    registry_pushes: Option<RegistryPushCoordinator>,
    worktrees: Option<Arc<dyn WorktreeService>>,
    worktree_release: Option<Arc<dyn AutomaticWorktreeRelease>>,
    collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
    event_observer: Option<Arc<dyn SessionEventObserver>>,
    resume_requirements: Option<Arc<dyn ResumeRequirements>>,
    session_access: Option<Arc<dyn SessionAccessMemory>>,
    /// Threads team tools started (protocol 57): they resume one hop down.
    team_children: Option<Arc<dyn TeamChildren>>,
    /// Who else drives a provider session (a Code pane, a hand-off to one):
    /// such a session is never resumed or sent to from here (review C1).
    session_claims: Option<Arc<interface_runtime::SessionClaims>>,
    /// `probeEngines {refresh: true}` finds the provider CLIs again first.
    provider_refresh: Option<Arc<crate::provider_refresh::ProviderRefresh>>,
    send_hooks: Option<Arc<dyn SendHooks>>,
    prompt_recovery: Option<Arc<interface_desktop_services::DesktopService>>,
    /// Told when a session listing changed the registry, which schedules no
    /// registry push of its own (the renderer reads the answer).
    listing_observer: Option<Arc<dyn RegistryRoundObserver>>,
    history_fallback: Option<Arc<dyn HistoryFallback>>,
    event_pumps: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

enum RuntimeDispatchRequest {
    Command(Box<RuntimeBridgeCommand>),
    Create {
        engine: EngineId,
        options: Box<LaunchOptions>,
        recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
    },
    Resume {
        session_key: String,
        gate: RequestedResumeGate,
    },
    Send {
        session_key: String,
        text: String,
        recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
    },
    Interrupt {
        session_key: String,
    },
    Close {
        session_key: String,
    },
    SetModel {
        session_key: String,
        model: Option<String>,
    },
    SetEffort {
        session_key: String,
        effort: Option<String>,
    },
    SetFastMode {
        session_key: String,
        enabled: bool,
    },
    SetPermissionMode {
        session_key: String,
        mode: String,
    },
    History {
        session_key: String,
        limit: u64,
    },
}

impl<R> RuntimeHostDispatcher<R> {
    #[must_use]
    pub fn new(runtime: Arc<R>) -> Self {
        Self {
            runtime,
            pushes: None,
            registry_pushes: None,
            worktrees: None,
            worktree_release: None,
            collaboration: None,
            event_observer: None,
            resume_requirements: None,
            session_access: None,
            team_children: None,
            session_claims: None,
            provider_refresh: None,
            send_hooks: None,
            prompt_recovery: None,
            listing_observer: None,
            history_fallback: None,
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[must_use]
    pub fn with_pushes(runtime: Arc<R>, pushes: mpsc::Sender<PushFrame>) -> Self
    where
        R: RuntimeCommandHandler,
    {
        let mut dispatcher = Self::new(Arc::clone(&runtime));
        let revision_runtime = Arc::clone(&runtime);
        dispatcher.registry_pushes = Some(RegistryPushCoordinator::start_registry(
            move || registry_push_round(runtime.session_summaries().map(Some)),
            move || revision_runtime.registry_content_revision(),
            pushes.clone(),
        ));
        dispatcher.pushes = Some(pushes);
        dispatcher
    }

    #[must_use]
    pub fn with_pushes_and_worktrees(
        runtime: Arc<R>,
        pushes: mpsc::Sender<PushFrame>,
        worktrees: Arc<dyn WorktreeService>,
    ) -> Self
    where
        R: RuntimeCommandHandler,
    {
        let mut dispatcher = Self::with_pushes(runtime, pushes);
        dispatcher.worktrees = Some(worktrees);
        dispatcher
    }

    /// Uses the host's shared registry coalescer instead of this dispatcher's
    /// own (which, no longer referenced, stops).
    #[must_use]
    pub fn with_shared_registry_pushes(mut self, shared: SharedRegistryPushes) -> Self {
        self.registry_pushes = Some(shared.0);
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

    #[must_use]
    pub fn with_collaboration_hooks(
        mut self,
        collaboration: Arc<dyn CollaborationSessionHooks>,
    ) -> Self {
        self.collaboration = Some(collaboration);
        self
    }

    #[must_use]
    pub fn with_event_observer(mut self, observer: Arc<dyn SessionEventObserver>) -> Self {
        self.event_observer = Some(observer);
        self
    }

    #[must_use]
    pub fn with_resume_requirements(mut self, requirements: Arc<dyn ResumeRequirements>) -> Self {
        self.resume_requirements = Some(requirements);
        self
    }

    /// Models & providers → Refresh rebuilds the provider catalog before it probes.
    #[must_use]
    pub fn with_provider_refresh(
        mut self,
        refresh: Arc<crate::provider_refresh::ProviderRefresh>,
    ) -> Self {
        self.provider_refresh = Some(refresh);
        self
    }

    #[must_use]
    pub fn with_session_access(mut self, memory: Arc<dyn SessionAccessMemory>) -> Self {
        self.session_access = Some(memory);
        self
    }

    /// Threads team tools started resume at collaboration depth 1, so they
    /// never get team tools or the collaboration MCP (exactly one level).
    #[must_use]
    pub fn with_team_children(mut self, children: Arc<dyn TeamChildren>) -> Self {
        self.team_children = Some(children);
        self
    }

    /// One writer per provider session: refuse to resume or send to a session
    /// a Code pane drives or is about to.
    #[must_use]
    pub fn with_session_claims(mut self, claims: Arc<interface_runtime::SessionClaims>) -> Self {
        self.session_claims = Some(claims);
        self
    }

    fn claimed_elsewhere(&self, session_key: &str) -> Option<RuntimeError> {
        self.session_claims
            .as_ref()
            .and_then(|claims| claims.refusal(session_key))
    }

    #[must_use]
    pub fn with_prompt_recovery(
        mut self,
        desktop: Arc<interface_desktop_services::DesktopService>,
    ) -> Self {
        self.prompt_recovery = Some(desktop);
        self
    }

    #[must_use]
    pub fn with_send_hooks(mut self, hooks: Arc<dyn SendHooks>) -> Self {
        self.send_hooks = Some(hooks);
        self
    }

    /// Tells `observer` after a `listSessions` that changed the registry
    /// (sessions found in the providers' stores), as the shared coalescer
    /// tells it after a round, without adding a registry push.
    #[must_use]
    pub fn with_listing_observer(mut self, observer: Arc<dyn RegistryRoundObserver>) -> Self {
        self.listing_observer = Some(observer);
        self
    }

    /// Answers `readHistory` from `fallback` when the provider's history is
    /// unavailable (protocol 56).
    #[must_use]
    pub fn with_history_fallback(mut self, fallback: Arc<dyn HistoryFallback>) -> Self {
        self.history_fallback = Some(fallback);
        self
    }
}

impl<R> RuntimeHostDispatcher<R>
where
    R: RuntimeCommandHandler,
{
    async fn start_event_pump(&self, session_key: &str) -> Result<(), DispatchError> {
        let events = self
            .runtime
            .take_runtime_events(session_key)
            .await
            .map_err(runtime_dispatch_error)?;
        let runtime = Arc::clone(&self.runtime);
        let pushes = self.pushes.clone();
        let registry_pushes = self.registry_pushes.clone();
        let worktree_release = self.worktree_release.clone();
        let collaboration = self.collaboration.clone();
        let event_observer = self.event_observer.clone();
        let session_key = session_key.to_owned();
        let task = tokio::spawn(async move {
            pump_runtime_events(
                runtime,
                pushes,
                registry_pushes,
                worktree_release,
                collaboration,
                event_observer,
                session_key,
                events,
            )
            .await;
        });
        if let Ok(mut tasks) = self.event_pumps.lock() {
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        } else {
            task.abort();
            return Err(DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                "The provider is live, but its desktop event owner is unavailable.",
                false,
                false,
            ));
        }
        Ok(())
    }

    /// Drains every host-side projection after the provider runtime has closed
    /// its ingress. This is the shutdown counterpart to the frozen bridge's
    /// pump-task drain and ensures terminal worktree releases are scheduled
    /// before their owner is closed.
    pub async fn close_event_pumps(&self) {
        loop {
            let tasks = match self.event_pumps.lock() {
                Ok(mut tasks) => std::mem::take(&mut *tasks),
                Err(_) => return,
            };
            if tasks.is_empty() {
                return;
            }
            for mut task in tasks {
                if tokio::time::timeout(EVENT_PUMP_DRAIN_TIMEOUT, &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
    }

    async fn attach_desktop_projection_best_effort(&self, session_key: &str, action: &str) {
        if let Err(error) = self.start_event_pump(session_key).await {
            tracing::warn!(
                session_key,
                action,
                code = %error.code,
                "provider is live but its desktop event projection could not attach"
            );
        }
        if let Err(error) = self.emit_registry_snapshot().await {
            tracing::warn!(
                session_key,
                action,
                code = %error.code,
                "provider is live but its registry push could not be emitted"
            );
        }
    }

    async fn guard_request(&self, request: &RuntimeDispatchRequest) -> Result<(), DispatchError> {
        let Some(collaboration) = &self.collaboration else {
            return Ok(());
        };
        let target = match request {
            RuntimeDispatchRequest::Resume { session_key, .. } => {
                Some((session_key.as_str(), "resume", false))
            }
            RuntimeDispatchRequest::Send { session_key, .. } => {
                Some((session_key.as_str(), "send", false))
            }
            RuntimeDispatchRequest::Interrupt { session_key } => {
                Some((session_key.as_str(), "interrupt", true))
            }
            RuntimeDispatchRequest::Close { session_key } => {
                Some((session_key.as_str(), "safe stop", true))
            }
            RuntimeDispatchRequest::SetModel { session_key, .. } => {
                Some((session_key.as_str(), "model change", false))
            }
            RuntimeDispatchRequest::SetEffort { session_key, .. } => {
                Some((session_key.as_str(), "effort change", false))
            }
            RuntimeDispatchRequest::SetFastMode { session_key, .. } => {
                Some((session_key.as_str(), "fast-mode change", false))
            }
            RuntimeDispatchRequest::SetPermissionMode { session_key, .. } => {
                Some((session_key.as_str(), "access change", false))
            }
            RuntimeDispatchRequest::Command(command) => runtime_command_guard(command),
            RuntimeDispatchRequest::Create { .. } | RuntimeDispatchRequest::History { .. } => None,
        };
        let Some((session_key, action, allow_safe_stop)) = target else {
            return Ok(());
        };
        collaboration
            .guard_session_mutation(session_key, action, allow_safe_stop)
            .await
    }

    /// Announces a registry change. With the shared coalescer this joins its
    /// 50 ms window, as the 0.1.169 bridge's `registry.onChange` debounce did;
    /// a whole-registry frame per send, approval or setting change multiplied
    /// the largest push the desktop receives.
    async fn emit_registry_snapshot(&self) -> Result<(), DispatchError> {
        if let Some(registry_pushes) = &self.registry_pushes {
            return if registry_pushes.schedule() {
                Ok(())
            } else {
                Err(registry_unreachable())
            };
        }
        let Some(pushes) = &self.pushes else {
            return Ok(());
        };
        let mut sessions = self
            .runtime
            .session_summaries()
            .map_err(runtime_dispatch_error)?;
        crate::session_access::mark_permission_modes(&mut sessions, self.session_access.as_ref());
        let frame = registry_push_frame(&sessions)?;
        pushes.send(frame).await.map_err(|_| registry_unreachable())
    }

    async fn resume_session(
        &self,
        session_key: String,
        requested: RequestedResumeGate,
    ) -> RuntimeBridgeResponse {
        let (engine, provider_session_id) = match parse_key(&session_key) {
            Ok(parsed) => parsed,
            Err(error) => {
                return runtime_input_error(format!("Cannot resume {session_key}: {error}."));
            }
        };
        let summary = match self.runtime.session_summary(&session_key) {
            Ok(Some(summary)) => summary,
            Ok(None) => {
                return runtime_input_error(format!(
                    "Cannot resume {session_key}: no record of that session is in the registry."
                ));
            }
            Err(error) => return RuntimeBridgeResponse::Error(error),
        };
        if summary.engine != engine || summary.session_id != provider_session_id {
            return RuntimeBridgeResponse::Error(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Registry,
                format!("Cannot resume {session_key}: its registry identity is inconsistent."),
                false,
            ));
        }
        if let Some(refusal) = self.claimed_elsewhere(&session_key) {
            return RuntimeBridgeResponse::Error(refusal);
        }
        let requirement = self
            .resume_requirements
            .as_ref()
            .map_or(ResumeRequirement::Unrestricted, |requirements| {
                requirements.resume_requirement(&session_key, &summary.cwd)
            });
        if let ResumeRequirement::Refused(message) = requirement {
            return RuntimeBridgeResponse::Error(RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                message,
                false,
            ));
        }
        let cwd = match resume_cwd(&session_key, &summary).await {
            Ok(cwd) => cwd,
            Err(error) => return RuntimeBridgeResponse::Error(error),
        };
        let mut options = ResumeOptions {
            setting_sources: summary.setting_sources,
            disallowed_tools: summary.disallowed_tools,
            model: concrete_setting(summary.model.as_deref()),
            effort: concrete_setting(
                summary
                    .effort
                    .as_ref()
                    .and_then(std::option::Option::as_deref),
            ),
            // A thread team tools started stays one hop down.
            collaboration_depth: self
                .team_children
                .as_ref()
                .is_some_and(|children| children.is_team_child(&session_key))
                .then_some(1),
            kind: summary.kind,
            approval_gate: requested.approval_gate,
            additional_directories: requested.additional_directories,
            agent_access: None,
            permission_mode: None,
        };
        // An agent session resumes only as its agent's settings say now, whichever caller
        // asked: behind the gate, or in the provider mode the owner chose.
        match requirement {
            ResumeRequirement::Gated(directories) => {
                options.approval_gate = Some(true);
                options.additional_directories = (!directories.is_empty()).then_some(directories);
            }
            ResumeRequirement::Access(access, directories) => {
                options.approval_gate = None;
                options.agent_access = Some(access);
                options.additional_directories = (!directories.is_empty()).then_some(directories);
            }
            // Any other session resumes in the mode its owner last chose for it.
            ResumeRequirement::Unrestricted | ResumeRequirement::Refused(_) => {
                if options.approval_gate != Some(true) && options.kind != Some(SessionKind::Agent) {
                    options.permission_mode = self
                        .session_access
                        .as_ref()
                        .and_then(|memory| memory.mode(&session_key));
                }
            }
        }
        self.runtime
            .handle_runtime_command(RuntimeBridgeCommand::ResumeSession {
                engine,
                provider_session_id,
                cwd,
                options,
            })
            .await
    }

    async fn ensure_effect_session(
        &self,
        session_key: &str,
    ) -> Result<(bool, Option<SessionSummary>), DispatchError> {
        // Live or not: a session a Code pane drives takes no effect from here.
        if let Some(refusal) = self.claimed_elsewhere(session_key) {
            return Err(runtime_dispatch_error(refusal));
        }
        if self
            .runtime
            .session_is_active(session_key)
            .map_err(runtime_dispatch_error)?
        {
            return Ok((false, None));
        }
        let summary = self
            .runtime
            .session_summary(session_key)
            .map_err(runtime_dispatch_error)?
            .ok_or_else(|| {
                runtime_dispatch_error(RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Registry,
                    format!(
                        "Cannot resume {session_key}: no record of that session is in the registry."
                    ),
                    false,
                ))
            })?;
        let response = self
            .resume_session(session_key.to_owned(), RequestedResumeGate::default())
            .await;
        match response {
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(session)) => {
                if session.key != session_key {
                    return Err(DispatchError::new(
                        OUTCOME_UNKNOWN_CODE,
                        "The provider resumed under a different session identifier.",
                        false,
                        false,
                    ));
                }
                self.attach_desktop_projection_best_effort(session_key, "resume")
                    .await;
                Ok((true, Some(summary)))
            }
            RuntimeBridgeResponse::SessionOpen(
                SessionOpenOutcome::DefinitelyUnsent(error)
                | SessionOpenOutcome::DefinitelyRejected(error)
                | SessionOpenOutcome::OutcomeUnknown(error),
            )
            | RuntimeBridgeResponse::Error(error) => Err(runtime_dispatch_error(error)),
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::ProviderReadyLocalFailure {
                provider,
                error,
            }) => Err(DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                format!(
                    "Provider session {} became ready, but local registration failed: {}",
                    provider.session_id, error.message
                ),
                false,
                false,
            )),
            _ => Err(runtime_response_mismatch(CHANNELS.resume_session)),
        }
    }

    async fn deliver_command(
        &self,
        operation: &str,
        command: RuntimeBridgeCommand,
    ) -> Result<(), DispatchError> {
        match self.runtime.handle_runtime_command(command).await {
            RuntimeBridgeResponse::Delivery(receipt) => {
                delivery_result(receipt)?;
                Ok(())
            }
            RuntimeBridgeResponse::Error(error) => Err(runtime_dispatch_error(error)),
            _ => Err(runtime_response_mismatch(operation)),
        }
    }

    /// Resumes a cold session first, delivers one command, then publishes
    /// the registry: the ordering every setting change shares. Returns
    /// whether it resumed.
    async fn resumed_delivery(
        &self,
        session_key: &str,
        operation: &str,
        command: RuntimeBridgeCommand,
    ) -> Result<bool, DispatchError> {
        let (resumed, _) = self.ensure_effect_session(session_key).await?;
        self.deliver_command(operation, command).await?;
        self.emit_registry_snapshot().await?;
        Ok(resumed)
    }

    /// Projects an event the host made for a session with no live provider:
    /// onto the registry, into collaboration's view, to the desktop, then the
    /// registry push.
    async fn project_synthetic(
        &self,
        session_key: &str,
        event: EngineEvent,
    ) -> Result<(), DispatchError> {
        self.runtime
            .project_event(session_key, &event)
            .map_err(runtime_dispatch_error)?;
        if let Some(collaboration) = &self.collaboration {
            collaboration.observe_event(session_key, &event)?;
            collaboration.observe_registry_change(session_key)?;
        }
        self.emit_session_event(session_key, event).await?;
        self.emit_registry_snapshot().await
    }

    async fn send_transaction(
        &self,
        session_key: String,
        text: String,
        recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
    ) -> Result<Value, DispatchError> {
        let receipt = if let Some(input) = recovery {
            let desktop = self.prompt_recovery.clone().ok_or_else(|| {
                DispatchError::new(
                    "prompt-recovery-unavailable",
                    "Prompt recovery is unavailable; nothing was sent.",
                    false,
                    true,
                )
            })?;
            let key = session_key.clone();
            let serialized = text.clone();
            Some(
                tokio::task::spawn_blocking(move || {
                    desktop.reserve_prompt_recovery(&key, input, &serialized)
                })
                .await
                .map_err(|_| {
                    DispatchError::new(
                        "prompt-recovery-unavailable",
                        "Prompt recovery stopped before delivery; nothing was sent.",
                        false,
                        true,
                    )
                })?
                .map_err(|e| DispatchError::new(e.code, e.message, false, true))?,
            )
        } else {
            None
        };
        let ticket = match &self.send_hooks {
            Some(hooks) => match hooks.before_send(&session_key, &text).await {
                Ok(ticket) => ticket,
                Err(error) => {
                    self.settle_recovery(
                        &session_key,
                        receipt.as_deref(),
                        interface_contracts::recovery::PromptDelivery::Unsent,
                    )
                    .await;
                    return Err(error);
                }
            },
            None => None,
        };
        if ticket.as_ref().is_some_and(|ticket| ticket.echoes_prompt()) {
            let event = EngineEvent {
                at: None,
                event: EngineEventKind::UserMessage {
                    text: text.clone(),
                    agent_key: None,
                    images: None,
                    image_references: None,
                },
            };
            // Display only: a prompt the desktop missed changes no outcome.
            if let Err(error) = self.emit_session_event(&session_key, event).await {
                tracing::warn!(code = %error.code, "a host prompt was not shown in its pane");
            }
        }
        let mut issued = false;
        let result = self
            .resumed_send(session_key.clone(), text, &mut issued)
            .await;
        let delivery = match &result {
            _ if !issued => interface_contracts::recovery::PromptDelivery::Unsent,
            Err(error) if error.code == DEFINITELY_UNSENT_CODE => {
                interface_contracts::recovery::PromptDelivery::Unsent
            }
            Ok(_) => interface_contracts::recovery::PromptDelivery::Accepted,
            _ => interface_contracts::recovery::PromptDelivery::Unknown,
        };
        self.settle_recovery(&session_key, receipt.as_deref(), delivery)
            .await;
        if let Some(ticket) = ticket {
            ticket.settle(match &result {
                _ if !issued => SendSettlement::Unsent,
                Err(error) if error.code == DEFINITELY_UNSENT_CODE => SendSettlement::Unsent,
                Err(error) if error.code == DEFINITELY_REJECTED_CODE => SendSettlement::Rejected,
                _ => SendSettlement::Delivered,
            });
        }
        result
    }

    /// `readHistory`: a switched thread's chain from the journal; else the
    /// provider's page with the visuals Interface showed in it (T3 section
    /// 8), or the journal when the provider keeps no history.
    async fn history_page(&self, session_key: &str, limit: u64) -> Result<Value, DispatchError> {
        use interface_contracts::ipc::HistoryAvailability;
        if let Some(fallback) = &self.history_fallback
            && let Some(chain) = fallback.chain_history(session_key, limit).await
        {
            return encode_legacy_result(chain);
        }
        let mut page = self
            .runtime
            .read_provider_history(session_key, limit)
            .await
            .map_err(runtime_dispatch_error)?;
        if let Some(fallback) = &self.history_fallback {
            if page.availability == Some(HistoryAvailability::Unavailable) {
                if let Some(journal) = fallback.history(session_key, limit).await {
                    page = journal;
                }
            } else {
                page = fallback.with_visuals(session_key, page).await;
            }
        }
        encode_legacy_result(page)
    }

    async fn settle_recovery(
        &self,
        key: &str,
        id: Option<&str>,
        delivery: interface_contracts::recovery::PromptDelivery,
    ) {
        let (Some(desktop), Some(id)) = (self.prompt_recovery.clone(), id) else {
            return;
        };
        let key = key.to_owned();
        let id = id.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            desktop.settle_prompt_recovery(&key, &id, delivery)
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!(
                "prompt recovery settlement could not be persisted; receipt stays unknown"
            );
        }
    }

    /// The send itself: resume a cold session, restore its model and
    /// effort, deliver. `issued` turns true once the send went to the runtime.
    async fn resumed_send(
        &self,
        session_key: String,
        text: String,
        issued: &mut bool,
    ) -> Result<Value, DispatchError> {
        let (resumed, summary) = self.ensure_effect_session(&session_key).await?;
        if resumed {
            let summary = summary.expect("a resumed effect has a canonical summary");
            if !summary.engine.as_str().starts_with("acp:") {
                if let Some(model) = concrete_setting(summary.model.as_deref()) {
                    self.deliver_command(
                        CHANNELS.set_model,
                        RuntimeBridgeCommand::SetModel {
                            session_key: session_key.clone(),
                            model: Some(model),
                        },
                    )
                    .await?;
                }
                if let Some(effort) = concrete_setting(
                    summary
                        .effort
                        .as_ref()
                        .and_then(std::option::Option::as_deref),
                ) {
                    self.deliver_command(
                        CHANNELS.set_effort,
                        RuntimeBridgeCommand::SetEffort {
                            session_key: session_key.clone(),
                            effort: Some(effort),
                        },
                    )
                    .await?;
                }
            }
        }
        if let Some(collaboration) = &self.collaboration {
            collaboration.user_send_started(&session_key)?;
        }
        *issued = true;
        let delivery = self
            .deliver_command(
                CHANNELS.send,
                RuntimeBridgeCommand::Send {
                    session_key: session_key.clone(),
                    text: text.clone(),
                },
            )
            .await;
        match delivery {
            Ok(()) => {
                if let Some(collaboration) = &self.collaboration {
                    collaboration.user_send_settled(
                        &session_key,
                        &text,
                        ProjectedSendOutcome::AdapterAccepted,
                    )?;
                }
            }
            Err(error) => {
                if let Some(collaboration) = &self.collaboration {
                    let outcome = if matches!(
                        error.code.as_str(),
                        DEFINITELY_UNSENT_CODE | DEFINITELY_REJECTED_CODE
                    ) {
                        ProjectedSendOutcome::DefinitelyUnsent
                    } else {
                        ProjectedSendOutcome::OutcomeUnknown
                    };
                    if let Err(projection_error) =
                        collaboration.user_send_settled(&session_key, &text, outcome)
                    {
                        tracing::error!(
                            code = %projection_error.code,
                            "collaboration source send projection failed after provider delivery failure"
                        );
                    }
                }
                return Err(error);
            }
        }
        self.emit_registry_snapshot().await?;
        Ok(Value::Null)
    }

    async fn set_model_transaction(
        &self,
        session_key: String,
        model: Option<String>,
    ) -> Result<Value, DispatchError> {
        let resumed = self
            .resumed_delivery(
                &session_key,
                CHANNELS.set_model,
                RuntimeBridgeCommand::SetModel {
                    session_key: session_key.clone(),
                    model,
                },
            )
            .await?;
        Ok(serde_json::json!({"key": session_key, "resumed": resumed}))
    }

    async fn set_effort_transaction(
        &self,
        session_key: String,
        effort: Option<String>,
    ) -> Result<Value, DispatchError> {
        let (engine, _) = parse_key(&session_key).map_err(|error| {
            runtime_dispatch_error(RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                format!("Cannot change effort for {session_key}: {error}."),
                false,
            ))
        })?;
        if engine.as_str() == "acp:grok"
            && !self
                .runtime
                .session_is_active(&session_key)
                .map_err(runtime_dispatch_error)?
        {
            let summary = self
                .runtime
                .session_summary(&session_key)
                .map_err(runtime_dispatch_error)?
                .ok_or_else(|| {
                    runtime_dispatch_error(RuntimeError::definitely_unsent(
                        RuntimeErrorKind::Registry,
                        format!(
                            "Cannot resume {session_key}: no record of that session is in the registry."
                        ),
                        false,
                    ))
                })?;
            let event = EngineEvent {
                at: None,
                event: EngineEventKind::ConfigChanged {
                    model: summary.model,
                    effort: Some(effort),
                },
            };
            self.project_synthetic(&session_key, event).await?;
            return Ok(Value::Null);
        }
        self.resumed_delivery(
            &session_key,
            CHANNELS.set_effort,
            RuntimeBridgeCommand::SetEffort {
                session_key: session_key.clone(),
                effort,
            },
        )
        .await?;
        Ok(Value::Null)
    }

    async fn set_fast_mode_transaction(
        &self,
        session_key: String,
        enabled: bool,
    ) -> Result<Value, DispatchError> {
        self.resumed_delivery(
            &session_key,
            CHANNELS.set_fast_mode,
            RuntimeBridgeCommand::SetFastMode {
                session_key: session_key.clone(),
                enabled,
            },
        )
        .await?;
        Ok(Value::Null)
    }

    async fn set_permission_mode_transaction(
        &self,
        session_key: String,
        mode: String,
    ) -> Result<Value, DispatchError> {
        self.resumed_delivery(
            &session_key,
            CHANNELS.set_permission_mode,
            RuntimeBridgeCommand::SetPermissionMode {
                session_key: session_key.clone(),
                mode: mode.clone(),
            },
        )
        .await?;
        let agent = self
            .runtime
            .session_summary(&session_key)
            .ok()
            .flatten()
            .is_none_or(|summary| summary.kind == Some(SessionKind::Agent));
        if !agent {
            self.remember_access(&session_key, Some(mode)).await;
        }
        Ok(Value::Null)
    }

    /// Keeps the mode a launch gave its session once the session is live,
    /// unless an agent's gate or access decides that session's mode instead.
    async fn remember_launch_access(
        &self,
        keep: bool,
        mode: Option<String>,
        response: &RuntimeBridgeResponse,
    ) {
        if let (true, RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(session))) =
            (keep, response)
        {
            self.remember_access(&session.key, mode).await;
        }
    }

    /// Keeps the mode the owner chose for a session; a failure only costs a
    /// later resume its mode, so it is logged rather than raised.
    async fn remember_access(&self, session_key: &str, mode: Option<String>) {
        let Some(memory) = self.session_access.clone() else {
            return;
        };
        let key = session_key.to_owned();
        if tokio::task::spawn_blocking(move || memory.remember(&key, mode.as_deref()))
            .await
            .is_err()
        {
            tracing::warn!(%session_key, "a session's permission mode could not be kept");
        }
        // Protocol 68: the desktop's registry carries the mode; tell it.
        if let Some(registry_pushes) = &self.registry_pushes {
            registry_pushes.schedule();
        }
    }

    async fn close_session_transaction(&self, session_key: String) -> Result<Value, DispatchError> {
        if !self
            .runtime
            .session_is_active(&session_key)
            .map_err(runtime_dispatch_error)?
        {
            return Ok(Value::Null);
        }
        self.deliver_command(
            CHANNELS.close_session,
            RuntimeBridgeCommand::CloseSession { session_key },
        )
        .await?;
        self.emit_registry_snapshot().await?;
        Ok(Value::Null)
    }

    async fn interrupt_transaction(&self, session_key: String) -> Result<Value, DispatchError> {
        let Some(summary) = self
            .runtime
            .session_summary(&session_key)
            .map_err(runtime_dispatch_error)?
        else {
            return Ok(Value::Null);
        };
        if !self
            .runtime
            .session_is_active(&session_key)
            .map_err(runtime_dispatch_error)?
        {
            if matches!(
                summary.status,
                SessionStatus::Working | SessionStatus::Waiting
            ) {
                let event = EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged {
                        status: SessionStatus::Idle,
                    },
                };
                self.project_synthetic(&session_key, event).await?;
            }
            return Ok(Value::Null);
        }
        if let Some(hooks) = &self.send_hooks {
            hooks.interrupt_requested(&session_key);
        }
        self.deliver_command(
            CHANNELS.interrupt,
            RuntimeBridgeCommand::CancelSession { session_key },
        )
        .await?;
        self.emit_registry_snapshot().await?;
        Ok(Value::Null)
    }

    async fn create_session_transaction(
        &self,
        engine: EngineId,
        options: LaunchOptions,
        recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
    ) -> Result<Value, DispatchError> {
        let reservation = if let Some(input) = recovery {
            let desktop = self.prompt_recovery.clone().ok_or_else(|| {
                DispatchError::new(
                    "prompt-recovery-unavailable",
                    "Prompt recovery is unavailable; no thread was started.",
                    false,
                    true,
                )
            })?;
            let provider =
                serde_json::to_string(&(&engine, &options.model, &options.effort, &options.kind))
                    .map_err(|_| {
                    DispatchError::new(
                        "prompt-recovery-unavailable",
                        "The launch identity could not be encoded; no thread was started.",
                        false,
                        true,
                    )
                })?;
            let cwd = options.cwd.clone();
            let prompt = options.prompt.clone();
            Some(
                tokio::task::spawn_blocking(move || {
                    desktop.reserve_launch_recovery(&provider, &cwd, input, &prompt)
                })
                .await
                .map_err(|_| {
                    DispatchError::new(
                        "prompt-recovery-unavailable",
                        "Prompt recovery stopped before launch; no thread was started.",
                        false,
                        true,
                    )
                })?
                .map_err(|e| DispatchError::new(e.code, e.message, false, false))?,
            )
        } else {
            None
        };
        let mut issued = false;
        let result = self
            .create_session_inner(engine, options, &mut issued)
            .await;
        if let Some((pending, id)) = reservation {
            if let Some(key) = result
                .as_ref()
                .ok()
                .and_then(|value| value.get("key"))
                .and_then(Value::as_str)
            {
                let desktop = self
                    .prompt_recovery
                    .clone()
                    .expect("reserved recovery store");
                let key = key.to_owned();
                let bound = tokio::task::spawn_blocking(move || {
                    desktop.bind_launch_recovery(&pending, &id, &key)
                })
                .await;
                if !matches!(bound, Ok(Ok(()))) {
                    return Err(DispatchError::new(
                        "prompt-recovery-unavailable",
                        "The thread started, but its recovery receipt could not be bound. Inspect the existing thread before continuing; do not launch it again.",
                        false,
                        false,
                    ));
                }
            } else {
                let delivery = if !issued
                    || result
                        .as_ref()
                        .is_err_and(|e| e.code == DEFINITELY_UNSENT_CODE)
                {
                    interface_contracts::recovery::PromptDelivery::Unsent
                } else {
                    interface_contracts::recovery::PromptDelivery::Unknown
                };
                self.settle_recovery(&pending, Some(&id), delivery).await;
            }
        }
        result
    }

    #[allow(clippy::too_many_lines)] // Worktree creation, provider effects and cleanup stay one ordered transaction.
    async fn create_session_inner(
        &self,
        engine: EngineId,
        mut options: LaunchOptions,
        issued: &mut bool,
    ) -> Result<Value, DispatchError> {
        if let Err(error) = self.runtime.pin_chat_launch(&mut options) {
            return Err(runtime_dispatch_error(error));
        }
        let keep_access = keeps_owner_access(&options);
        let access = options.permission_mode.clone();
        if options.worktree != Some(true) {
            *issued = true;
            let response = self
                .runtime
                .handle_runtime_command(RuntimeBridgeCommand::StartSession { engine, options })
                .await;
            self.remember_launch_access(keep_access, access, &response)
                .await;
            return self
                .legacy_response(CHANNELS.create_session, response)
                .await;
        }
        let service = self.worktrees.as_ref().ok_or_else(|| {
            DispatchError::new(
                "worktree-isolation-unavailable",
                "Worktree isolation is not attached to this Rust host; no provider was started.",
                false,
                true,
            )
        })?;
        let worktree = service
            .create(options.cwd.clone(), options.name.clone())
            .await
            .map_err(|error| {
                DispatchError::new("worktree-isolation-failed", error.message, false, true)
            })?;
        options.cwd = worktree.session_cwd.to_string_lossy().into_owned();
        options.worktree = None;
        *issued = true;
        let response = self
            .runtime
            .handle_runtime_command(RuntimeBridgeCommand::StartSession { engine, options })
            .await;
        self.remember_launch_access(keep_access, access, &response)
            .await;
        match response {
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(session)) => {
                if let Err(error) = self.runtime.associate_worktree(&session.key, &worktree) {
                    tracing::warn!(
                        session_key = %session.key,
                        %error,
                        "provider is live but its worktree fields could not be projected"
                    );
                }
                if let Err(error) = service.persist(&session.key, &worktree).await {
                    tracing::warn!(
                        session_key = %session.key,
                        %error.message,
                        "provider is live but managed-worktree metadata could not be saved"
                    );
                }
                self.attach_desktop_projection_best_effort(&session.key, "create")
                    .await;
                if let Err(error) = self.emit_worktree_notice(&session.key, &worktree).await {
                    tracing::warn!(
                        session_key = %session.key,
                        code = %error.code,
                        "provider is live but its worktree notice could not be emitted"
                    );
                }
                Ok(serde_json::json!({"key": session.key}))
            }
            RuntimeBridgeResponse::SessionOpen(
                SessionOpenOutcome::DefinitelyUnsent(error)
                | SessionOpenOutcome::DefinitelyRejected(error),
            ) => {
                let mut error = runtime_dispatch_error(error);
                if let Some(cleanup) = service.discard(worktree).await {
                    error.message.push_str(&cleanup);
                }
                Err(error)
            }
            RuntimeBridgeResponse::Error(error)
                if error.certainty != EffectCertainty::OutcomeUnknown =>
            {
                let mut error = runtime_dispatch_error(error);
                if let Some(cleanup) = service.discard(worktree).await {
                    error.message.push_str(&cleanup);
                }
                Err(error)
            }
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::OutcomeUnknown(error))
            | RuntimeBridgeResponse::Error(error) => Err(runtime_dispatch_error(error)),
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::ProviderReadyLocalFailure {
                provider,
                error,
            }) => Err(provider_ready_local_error(&provider.session_id, &error)),
            _ => {
                let mut error = runtime_response_mismatch(CHANNELS.create_session);
                if let Some(cleanup) = service.discard(worktree).await {
                    error.message.push_str(&cleanup);
                }
                Err(error)
            }
        }
    }

    async fn emit_worktree_notice(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<(), DispatchError> {
        let Some(pushes) = &self.pushes else {
            return Ok(());
        };
        let mut payload = serde_json::json!({
            "key": session_key,
            "path": worktree.path,
            "branch": worktree.branch,
            "state": "created",
        });
        if worktree.dirty_source {
            payload["detail"] = Value::String(format!(
                "{} has uncommitted changes; this worktree starts from the last commit, so the agent will not see them.",
                worktree.repo_root.display()
            ));
        }
        let frame = PushFrame::new(WORKTREE_PUSH, payload).map_err(bridge_dispatch_error)?;
        pushes.send(frame).await.map_err(|_| {
            DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                "The session started, but its worktree notice could not reach the desktop.",
                false,
                false,
            )
        })
    }

    async fn emit_session_event(
        &self,
        session_key: &str,
        event: EngineEvent,
    ) -> Result<(), DispatchError> {
        let Some(pushes) = &self.pushes else {
            return Ok(());
        };
        let frame = session_event_frame(session_key, &event).map_err(bridge_dispatch_error)?;
        pushes.send(frame).await.map_err(|_| {
            DispatchError::new(
                OUTCOME_UNKNOWN_CODE,
                "The session changed, but its event could not reach the desktop.",
                false,
                false,
            )
        })
    }

    async fn session_open_result(
        &self,
        outcome: SessionOpenOutcome,
    ) -> Result<Value, DispatchError> {
        match outcome {
            SessionOpenOutcome::Ready(session) => {
                self.attach_desktop_projection_best_effort(&session.key, "open")
                    .await;
                Ok(serde_json::json!({"key": session.key}))
            }
            SessionOpenOutcome::DefinitelyUnsent(error)
            | SessionOpenOutcome::DefinitelyRejected(error)
            | SessionOpenOutcome::OutcomeUnknown(error) => Err(runtime_dispatch_error(error)),
            SessionOpenOutcome::ProviderReadyLocalFailure { provider, error } => {
                Err(DispatchError::new(
                    OUTCOME_UNKNOWN_CODE,
                    format!(
                        "Provider session {} became ready, but local registration failed: {}",
                        provider.session_id, error.message
                    ),
                    false,
                    false,
                ))
            }
        }
    }

    async fn legacy_response(
        &self,
        operation: &str,
        response: RuntimeBridgeResponse,
    ) -> Result<Value, DispatchError> {
        let response = match response {
            RuntimeBridgeResponse::Error(error) => return Err(runtime_dispatch_error(error)),
            response => response,
        };
        if operation == CHANNELS.list_sessions {
            return match response {
                RuntimeBridgeResponse::Sessions(sessions) => encode_legacy_result(sessions),
                _ => Err(runtime_response_mismatch(operation)),
            };
        }
        if operation == CHANNELS.probe_engines {
            return match response {
                RuntimeBridgeResponse::ProvidersReadiness(readiness) => {
                    encode_legacy_result(engine_probes(readiness))
                }
                _ => Err(runtime_response_mismatch(operation)),
            };
        }
        if operation == CHANNELS.create_session || operation == CHANNELS.resume_session {
            return match response {
                RuntimeBridgeResponse::SessionOpen(outcome) => {
                    self.session_open_result(outcome).await
                }
                _ => Err(runtime_response_mismatch(operation)),
            };
        }
        if is_delivery_operation(operation) {
            return match response {
                RuntimeBridgeResponse::Delivery(receipt) => {
                    let result = delivery_result(receipt)?;
                    self.emit_registry_snapshot().await?;
                    Ok(result)
                }
                _ => Err(runtime_response_mismatch(operation)),
            };
        }
        Err(runtime_response_mismatch(operation))
    }
}

#[async_trait]
impl<R> HostDispatcher for RuntimeHostDispatcher<R>
where
    R: RuntimeCommandHandler,
{
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let payload = if operation == CHANNELS.probe_engines {
            // Protocol 41: `{refresh: true}` finds a CLI installed since start.
            if crate::provider_refresh::refresh_requested(&payload, invalid_payload)?
                && let Some(refresh) = &self.provider_refresh
                && let Err(error) = refresh.refresh().await
            {
                tracing::warn!(%error, "provider CLIs could not be looked up again");
            }
            Value::Null
        } else {
            payload
        };
        let request = decode_runtime_request(operation, payload)?;
        self.guard_request(&request).await?;
        let response = match request {
            RuntimeDispatchRequest::Create {
                engine,
                options,
                recovery,
            } => {
                return self
                    .create_session_transaction(engine, *options, recovery)
                    .await;
            }
            RuntimeDispatchRequest::Send {
                session_key,
                text,
                recovery,
            } => {
                return self.send_transaction(session_key, text, recovery).await;
            }
            RuntimeDispatchRequest::Interrupt { session_key } => {
                return self.interrupt_transaction(session_key).await;
            }
            RuntimeDispatchRequest::Close { session_key } => {
                return self.close_session_transaction(session_key).await;
            }
            RuntimeDispatchRequest::SetModel { session_key, model } => {
                return self.set_model_transaction(session_key, model).await;
            }
            RuntimeDispatchRequest::SetEffort {
                session_key,
                effort,
            } => {
                return self.set_effort_transaction(session_key, effort).await;
            }
            RuntimeDispatchRequest::SetFastMode {
                session_key,
                enabled,
            } => {
                return self.set_fast_mode_transaction(session_key, enabled).await;
            }
            RuntimeDispatchRequest::SetPermissionMode { session_key, mode } => {
                return self
                    .set_permission_mode_transaction(session_key, mode)
                    .await;
            }
            RuntimeDispatchRequest::Command(command) => {
                let observer = self
                    .listing_observer
                    .as_ref()
                    .filter(|_| matches!(*command, RuntimeBridgeCommand::ListSessions));
                let before = observer.and_then(|_| self.runtime.registry_content_revision());
                let response = self.runtime.handle_runtime_command(*command).await;
                // A listing merges what the providers' stores hold into the
                // registry; with no push behind it, state kept beside the
                // registry (threads) would not hear of new sessions until
                // the next unrelated change. An unknown revision always tells.
                if let Some(observer) = observer
                    && matches!(response, RuntimeBridgeResponse::Sessions(_))
                    && (before.is_none() || self.runtime.registry_content_revision() != before)
                {
                    observer.registry_round();
                }
                response
            }
            RuntimeDispatchRequest::Resume { session_key, gate } => {
                self.resume_session(session_key, gate).await
            }
            RuntimeDispatchRequest::History { session_key, limit } => {
                return self.history_page(&session_key, limit).await;
            }
        };
        self.legacy_response(operation, response).await
    }
}

/// Whether a launch's session keeps its owner's mode across restarts: not
/// when an agent's gate or access decides it instead.
fn keeps_owner_access(options: &LaunchOptions) -> bool {
    options.kind != Some(SessionKind::Agent)
        && options.approval_gate != Some(true)
        && options.agent_access.is_none()
}

async fn resume_cwd(session_key: &str, summary: &SessionSummary) -> Result<String, RuntimeError> {
    if summary.cwd.is_empty() {
        return Err(RuntimeError::definitely_unsent(
            RuntimeErrorKind::InvalidInput,
            format!("Cannot resume {session_key}: no working directory was recorded for it."),
            false,
        ));
    }
    if tokio::fs::metadata(&summary.cwd).await.is_ok() {
        return Ok(summary.cwd.clone());
    }
    if let Some(worktree_home) = summary.worktree_home.as_ref()
        && !worktree_home.is_empty()
        && tokio::fs::metadata(worktree_home).await.is_ok()
    {
        return Ok(worktree_home.clone());
    }
    Err(RuntimeError::definitely_unsent(
        RuntimeErrorKind::InvalidInput,
        format!("Cannot resume {session_key}: its recorded working directory no longer exists."),
        false,
    ))
}

fn concrete_setting(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "default")
        .map(ToOwned::to_owned)
}

fn runtime_command_guard(command: &RuntimeBridgeCommand) -> Option<(&str, &'static str, bool)> {
    match command {
        RuntimeBridgeCommand::Send { session_key, .. } => Some((session_key, "send", false)),
        RuntimeBridgeCommand::CancelSession { session_key }
        | RuntimeBridgeCommand::CloseSession { session_key } => {
            Some((session_key, "safe stop", true))
        }
        RuntimeBridgeCommand::RespondToApproval { session_key, .. } => {
            Some((session_key, "approval response", false))
        }
        RuntimeBridgeCommand::SetModel { session_key, .. } => {
            Some((session_key, "model change", false))
        }
        RuntimeBridgeCommand::SetEffort { session_key, .. } => {
            Some((session_key, "effort change", false))
        }
        RuntimeBridgeCommand::SetFastMode { session_key, .. } => {
            Some((session_key, "fast-mode change", false))
        }
        RuntimeBridgeCommand::SetPermissionMode { session_key, .. } => {
            Some((session_key, "access change", false))
        }
        // Read-only and session-management commands skip this fence. The
        // fail-closed collaboration net is the explicit mutation arms above,
        // not a default deny: create/list/probe/history never mutate a live
        // session through this owner.
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn pump_runtime_events<R>(
    runtime: Arc<R>,
    mut pushes: Option<mpsc::Sender<PushFrame>>,
    mut registry_pushes: Option<RegistryPushCoordinator>,
    worktree_release: Option<Arc<dyn AutomaticWorktreeRelease>>,
    collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
    event_observer: Option<Arc<dyn SessionEventObserver>>,
    session_key: String,
    mut events: BoundedEventStream<RuntimeEventFrame>,
) where
    R: RuntimeCommandHandler,
{
    loop {
        match events.recv().await {
            Ok(Some(RuntimeEventFrame::Event { mut value })) => {
                // Provider adapters may use this internal marker to finish
                // their own stream. The frozen desktop contract announces it
                // exactly once, after the attachment has actually left the
                // live map, so hold it for the terminal arm below.
                if matches!(value.event.event, EngineEventKind::SessionClosed) {
                    continue;
                }
                // The bridge observation timestamp is the public live-event
                // timestamp. Runtime history keeps provider timestamps and a
                // separate received-at field; the desktop always receives the
                // latter, just as 0.1.169 overwrote `at` at broadcast time.
                value.event.at = Some(value.received_at.clone());
                if let Some(collaboration) = &collaboration {
                    if let Err(error) =
                        collaboration.observe_event(&value.session_key, &value.event)
                    {
                        tracing::error!(code = %error.code, "collaboration source event projection failed");
                    }
                    if let Err(error) = collaboration.observe_registry_change(&value.session_key) {
                        tracing::error!(code = %error.code, "collaboration source registry projection failed");
                    }
                }
                if let Some(observer) = &event_observer {
                    observer.observe_session_event(&value.session_key, &value.event);
                }
                if let Some(sender) = &pushes {
                    let event = session_event_frame(&value.session_key, &value.event);
                    let Ok(event) = event else {
                        tracing::error!("failed to construct the frozen session event push");
                        break;
                    };
                    // The desktop is gone (at quit its receiver closes before
                    // the runtime closes sessions) or the shared coalescer
                    // stopped: stop forwarding, but keep draining so the
                    // terminal close projection and worktree release still run.
                    if sender.send(event).await.is_err()
                        || registry_pushes
                            .as_ref()
                            .is_some_and(|registry_pushes| !registry_pushes.schedule())
                    {
                        pushes = None;
                        registry_pushes = None;
                    }
                }
            }
            Ok(Some(RuntimeEventFrame::Terminal {
                session_key: terminal_key,
                ..
            })) => {
                if terminal_key != session_key {
                    tracing::error!(
                        expected = session_key,
                        actual = terminal_key,
                        "runtime terminal identity changed inside one event subscription"
                    );
                    break;
                }
                project_terminal_close(
                    runtime.as_ref(),
                    pushes.as_ref(),
                    registry_pushes.as_ref(),
                    collaboration.as_deref(),
                    event_observer.as_deref(),
                    worktree_release.as_deref(),
                    &session_key,
                )
                .await;
                break;
            }
            Ok(None) => {
                project_terminal_close(
                    runtime.as_ref(),
                    pushes.as_ref(),
                    registry_pushes.as_ref(),
                    collaboration.as_deref(),
                    event_observer.as_deref(),
                    worktree_release.as_deref(),
                    &session_key,
                )
                .await;
                break;
            }
            Err(error) => {
                tracing::error!(%error, "runtime-to-desktop event queue overflowed");
                // The runtime fails an overflowed session closed and then
                // evicts it, but this stream reports the overflow first:
                // wait for the eviction so the close can be proven.
                wait_for_detach(
                    || runtime.session_is_active(&session_key).map(Some),
                    OVERFLOW_DETACH_WAIT,
                )
                .await;
                project_terminal_close(
                    runtime.as_ref(),
                    pushes.as_ref(),
                    registry_pushes.as_ref(),
                    collaboration.as_deref(),
                    event_observer.as_deref(),
                    worktree_release.as_deref(),
                    &session_key,
                )
                .await;
                break;
            }
        }
    }
}

async fn project_terminal_close<R>(
    runtime: &R,
    pushes: Option<&mpsc::Sender<PushFrame>>,
    registry_pushes: Option<&RegistryPushCoordinator>,
    collaboration: Option<&dyn CollaborationSessionHooks>,
    event_observer: Option<&dyn SessionEventObserver>,
    worktree_release: Option<&dyn AutomaticWorktreeRelease>,
    session_key: &str,
) where
    R: RuntimeCommandHandler,
{
    // A replacement can attach after the old runtime removes itself but before
    // this desktop task observes the terminal frame. Only the current absence
    // authorizes the liveness-death signal and worktree release.
    if !matches!(
        settled_session_activity(
            || runtime.session_is_active(session_key).map(Some),
            session_key,
            "could not prove that the terminal attachment left the runtime",
        )
        .await,
        Some(false)
    ) {
        return;
    }

    let closed = EngineEvent {
        at: Some(SystemRuntimeClock.now()),
        event: EngineEventKind::SessionClosed,
    };
    if let Err(error) = runtime.project_event(session_key, &closed) {
        tracing::error!(session_key, %error, "could not project the terminal session event");
    }
    if let Some(collaboration) = collaboration {
        if let Err(error) = collaboration.observe_event(session_key, &closed) {
            tracing::error!(code = %error.code, "collaboration terminal event projection failed");
        }
        if let Err(error) = collaboration.observe_registry_change(session_key) {
            tracing::error!(code = %error.code, "collaboration terminal registry projection failed");
        }
    }
    if let Some(observer) = event_observer {
        observer.observe_session_event(session_key, &closed);
    }
    let mut desktop_gone = false;
    if let Some(pushes) = pushes {
        let frame = session_event_frame(session_key, &closed);
        match frame {
            Ok(frame) => {
                // At quit the desktop's receiver is already gone: there is no
                // registry frame to send, but the release below still runs.
                desktop_gone = pushes.send(frame).await.is_err();
            }
            Err(error) => {
                tracing::error!(%error.message, "could not construct the terminal event push");
                return;
            }
        }
    }
    if !desktop_gone {
        if let Some(registry_pushes) = registry_pushes {
            let _ = registry_pushes.flush().await;
        } else if let Some(pushes) = pushes {
            let _ = emit_registry_from_runtime(runtime, pushes).await;
        }
    }
    if let Some(worktree_release) = worktree_release {
        worktree_release.schedule_session_release(session_key.to_owned());
    }
}

/// Polls until `session_is_active` reports the attachment gone (or has
/// nothing to report), backing off to 250 ms, for at most `limit`. Returns
/// whether it saw it go; the caller's terminal projection makes the final,
/// logged decision either way.
pub(crate) async fn wait_for_detach(
    session_is_active: impl Fn() -> Result<Option<bool>, RuntimeError>,
    limit: Duration,
) -> bool {
    let deadline = Instant::now() + limit;
    let mut pause = Duration::from_millis(5);
    loop {
        if matches!(session_is_active(), Ok(Some(false) | None)) {
            return true;
        }
        if Instant::now() + pause > deadline {
            return false;
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_millis(250));
    }
}

/// Settles live-attachment reads across a bounded attachment edge.
///
/// The production runtime only reports a retryable miss while its live map is
/// crossing that edge. Returns `Some(true)` if the session is still live,
/// `Some(false)` once it is proven inactive, and `None` after an unretryable
/// or exhausted failure (already logged).
pub(crate) async fn settled_session_activity(
    session_is_active: impl Fn() -> Result<Option<bool>, RuntimeError>,
    session_key: &str,
    exhausted_warning: &'static str,
) -> Option<bool> {
    let mut last_error = None;
    let mut active = None;
    for attempt in 0..16 {
        match session_is_active() {
            // A port with nothing to report is not a retryable miss: stop
            // without retrying and without the exhausted warning.
            Ok(None) => return None,
            Ok(Some(is_active)) => {
                active = Some(is_active);
                break;
            }
            Err(error) => {
                let retryable = error.retryable;
                last_error = Some(error);
                if !retryable || attempt == 15 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
    }
    let Some(is_active) = active else {
        if let Some(error) = last_error {
            tracing::warn!(
                session_key,
                %error,
                reason = exhausted_warning,
                "could not settle session activity"
            );
        }
        return None;
    };
    Some(is_active)
}

fn runtime_input_error(message: String) -> RuntimeBridgeResponse {
    RuntimeBridgeResponse::Error(RuntimeError::definitely_unsent(
        RuntimeErrorKind::InvalidInput,
        message,
        false,
    ))
}

pub(crate) fn bridge_dispatch_error(error: BridgeError) -> DispatchError {
    DispatchError::new(
        error.code,
        error.message,
        error.retryable,
        error.replay_safe,
    )
}

pub(crate) fn engine_probes(readiness: Vec<ProviderReadiness>) -> Vec<EngineProbe> {
    readiness
        .into_iter()
        .map(|readiness| match readiness {
            ProviderReadiness::Ready { engine, info } => EngineProbe {
                engine,
                ok: true,
                version: Some(info.version.clone()),
                info: Some(*info),
                error: None,
            },
            ProviderReadiness::Unavailable { engine, error } => EngineProbe {
                engine,
                ok: false,
                version: None,
                info: None,
                error: Some(error.message),
            },
        })
        .collect()
}

fn delivery_result(receipt: RuntimeDeliveryReceipt) -> Result<Value, DispatchError> {
    if receipt.disposition == RuntimeDeliveryDisposition::Confirmed {
        return Ok(Value::Null);
    }
    let (message, retryable) = receipt.error.map_or_else(
        || {
            (
                "The provider effect did not complete with confirmed delivery.".to_owned(),
                false,
            )
        },
        |error| (error.message, error.retryable),
    );
    let code = match receipt.disposition {
        RuntimeDeliveryDisposition::Confirmed => unreachable!("handled above"),
        RuntimeDeliveryDisposition::DefinitelyUnsent => DEFINITELY_UNSENT_CODE,
        RuntimeDeliveryDisposition::DefinitelyRejected => DEFINITELY_REJECTED_CODE,
        RuntimeDeliveryDisposition::OutcomeUnknown => OUTCOME_UNKNOWN_CODE,
    };
    let replay_safe = receipt.disposition == RuntimeDeliveryDisposition::DefinitelyUnsent;
    Err(DispatchError::new(code, message, retryable, replay_safe))
}

/// Builds the frozen `iface:event` push frame for one projected session event.
///
/// # Errors
///
/// Returns the bridge error raised when the frame exceeds the push envelope.
pub(crate) fn session_event_frame(
    session_key: &str,
    event: &EngineEvent,
) -> Result<PushFrame, BridgeError> {
    PushFrame::new(
        CHANNELS.event,
        serde_json::json!({"key": session_key, "event": event}),
    )
}

/// Maps a runtime failure onto its exact certainty-preserving dispatch error.
#[must_use]
pub fn runtime_dispatch_error(error: RuntimeError) -> DispatchError {
    let code = match error.certainty {
        EffectCertainty::DefinitelyUnsent => DEFINITELY_UNSENT_CODE,
        EffectCertainty::DefinitelyRejected => DEFINITELY_REJECTED_CODE,
        EffectCertainty::OutcomeUnknown => OUTCOME_UNKNOWN_CODE,
    };
    let replay_safe = error.replay_is_safe();
    DispatchError::new(code, error.message, error.retryable, replay_safe)
}

pub(crate) fn encode_legacy_result<T>(value: T) -> Result<Value, DispatchError>
where
    T: Serialize,
{
    serde_json::to_value(value).map_err(|error| {
        tracing::error!(%error, "failed to encode a legacy desktop result");
        DispatchError::new(
            "legacy-result-encode-failed",
            "The Rust host could not encode the desktop result.",
            false,
            false,
        )
    })
}

fn runtime_response_mismatch(operation: &str) -> DispatchError {
    tracing::error!(
        operation,
        "runtime returned a response for a different operation"
    );
    DispatchError::new(
        "runtime-response-mismatch",
        "The Rust runtime returned an incompatible response.",
        false,
        false,
    )
}

fn provider_ready_local_error(session_id: &str, error: &RuntimeError) -> DispatchError {
    DispatchError::new(
        OUTCOME_UNKNOWN_CODE,
        format!(
            "Provider session {session_id} became ready, but local registration failed: {}",
            error.message
        ),
        false,
        false,
    )
}

fn is_delivery_operation(operation: &str) -> bool {
    [
        CHANNELS.close_session,
        CHANNELS.respond,
        CHANNELS.set_effort,
        CHANNELS.set_fast_mode,
        CHANNELS.set_permission_mode,
    ]
    .contains(&operation)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartSessionPayload {
    engine: EngineId,
    options: LaunchOptions,
    #[serde(default)]
    recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionPayload {
    session_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResumePayload {
    session_key: String,
    #[serde(default)]
    approval_gate: Option<bool>,
    #[serde(default)]
    additional_directories: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadHistoryPayload {
    session_key: String,
    #[serde(default)]
    options: Option<HistoryReadOptions>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendPayload {
    session_key: String,
    text: String,
    #[serde(default)]
    recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalPayload {
    session_key: String,
    approval_id: String,
    verdict: BridgeApprovalVerdict,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    answers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetModelPayload {
    session_key: String,
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetEffortPayload {
    session_key: String,
    effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetFastModePayload {
    session_key: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPermissionModePayload {
    session_key: String,
    mode: String,
}

fn decode_runtime_request(
    operation: &str,
    payload: Value,
) -> Result<RuntimeDispatchRequest, DispatchError> {
    if operation == CHANNELS.create_session {
        let payload = decode_payload::<StartSessionPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::Create {
            engine: payload.engine,
            options: Box::new(payload.options),
            recovery: payload.recovery,
        });
    }
    if operation == CHANNELS.send {
        let payload = decode_payload::<SendPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::Send {
            session_key: payload.session_key,
            text: payload.text,
            recovery: payload.recovery,
        });
    }
    if operation == CHANNELS.interrupt {
        let payload = decode_payload::<SessionPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::Interrupt {
            session_key: payload.session_key,
        });
    }
    if operation == CHANNELS.set_model {
        let payload = decode_payload::<SetModelPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::SetModel {
            session_key: payload.session_key,
            model: payload.model,
        });
    }
    if operation == CHANNELS.close_session {
        let payload = decode_payload::<SessionPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::Close {
            session_key: payload.session_key,
        });
    }
    if operation == CHANNELS.set_effort {
        let payload = decode_payload::<SetEffortPayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::SetEffort {
            session_key: payload.session_key,
            effort: payload.effort,
        });
    }
    if operation == CHANNELS.set_fast_mode {
        let payload = decode_payload::<SetFastModePayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::SetFastMode {
            session_key: payload.session_key,
            enabled: payload.enabled,
        });
    }
    if operation == CHANNELS.set_permission_mode {
        let payload = decode_payload::<SetPermissionModePayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::SetPermissionMode {
            session_key: payload.session_key,
            mode: payload.mode,
        });
    }
    if operation == CHANNELS.resume_session {
        let payload = decode_payload::<ResumePayload>(operation, payload)?;
        return Ok(RuntimeDispatchRequest::Resume {
            session_key: payload.session_key,
            gate: RequestedResumeGate {
                approval_gate: payload.approval_gate,
                additional_directories: payload.additional_directories,
            },
        });
    }
    if operation == CHANNELS.read_history {
        let payload = decode_payload::<ReadHistoryPayload>(operation, payload)?;
        let limit = history_read_limit(payload.options.and_then(|options| options.limit));
        return Ok(RuntimeDispatchRequest::History {
            session_key: payload.session_key,
            limit,
        });
    }
    decode_runtime_command(operation, payload)
        .map(Box::new)
        .map(RuntimeDispatchRequest::Command)
}

fn decode_runtime_command(
    operation: &str,
    payload: Value,
) -> Result<RuntimeBridgeCommand, DispatchError> {
    if operation == CHANNELS.list_sessions {
        decode_empty(operation, &payload)?;
        return Ok(RuntimeBridgeCommand::ListSessions);
    }
    if operation == CHANNELS.probe_engines {
        decode_empty(operation, &payload)?;
        return Ok(RuntimeBridgeCommand::ProbeAllProviders);
    }
    if operation == CHANNELS.respond {
        let payload = decode_payload::<ApprovalPayload>(operation, payload)?;
        return Ok(RuntimeBridgeCommand::RespondToApproval {
            session_key: payload.session_key,
            approval_id: payload.approval_id,
            verdict: payload.verdict,
            response: BridgeApprovalResponse {
                note: payload.note,
                answers: payload.answers.into_iter().collect(),
            },
        });
    }
    Err(operation_not_migrated())
}

fn decode_empty(operation: &str, payload: &Value) -> Result<(), DispatchError> {
    payload::require_empty(payload, || {
        tracing::warn!(
            operation,
            "non-empty payload supplied to a zero-argument operation"
        );
        invalid_payload()
    })
}

fn decode_payload<T>(operation: &str, payload: Value) -> Result<T, DispatchError>
where
    T: DeserializeOwned,
{
    payload::decode_object(payload, |error| {
        match error {
            None => tracing::warn!(operation, "rejected a non-object runtime bridge payload"),
            Some(error) => {
                tracing::warn!(operation, %error, "rejected an invalid runtime bridge payload");
            }
        }
        invalid_payload()
    })
}

fn invalid_payload() -> DispatchError {
    DispatchError::new(
        INVALID_PAYLOAD_CODE,
        "The request payload does not match the migrated operation schema.",
        false,
        true,
    )
}

fn operation_not_migrated() -> DispatchError {
    DispatchError::new(
        OPERATION_NOT_MIGRATED_CODE,
        "This Interface operation has not migrated to the Rust runtime.",
        false,
        true,
    )
}

fn registry_unreachable() -> DispatchError {
    DispatchError::new(
        OUTCOME_UNKNOWN_CODE,
        "The session changed, but its registry update could not reach the desktop.",
        false,
        false,
    )
}

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
mod tests;
