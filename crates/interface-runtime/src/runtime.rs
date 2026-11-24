#[cfg(test)]
use interface_engine::bounded_event_stream;
use interface_engine::event_budget::BufferedEvent;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use engine_acp::{AcpAdapter, AcpAdapterConfig, AcpProviderFlavor};
use engine_claude::ClaudeAdapter;
use engine_codex::{CodexAdapter, CodexAdapterConfig};
use interface_contracts::canonical_timestamp;
use interface_contracts::engine::{
    CollaborationProviderBindings, EngineEvent, EngineId, EngineInfo, LaunchOptions, ModelOption,
    ResumeOptions, SessionKind, SessionOrigin, SessionStatus, SessionSummary, session_key,
    session_kind_of,
};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EffectCertainty,
    EngineAdapter, EngineError, EngineErrorKind, EventSink, FileVersion, SessionHandle,
    bounded_event_stream_with_budget,
};
use interface_orchestration::OrchestrationStore;
use interface_registry::{GitRunner, SessionRegistry, SystemGit};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{RwLock, mpsc},
    task::{JoinHandle, JoinSet},
    time::Instant,
};
use uuid::Uuid;

use crate::{
    EngineHistoryStore, RuntimeEffectKind, RuntimeError, RuntimeErrorKind, RuntimeHistorySnapshot,
    management::SessionManagementState, project_root::ProjectRootProjector,
};

#[path = "pump.rs"]
mod pump;
#[path = "registry_cache.rs"]
mod registry_cache;

use pump::{
    ProviderIngress, forward_provider_events, mark_persistence_failed, persist_effect,
    persist_registration, run_event_pump,
};
use registry_cache::RegistryCacheState;

const LIVE_ACTIVE: u8 = 0;
const LIVE_OUTCOME_UNKNOWN: u8 = 1;
const LIVE_PERSISTENCE_FAILED: u8 = 2;
const LIVE_CLOSING: u8 = 3;
const LIVE_CLOSED: u8 = 4;
const RUNTIME_ACCEPTING: u8 = 0;
const RUNTIME_QUIESCING: u8 = 1;
const RUNTIME_SHUT_DOWN: u8 = 2;
const PUMP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a successful probe is reused while the provider's executable still
/// resolves to the file it probed. A provider's catalogue and login change with
/// its binary or account, not with every health read; an explicit refresh
/// (`probe_all_providers`) never reuses one.
const PROBE_FRESHNESS: Duration = Duration::from_secs(5 * 60);
/// The reuse window for a provider whose executable cannot be stat'ed, so a
/// changed binary could not be noticed.
const UNIDENTIFIED_PROBE_FRESHNESS: Duration = Duration::from_secs(10);
const CHAT_UNAVAILABLE: &str = "Chat is unavailable. This build has no chat folder configured.";

pub trait RuntimeClock: Send + Sync {
    fn now(&self) -> String;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRuntimeClock;

impl RuntimeClock for SystemRuntimeClock {
    fn now(&self) -> String {
        canonical_timestamp(Utc::now())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeConfiguration {
    pub pre_ready_event_capacity: usize,
    pub subscriber_event_capacity: usize,
}

impl Default for RuntimeConfiguration {
    fn default() -> Self {
        Self {
            pre_ready_event_capacity: 512,
            subscriber_event_capacity: 2_000,
        }
    }
}

impl RuntimeConfiguration {
    fn validate(self) -> Result<Self, RuntimeError> {
        if self.pre_ready_event_capacity == 0 || self.subscriber_event_capacity == 0 {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                "runtime event capacities must be positive",
                false,
            ));
        }
        Ok(self)
    }
}

/// Provider configurations that can be registered without launching or probing them.
#[derive(Clone, Debug, Default)]
pub struct NativeAdapterConfiguration {
    pub codex: Option<CodexAdapterConfig>,
    pub claude_executable: Option<PathBuf>,
    pub acp: Vec<AcpAdapterConfig>,
}

impl NativeAdapterConfiguration {
    /// Sets the `clientInfo.version` the Codex adapter and every ACP adapter
    /// send in their handshakes. The binaries pass their product version here
    /// at runtime, so no engine crate compiles a release version in.
    #[must_use]
    pub fn with_client_version(mut self, version: &str) -> Self {
        if let Some(codex) = self.codex.as_mut() {
            version.clone_into(&mut codex.client_version);
        }
        for acp in &mut self.acp {
            version.clone_into(&mut acp.client_version);
        }
        self
    }
}

/// Canonical state services shared by the desktop, CLI, and orchestration host.
///
/// Plans and orchestration remain optional because engine-only callers should
/// not need to acquire their writer leases. When present, these are the exact
/// canonical stores rather than parallel runtime models.
#[derive(Clone)]
pub struct RuntimeServices {
    sessions: Arc<Mutex<SessionRegistry>>,
    registry_cache: Arc<RegistryCacheState>,
    history: Arc<Mutex<EngineHistoryStore>>,
    orchestration: Option<Arc<Mutex<OrchestrationStore>>>,
}

impl RuntimeServices {
    #[must_use]
    pub fn new(sessions: SessionRegistry, history: EngineHistoryStore) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(sessions)),
            registry_cache: Arc::new(RegistryCacheState::default()),
            history: Arc::new(Mutex::new(history)),
            orchestration: None,
        }
    }

    #[must_use]
    pub fn with_orchestration(mut self, orchestration: OrchestrationStore) -> Self {
        self.orchestration = Some(Arc::new(Mutex::new(orchestration)));
        self
    }

    #[must_use]
    pub fn session_registry(&self) -> Arc<Mutex<SessionRegistry>> {
        Arc::clone(&self.sessions)
    }

    #[must_use]
    pub fn orchestration(&self) -> Option<Arc<Mutex<OrchestrationStore>>> {
        self.orchestration.as_ref().map(Arc::clone)
    }
}

impl std::fmt::Debug for RuntimeServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeServices")
            .field("orchestration", &self.orchestration.is_some())
            .finish_non_exhaustive()
    }
}

pub struct SessionRuntimeBuilder {
    services: RuntimeServices,
    adapters: HashMap<EngineId, Arc<dyn EngineAdapter>>,
    configuration: RuntimeConfiguration,
    clock: Arc<dyn RuntimeClock>,
    project_root_git: Arc<dyn GitRunner>,
    chat_home: Option<PathBuf>,
}

impl SessionRuntimeBuilder {
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        Self {
            services,
            adapters: HashMap::new(),
            configuration: RuntimeConfiguration::default(),
            clock: Arc::new(SystemRuntimeClock),
            project_root_git: Arc::new(SystemGit),
            chat_home: None,
        }
    }

    #[must_use]
    pub const fn configuration(mut self, configuration: RuntimeConfiguration) -> Self {
        self.configuration = configuration;
        self
    }

    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn RuntimeClock>) -> Self {
        self.clock = clock;
        self
    }

    #[must_use]
    pub fn chat_home(mut self, path: PathBuf) -> Self {
        self.chat_home = Some(path);
        self
    }

    /// Registers one provider-neutral adapter without probing it.
    ///
    /// # Errors
    ///
    /// Refuses duplicate engine identifiers.
    pub fn register_adapter<A>(&mut self, adapter: A) -> Result<(), RuntimeError>
    where
        A: EngineAdapter + 'static,
    {
        self.register_shared_adapter(Arc::new(adapter))
    }

    /// Registers a shared fake or native adapter without probing it.
    ///
    /// # Errors
    ///
    /// Refuses duplicate engine identifiers.
    pub fn register_shared_adapter(
        &mut self,
        adapter: Arc<dyn EngineAdapter>,
    ) -> Result<(), RuntimeError> {
        let engine = adapter.id().clone();
        if self.adapters.contains_key(&engine) {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::DuplicateEngine,
                format!("engine {engine} is already registered"),
                false,
            ));
        }
        self.adapters.insert(engine, adapter);
        Ok(())
    }

    /// The registered adapter for `engine`, so a host can stand an
    /// [`crate::accounts::AccountRoutedAdapter`] in front of it with
    /// [`Self::replace_adapter`].
    #[must_use]
    pub fn adapter(&self, engine: &EngineId) -> Option<Arc<dyn EngineAdapter>> {
        self.adapters.get(engine).cloned()
    }

    /// Replaces the registered adapter for its engine id.
    ///
    /// # Errors
    ///
    /// Refuses an engine that has no registered adapter.
    pub fn replace_adapter(&mut self, adapter: Arc<dyn EngineAdapter>) -> Result<(), RuntimeError> {
        let engine = adapter.id().clone();
        let Some(slot) = self.adapters.get_mut(&engine) else {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::EngineNotRegistered,
                format!("engine {engine} is not registered"),
                false,
            ));
        };
        *slot = adapter;
        Ok(())
    }

    /// Constructs and registers Codex, Claude, and any ACP profiles without
    /// contacting their providers.
    ///
    /// # Errors
    ///
    /// Returns local adapter validation or duplicate-identifier failures.
    pub fn register_native_adapters(
        &mut self,
        configuration: NativeAdapterConfiguration,
    ) -> Result<(), RuntimeError> {
        self.register_native_adapters_with_collaboration(configuration, None)
    }

    /// Constructs and registers native adapters with the exact host-owned
    /// collaboration bundle used by the legacy provider composition.
    ///
    /// Codex receives its process descriptor, source registrar, and depth
    /// fence as one atomic bundle. Only the built-in Grok ACP adapter receives
    /// the per-session MCP factory; portable Qwen and custom ACP profiles must
    /// remain provider-neutral.
    ///
    /// # Errors
    ///
    /// Returns local adapter validation or duplicate-identifier failures.
    pub fn register_native_adapters_with_collaboration(
        &mut self,
        configuration: NativeAdapterConfiguration,
        collaboration: Option<&CollaborationProviderBindings>,
    ) -> Result<(), RuntimeError> {
        self.register_native_adapters_with_team_tools(configuration, collaboration, None)
    }

    /// [`Self::register_native_adapters_with_collaboration`], and every ACP
    /// adapter also gets the host's team tools factory (`iface_team`,
    /// protocol 57): every flavor takes stdio MCP servers. Codex gets it
    /// through its own configuration (`CodexAdapterConfig::team_tools`).
    ///
    /// # Errors
    ///
    /// Returns local adapter validation or duplicate-identifier failures.
    pub fn register_native_adapters_with_team_tools(
        &mut self,
        configuration: NativeAdapterConfiguration,
        collaboration: Option<&CollaborationProviderBindings>,
        team_tools: Option<&Arc<dyn interface_contracts::engine::TeamToolsFactory>>,
    ) -> Result<(), RuntimeError> {
        if let Some(codex) = configuration.codex {
            let mut adapter = CodexAdapter::new(codex).map_err(engine_runtime_error)?;
            if let Some(bindings) = collaboration {
                adapter = adapter
                    .with_collaboration_bindings(bindings)
                    .map_err(engine_runtime_error)?;
            }
            self.register_adapter(adapter)?;
        }
        if let Some(executable) = configuration.claude_executable {
            let mut adapter = ClaudeAdapter::new(executable).map_err(engine_runtime_error)?;
            if let Some(bindings) = collaboration {
                adapter = adapter.with_collaboration_factory(Arc::clone(&bindings.session_factory));
            }
            self.register_adapter(adapter)?;
        }
        for acp in configuration.acp {
            let flavor = acp.flavor;
            let mut adapter = AcpAdapter::new(acp).map_err(engine_runtime_error)?;
            if let (Some(bindings), AcpProviderFlavor::Grok) = (collaboration, flavor) {
                adapter = adapter
                    .with_collaboration_factory(Arc::clone(&bindings.session_factory))
                    .map_err(engine_runtime_error)?;
            }
            if let Some(team_tools) = team_tools {
                adapter = adapter.with_team_tools(Arc::clone(team_tools));
            }
            self.register_adapter(adapter)?;
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error for zero runtime capacities.
    pub fn build(self) -> Result<SessionRuntime, RuntimeError> {
        let configuration = self.configuration.validate()?;
        let management = Arc::new(SessionManagementState::new(self.adapters.keys().cloned()));
        let model_probe_locks = self
            .adapters
            .keys()
            .cloned()
            .map(|engine| (engine, Arc::new(tokio::sync::Mutex::new(()))))
            .collect();
        let provider_probe_locks = self
            .adapters
            .keys()
            .cloned()
            .map(|engine| (engine, tokio::sync::Mutex::new(())))
            .collect();
        Ok(SessionRuntime {
            inner: Arc::new(RuntimeInner {
                services: self.services,
                adapters: self.adapters,
                live: RwLock::new(HashMap::new()),
                management,
                configuration,
                clock: self.clock,
                lifecycle: AtomicU8::new(RUNTIME_ACCEPTING),
                shutdown_lock: tokio::sync::Mutex::new(()),
                model_catalogues: RwLock::new(HashMap::new()),
                kept_probes: RwLock::new(HashMap::new()),
                probe_starts: AtomicU64::new(0),
                orchestration_model_probes: RwLock::new(HashSet::new()),
                model_probe_locks,
                provider_probe_locks,
                project_roots: Arc::new(ProjectRootProjector::new(self.project_root_git)),
                chat_home: self.chat_home,
            }),
        })
    }
}

pub(crate) struct RuntimeInner {
    pub(crate) services: RuntimeServices,
    pub(crate) adapters: HashMap<EngineId, Arc<dyn EngineAdapter>>,
    pub(crate) live: RwLock<HashMap<String, Arc<LiveSession>>>,
    pub(crate) management: Arc<SessionManagementState>,
    configuration: RuntimeConfiguration,
    pub(crate) clock: Arc<dyn RuntimeClock>,
    lifecycle: AtomicU8,
    shutdown_lock: tokio::sync::Mutex<()>,
    model_catalogues: RwLock<HashMap<EngineId, Vec<ModelOption>>>,
    /// Successful probes only, so a failure is always re-probed.
    kept_probes: RwLock<HashMap<EngineId, KeptProbe>>,
    /// Counts probes as they start, so an explicit refresh can tell a probe
    /// that began after it asked from one that was already running.
    probe_starts: AtomicU64,
    orchestration_model_probes: RwLock<HashSet<EngineId>>,
    model_probe_locks: HashMap<EngineId, Arc<tokio::sync::Mutex<()>>>,
    provider_probe_locks: HashMap<EngineId, tokio::sync::Mutex<()>>,
    pub(crate) project_roots: Arc<ProjectRootProjector>,
    chat_home: Option<PathBuf>,
}

/// A successful probe and the file it probed.
struct KeptProbe {
    /// The executable's version just before the probe ran; `None` when the
    /// adapter has no executable or it could not be stat'ed.
    executable: Option<FileVersion>,
    /// This probe's place in `probe_starts`.
    started: u64,
    finished: Instant,
    info: Box<EngineInfo>,
}

impl KeptProbe {
    /// Whether this answer stands in for a probe asked for now, while the
    /// executable is at `executable`. An explicit refresh (`fresh_after`) takes
    /// only a probe that started after it asked.
    fn serves(&self, executable: Option<FileVersion>, fresh_after: Option<u64>) -> bool {
        if self.executable != executable {
            return false;
        }
        if let Some(asked) = fresh_after {
            return self.started > asked;
        }
        let window = if executable.is_some() {
            PROBE_FRESHNESS
        } else {
            UNIDENTIFIED_PROBE_FRESHNESS
        };
        self.finished.elapsed() < window
    }
}

pub(crate) struct LiveSession {
    descriptor: Arc<RuntimeSession>,
    handle: Arc<dyn SessionHandle>,
    subscriber: EventSink<RuntimeEventFrame>,
    events: Mutex<Option<BoundedEventStream<RuntimeEventFrame>>>,
    state: AtomicU8,
    /// The turn slot. A send, a setting change or a close holds it for its
    /// whole delivery, so they run one at a time in call order. An approval
    /// or cancel queues for it too, except while a send is parked.
    effects: tokio::sync::Mutex<()>,
    /// Write order. Every effect holds it while it gates on `state`, writes
    /// to the provider and publishes its outcome, so provider frames go out
    /// in the order effects take it. A send lets go once its frame is queued
    /// if its adapter reports that before the turn ends (ACP), and takes it
    /// back to publish. Always taken after `effects`, never while waiting
    /// for it.
    dispatch: tokio::sync::Mutex<()>,
    /// True while a send's frame is queued and the send awaits the end of
    /// the provider's turn, holding `effects` but not `dispatch`.
    parked: tokio::sync::watch::Sender<bool>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl LiveSession {
    fn new(
        descriptor: RuntimeSession,
        handle: Arc<dyn SessionHandle>,
        subscriber: EventSink<RuntimeEventFrame>,
        events: Option<BoundedEventStream<RuntimeEventFrame>>,
        state: u8,
    ) -> Self {
        Self {
            descriptor: Arc::new(descriptor),
            handle,
            subscriber,
            events: Mutex::new(events),
            state: AtomicU8::new(state),
            effects: tokio::sync::Mutex::new(()),
            dispatch: tokio::sync::Mutex::new(()),
            parked: tokio::sync::watch::Sender::new(false),
            pump: Mutex::new(None),
        }
    }

    /// The turn slot for an approval or cancel, or `None` once a send is
    /// parked: the effect then goes without the slot, still after that
    /// send's frame, instead of waiting for the provider's turn to end.
    async fn serial_unless_parked(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        let mut parked = self.parked.subscribe();
        let serial = self.effects.lock();
        tokio::pin!(serial);
        loop {
            if *parked.borrow_and_update() {
                return None;
            }
            tokio::select! {
                biased;
                guard = &mut serial => return Some(guard),
                changed = parked.changed() => if changed.is_err() {
                    return Some(serial.await);
                },
            }
        }
    }

    /// The turn slot for a close. While the close waits for it, every send
    /// that parks ahead of it is cancelled once, after its frame, so the
    /// close is not held for a whole provider turn.
    async fn serial_for_close(&self) -> tokio::sync::MutexGuard<'_, ()> {
        let mut parked = self.parked.subscribe();
        let mut cancel = *parked.borrow_and_update();
        let serial = self.effects.lock();
        tokio::pin!(serial);
        loop {
            // `serial` is polled first, so the close keeps its place in the
            // queue while it cancels.
            tokio::select! {
                biased;
                guard = &mut serial => return guard,
                () = std::future::ready(()), if cancel => {
                    cancel = false;
                    self.cancel_parked_send().await;
                }
                changed = parked.changed() => {
                    if changed.is_err() {
                        return serial.await;
                    }
                    cancel = *parked.borrow_and_update();
                }
            }
        }
    }

    /// The pre-close cancel. It is part of the close, so it is admitted in
    /// the states a close is and has no audit row of its own; an unknown
    /// outcome still quarantines the session.
    async fn cancel_parked_send(&self) {
        let _dispatch = self.dispatch.lock().await;
        if !*self.parked.borrow() || !closable(self.state.load(Ordering::Acquire)) {
            return;
        }
        if matches!(
            self.handle.interrupt().await,
            DeliveryOutcome::OutcomeUnknown(_)
        ) {
            self.state.store(LIVE_OUTCOME_UNKNOWN, Ordering::Release);
        }
    }

    /// Runs a send holding `dispatch` only until its frame is queued. When
    /// the adapter reports that before the send resolves, the send parks:
    /// approvals and cancels may go until its outcome arrives. It then takes
    /// `dispatch` back so the outcome is published in write order.
    async fn send_in_order<'a>(
        &'a self,
        text: String,
        dispatch: tokio::sync::MutexGuard<'a, ()>,
    ) -> (DeliveryOutcome<()>, tokio::sync::MutexGuard<'a, ()>) {
        let (dispatched, mut queued) = tokio::sync::oneshot::channel();
        let send = self.handle.send_reporting_dispatch(text, dispatched);
        tokio::pin!(send);
        tokio::select! {
            biased;
            outcome = &mut send => return (outcome, dispatch),
            _ = &mut queued => {}
        }
        drop(dispatch);
        let outcome = {
            let _parked = ParkedSend::mark(&self.parked);
            send.await
        };
        (outcome, self.dispatch.lock().await)
    }
}

/// Holds `LiveSession::parked` true while it lives, so a delivery dropped
/// mid-turn cannot leave a send marked parked.
struct ParkedSend<'a>(&'a tokio::sync::watch::Sender<bool>);

impl<'a> ParkedSend<'a> {
    fn mark(parked: &'a tokio::sync::watch::Sender<bool>) -> Self {
        parked.send_replace(true);
        Self(parked)
    }
}

impl Drop for ParkedSend<'_> {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

#[derive(Clone)]
pub struct SessionRuntime {
    pub(crate) inner: Arc<RuntimeInner>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionRef {
    pub engine: EngineId,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSession {
    pub key: String,
    pub engine: EngineId,
    pub session_id: String,
    pub ready_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvent {
    pub session_key: String,
    pub sequence: u64,
    pub received_at: String,
    pub event: EngineEvent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeEventTermination {
    Closed,
    ProviderOverflow,
    PersistenceFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeEventFrame {
    Event {
        value: RuntimeEvent,
    },
    Terminal {
        session_key: String,
        reason: RuntimeEventTermination,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

impl BufferedEvent for RuntimeEventFrame {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Event { value } => value
                .event
                .payload_bytes()
                .saturating_add(value.session_key.len())
                .saturating_add(value.received_at.len())
                .saturating_add(size_of::<Self>()),
            Self::Terminal {
                session_key,
                message,
                ..
            } => size_of::<Self>()
                .saturating_add(session_key.len())
                .saturating_add(message.as_ref().map_or(0, String::len)),
        }
    }
    fn disposable(&self) -> bool {
        matches!(self, Self::Event { value } if value.event.disposable())
    }
    fn omission(&self) -> Self {
        match self {
            Self::Event { value } => Self::Event {
                value: RuntimeEvent {
                    session_key: value.session_key.clone(),
                    sequence: value.sequence,
                    received_at: value.received_at.clone(),
                    event: value.event.omission(),
                },
            },
            Self::Terminal { .. } => self.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum SessionOpenOutcome {
    Ready(RuntimeSession),
    DefinitelyUnsent(RuntimeError),
    DefinitelyRejected(RuntimeError),
    OutcomeUnknown(RuntimeError),
    /// Provider readiness and its exact identifier were confirmed, but local
    /// durable registration failed. Blindly starting again is not safe.
    ProviderReadyLocalFailure {
        provider: ProviderSessionRef,
        error: RuntimeError,
    },
}

impl SessionOpenOutcome {
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(self, Self::DefinitelyUnsent(_))
    }

    fn from_engine_error(error: &EngineError) -> Self {
        let runtime = RuntimeError::from_engine(error);
        match error.certainty {
            EffectCertainty::DefinitelyUnsent => Self::DefinitelyUnsent(runtime),
            EffectCertainty::DefinitelyRejected => Self::DefinitelyRejected(runtime),
            EffectCertainty::OutcomeUnknown => Self::OutcomeUnknown(runtime),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ProviderReadiness {
    Ready {
        engine: EngineId,
        info: Box<EngineInfo>,
    },
    Unavailable {
        engine: EngineId,
        error: RuntimeError,
    },
}

impl ProviderReadiness {
    fn engine(&self) -> &EngineId {
        match self {
            Self::Ready { engine, .. } | Self::Unavailable { engine, .. } => engine,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RuntimeDeliveryAudit {
    NotApplicable,
    Persisted {
        effect_id: String,
    },
    PersistenceFailed {
        effect_id: String,
        error: RuntimeError,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeDelivery {
    pub outcome: DeliveryOutcome<()>,
    pub audit: RuntimeDeliveryAudit,
}

impl RuntimeDelivery {
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        self.outcome.replay_is_safe()
    }
}

impl SessionRuntime {
    #[must_use]
    pub fn builder(services: RuntimeServices) -> SessionRuntimeBuilder {
        SessionRuntimeBuilder::new(services)
    }

    #[must_use]
    pub fn services(&self) -> &RuntimeServices {
        &self.inner.services
    }

    #[must_use]
    pub fn registered_engines(&self) -> Vec<EngineId> {
        let mut engines = self.inner.adapters.keys().cloned().collect::<Vec<_>>();
        engines.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        engines
    }

    /// Facts for an approval raised by `session_key`, from that session's adapter.
    #[must_use]
    pub fn approval_facts(
        &self,
        session_key: &str,
        tool: &str,
        input: &serde_json::Value,
    ) -> interface_contracts::agents::ApprovalFacts {
        let unknown = || interface_contracts::agents::ApprovalFacts::Unknown {
            tool: tool.to_owned(),
        };
        let Ok((engine, _)) = interface_contracts::ipc::parse_key(session_key) else {
            return unknown();
        };
        self.inner
            .adapters
            .get(&engine)
            .map_or_else(unknown, |adapter| adapter.approval_facts(tool, input))
    }

    pub(crate) fn ensure_accepting(&self) -> Result<(), RuntimeError> {
        if self.inner.lifecycle.load(Ordering::Acquire) == RUNTIME_ACCEPTING {
            Ok(())
        } else {
            Err(runtime_is_closing())
        }
    }

    /// Returns whether this process currently owns the exact live attachment.
    ///
    /// Durable history is deliberately not consulted: a forced-exit row may
    /// still say open, but only the in-memory map can authorize live effects.
    ///
    /// # Errors
    ///
    /// Fails closed if a concurrent attachment transition owns the map.
    pub fn session_is_live(&self, key: &str) -> Result<bool, RuntimeError> {
        self.inner
            .live
            .try_read()
            .map(|live| live.contains_key(key))
            .map_err(|_| attachment_changing())
    }

    /// Returns an exact detached snapshot of the process-owned live keys.
    ///
    /// # Errors
    ///
    /// Fails closed if a concurrent attachment transition owns the map.
    pub fn live_session_keys(&self) -> Result<HashSet<String>, RuntimeError> {
        self.inner
            .live
            .try_read()
            .map(|live| live.keys().cloned().collect())
            .map_err(|_| attachment_changing())
    }

    /// Quiesces provider ingress, closes every live handle, disposes every
    /// adapter, and drains runtime event pumps before storage is released.
    ///
    /// The operation is idempotent across runtime clones. A bounded failure in
    /// one provider never skips cleanup of the remaining providers.
    ///
    /// # Errors
    ///
    /// Returns the first sanitized cleanup failure after all bounded cleanup
    /// phases have been attempted.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        let _shutdown = self.inner.shutdown_lock.lock().await;
        if self.inner.lifecycle.load(Ordering::Acquire) == RUNTIME_SHUT_DOWN {
            return Ok(());
        }
        self.inner
            .lifecycle
            .store(RUNTIME_QUIESCING, Ordering::Release);
        let deadline = tokio::time::Instant::now() + RUNTIME_SHUTDOWN_TIMEOUT;
        let live = self
            .inner
            .live
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut failure = None;

        let mut closes = JoinSet::new();
        for session in &live {
            closes.spawn(close_live_for_shutdown(Arc::clone(session)));
        }
        drain_shutdown_tasks(
            &mut closes,
            deadline,
            &mut failure,
            "provider session close",
        )
        .await;

        let mut adapters = JoinSet::new();
        for adapter in self.inner.adapters.values() {
            let adapter = Arc::clone(adapter);
            adapters.spawn(async move { adapter.dispose().await.map_err(engine_runtime_error) });
        }
        drain_shutdown_tasks(
            &mut adapters,
            deadline,
            &mut failure,
            "provider adapter disposal",
        )
        .await;

        for session in &live {
            let Some(mut pump) = take_pump(session) else {
                continue;
            };
            match tokio::time::timeout_at(deadline, &mut pump).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => record_shutdown_failure(
                    &mut failure,
                    RuntimeError::definitely_unsent(
                        RuntimeErrorKind::Internal,
                        format!("runtime event pump failed during shutdown: {error}"),
                        false,
                    ),
                ),
                Err(_) => {
                    pump.abort();
                    record_shutdown_failure(&mut failure, shutdown_timeout("runtime event drain"));
                }
            }
        }
        let _ = tokio::time::timeout_at(deadline, self.inner.project_roots.wait_idle()).await;
        self.inner.services.flush_registry_cache().await;
        self.inner.live.write().await.clear();
        self.inner
            .lifecycle
            .store(RUNTIME_SHUT_DOWN, Ordering::Release);
        failure.map_or(Ok(()), Err)
    }

    /// A provider's readiness for a launch, health or catalogue read. A
    /// successful probe is reused while it is young and the provider's
    /// executable is unchanged; a failure is never kept.
    pub async fn probe_provider(&self, engine: &EngineId) -> ProviderReadiness {
        self.probe_provider_with(engine, None).await
    }

    /// `fresh_after`: an explicit refresh's count of probes started when it
    /// asked; only a later probe answers it.
    async fn probe_provider_with(
        &self,
        engine: &EngineId,
        fresh_after: Option<u64>,
    ) -> ProviderReadiness {
        if let Err(error) = self.ensure_accepting() {
            return ProviderReadiness::Unavailable {
                engine: engine.clone(),
                error,
            };
        }
        let Some(adapter) = self.inner.adapters.get(engine) else {
            return ProviderReadiness::Unavailable {
                engine: engine.clone(),
                error: engine_missing(engine),
            };
        };
        // Concurrent health, launch and orchestration reads share a successful
        // probe. The gate is per provider, so one slow CLI cannot block others.
        // Recheck the kept probe after waiting; failures retain immediate retry.
        let _probe = self.inner.provider_probe_locks[engine].lock().await;
        if let Err(error) = self.ensure_accepting() {
            return ProviderReadiness::Unavailable {
                engine: engine.clone(),
                error,
            };
        }
        // Taken before the probe runs: a binary replaced while it runs no
        // longer matches, so its answer is asked for again next time.
        let executable = adapter.executable().and_then(|path| FileVersion::of(&path));
        if let Some(kept) = self.inner.kept_probes.read().await.get(engine)
            && kept.serves(executable, fresh_after)
        {
            // No record_model_catalogue here on purpose: this EngineInfo was
            // recorded by the probe that kept it, and nothing ever clears
            // model_catalogues, so re-recording would only rewrite the same rows.
            return ProviderReadiness::Ready {
                engine: engine.clone(),
                info: kept.info.clone(),
            };
        }
        let started = self
            .inner
            .probe_starts
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        match adapter.probe().await {
            Ok(info) => {
                self.record_model_catalogue(engine, &info).await;
                let info = Box::new(info);
                self.inner.kept_probes.write().await.insert(
                    engine.clone(),
                    KeptProbe {
                        executable,
                        started,
                        finished: Instant::now(),
                        info: info.clone(),
                    },
                );
                ProviderReadiness::Ready {
                    engine: engine.clone(),
                    info,
                }
            }
            Err(error) => {
                self.inner.kept_probes.write().await.remove(engine);
                ProviderReadiness::Unavailable {
                    engine: engine.clone(),
                    error: RuntimeError::from_engine(&error),
                }
            }
        }
    }

    /// Returns the provider catalogue used by orchestration, probing at most
    /// once on the first orchestration request when no earlier probe populated
    /// the cache.
    pub async fn orchestration_model_catalogue(
        &self,
        engine: &EngineId,
    ) -> Option<Vec<ModelOption>> {
        if let Some(models) = self.inner.model_catalogues.read().await.get(engine) {
            return Some(models.clone());
        }
        let probe = self.inner.model_probe_locks.get(engine)?.clone();
        let _probe = probe.lock().await;
        if let Some(models) = self.inner.model_catalogues.read().await.get(engine) {
            return Some(models.clone());
        }
        if !self
            .inner
            .orchestration_model_probes
            .write()
            .await
            .insert(engine.clone())
        {
            return self
                .inner
                .model_catalogues
                .read()
                .await
                .get(engine)
                .cloned();
        }
        let _ = self.probe_provider(engine).await;
        self.inner
            .model_catalogues
            .read()
            .await
            .get(engine)
            .cloned()
    }

    async fn record_model_catalogue(&self, engine: &EngineId, info: &EngineInfo) {
        let Some(fresh) = info
            .model_catalog
            .as_ref()
            .filter(|models| !models.is_empty())
        else {
            return;
        };
        let mut catalogues = self.inner.model_catalogues.write().await;
        let existing = catalogues.get(engine).cloned().unwrap_or_default();
        let mut merged = fresh.clone();
        let mut seen = merged
            .iter()
            .map(|model| catalogue_model_key(engine, &model.value))
            .collect::<HashSet<_>>();
        for model in existing {
            if seen.insert(catalogue_model_key(engine, &model.value)) {
                merged.push(model);
            }
        }
        catalogues.insert(engine.clone(), merged);
    }

    /// The explicit provider refresh: probes every provider again, reusing
    /// only a probe that started after this call, and restarts every kept
    /// session listing so the next listing asks each provider again.
    pub async fn probe_all_providers(&self) -> Vec<ProviderReadiness> {
        let asked = self.inner.probe_starts.load(Ordering::Acquire);
        self.inner.management.restart_listings();
        let engines = self.registered_engines();
        let mut work = JoinSet::new();
        for engine in engines {
            let runtime = self.clone();
            work.spawn(async move { runtime.probe_provider_with(&engine, Some(asked)).await });
        }
        let mut readiness = Vec::with_capacity(work.len());
        while let Some(joined) = work.join_next().await {
            if let Ok(item) = joined {
                readiness.push(item);
            }
        }
        readiness.sort_by(|left, right| left.engine().as_str().cmp(right.engine().as_str()));
        readiness
    }

    pub async fn start_session(
        &self,
        engine: &EngineId,
        options: LaunchOptions,
    ) -> SessionOpenOutcome {
        let mut options = options;
        if let Err(error) = self.pin_chat_launch(&mut options) {
            return SessionOpenOutcome::DefinitelyUnsent(error);
        }
        self.open_session(engine, OpenRequest::Start(options)).await
    }

    /// Pins a Chat launch onto this runtime's chat folder.
    ///
    /// Code and Agent launches, including an omitted kind, are left unchanged.
    /// A Chat launch never keeps the caller's cwd or a worktree request.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent invalid-input error when Chat is requested
    /// and this runtime has no chat folder configured.
    pub fn pin_chat_launch(&self, options: &mut LaunchOptions) -> Result<(), RuntimeError> {
        Self::pin_chat_launch_with(self.inner.chat_home.as_deref(), options)
    }

    /// Pins a Chat launch onto `chat_home`.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent invalid-input error when Chat is requested
    /// and `chat_home` is absent or empty.
    pub fn pin_chat_launch_with(
        chat_home: Option<&Path>,
        options: &mut LaunchOptions,
    ) -> Result<(), RuntimeError> {
        if session_kind_of(options.kind) != SessionKind::Chat {
            return Ok(());
        }
        let Some(chat_home) = chat_home.filter(|path| !path.as_os_str().is_empty()) else {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                CHAT_UNAVAILABLE,
                false,
            ));
        };
        options.cwd = chat_home.to_string_lossy().into_owned();
        options.worktree = None;
        Ok(())
    }

    pub async fn resume_session(
        &self,
        engine: &EngineId,
        provider_session_id: String,
        cwd: String,
        options: ResumeOptions,
    ) -> SessionOpenOutcome {
        self.open_session(
            engine,
            OpenRequest::Resume {
                provider_session_id,
                cwd,
                options,
            },
        )
        .await
    }

    async fn open_session(&self, engine: &EngineId, request: OpenRequest) -> SessionOpenOutcome {
        if let Err(error) = self.ensure_accepting() {
            return SessionOpenOutcome::DefinitelyUnsent(error);
        }
        let Some(adapter) = self.inner.adapters.get(engine).cloned() else {
            return SessionOpenOutcome::DefinitelyUnsent(engine_missing(engine));
        };
        let prepared = match self
            .prepare_provider_open(engine, adapter.as_ref(), request)
            .await
        {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
        self.activate_prepared_session(engine, prepared).await
    }

    async fn prepare_provider_open(
        &self,
        engine: &EngineId,
        adapter: &dyn EngineAdapter,
        request: OpenRequest,
    ) -> Result<PreparedOpen, SessionOpenOutcome> {
        let metadata = request.metadata();
        let requested_resume_id = request.resume_id().map(ToOwned::to_owned);
        let mut handle = match request.create_handle(adapter) {
            Ok(handle) => handle,
            Err(error) => return Err(SessionOpenOutcome::from_engine_error(&error)),
        };
        if handle.engine() != engine {
            let _ = handle.close().await;
            return Err(SessionOpenOutcome::DefinitelyUnsent(
                RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Lifecycle,
                    "adapter returned a handle for a different engine",
                    false,
                ),
            ));
        }
        let stream = match handle.take_events() {
            Ok(stream) => stream,
            Err(error) => {
                let _ = handle.close().await;
                return Err(SessionOpenOutcome::from_engine_error(&error));
            }
        };
        let (ingress_tx, ingress_rx) =
            mpsc::channel(self.inner.configuration.pre_ready_event_capacity);
        tokio::spawn(forward_provider_events(stream, ingress_tx));

        let ready = match handle.ready().await {
            Ok(ready) => ready,
            Err(error) => {
                drop(ingress_rx);
                let _ = handle.close().await;
                return Err(SessionOpenOutcome::from_engine_error(&error));
            }
        };
        let provider = ProviderSessionRef {
            engine: engine.clone(),
            session_id: ready.session_id.clone(),
        };
        if let Err(error) = validate_ready_session(
            &ready.session_id,
            &handle.session_id(),
            requested_resume_id.as_deref(),
        ) {
            drop(ingress_rx);
            let _ = handle.close().await;
            return Err(SessionOpenOutcome::ProviderReadyLocalFailure { provider, error });
        }
        Ok(PreparedOpen {
            handle,
            ingress_rx,
            provider,
            metadata,
        })
    }

    #[allow(clippy::too_many_lines)] // Readiness, persistence, and pump ownership form one transaction.
    async fn activate_prepared_session(
        &self,
        engine: &EngineId,
        prepared: PreparedOpen,
    ) -> SessionOpenOutcome {
        let prepared = match self.fence_prepared_activation(prepared).await {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
        let PreparedOpen {
            handle,
            ingress_rx,
            provider,
            metadata,
        } = prepared;
        let ready_at = self.inner.clock.now();
        let key = session_key(engine, &provider.session_id);
        let descriptor = RuntimeSession {
            key: key.clone(),
            engine: engine.clone(),
            session_id: provider.session_id.clone(),
            ready_at: ready_at.clone(),
        };
        let summary = metadata.summary(engine.clone(), provider.session_id.clone(), &ready_at);

        {
            let live = self.inner.live.read().await;
            if live.contains_key(&key) {
                drop(live);
                drop(ingress_rx);
                let _ = handle.close().await;
                return SessionOpenOutcome::ProviderReadyLocalFailure {
                    provider,
                    error: RuntimeError::definitely_unsent(
                        RuntimeErrorKind::Lifecycle,
                        format!("session {key} already has a live attachment"),
                        false,
                    ),
                };
            }
        }

        if let Err(error) = persist_registration(
            self.inner.services.clone(),
            Arc::clone(&self.inner.project_roots),
            key.clone(),
            summary,
            ready_at.clone(),
        )
        .await
        {
            let _ = mark_persistence_failed(
                self.inner.services.clone(),
                key,
                error.message.clone(),
                ready_at,
            )
            .await;
            drop(ingress_rx);
            let _ = handle.close().await;
            return SessionOpenOutcome::ProviderReadyLocalFailure { provider, error };
        }

        let (subscriber, events) = bounded_event_stream_with_budget(
            self.inner.configuration.subscriber_event_capacity,
            interface_engine::event_budget::SUBSCRIBER_EVENT_BYTES,
        );
        let live = Arc::new(LiveSession::new(
            descriptor.clone(),
            Arc::from(handle),
            subscriber,
            Some(events),
            LIVE_ACTIVE,
        ));
        let inserted = {
            let mut sessions = self.inner.live.write().await;
            match sessions.entry(key.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Arc::clone(&live));
                    true
                }
                std::collections::hash_map::Entry::Occupied(_) => false,
            }
        };
        if !inserted {
            live.state.store(LIVE_PERSISTENCE_FAILED, Ordering::Release);
            let _ = live.handle.close().await;
            return SessionOpenOutcome::ProviderReadyLocalFailure {
                provider,
                error: RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Lifecycle,
                    format!("session {key} was attached concurrently"),
                    false,
                ),
            };
        }
        let task = tokio::spawn(run_event_pump(
            self.inner.services.clone(),
            Arc::clone(&self.inner.project_roots),
            Arc::clone(&self.inner.clock),
            Arc::downgrade(&self.inner),
            Arc::clone(&live),
            ingress_rx,
        ));
        match live.pump.lock() {
            Ok(mut pump) => *pump = Some(task),
            Err(_) => task.abort(),
        }
        SessionOpenOutcome::Ready(descriptor)
    }

    async fn fence_prepared_activation(
        &self,
        prepared: PreparedOpen,
    ) -> Result<PreparedOpen, SessionOpenOutcome> {
        if self.ensure_accepting().is_ok() {
            return Ok(prepared);
        }
        drop(prepared.ingress_rx);
        let _ = prepared.handle.close().await;
        Err(SessionOpenOutcome::ProviderReadyLocalFailure {
            provider: prepared.provider,
            error: runtime_is_closing(),
        })
    }

    /// Refreshes every provider store and returns the canonical visible session projection.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry mutex was poisoned or its blocking task failed.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        self.list_managed_sessions().await
    }

    /// Transfers the runtime-owned, post-projection event stream to one caller.
    ///
    /// # Errors
    ///
    /// Refuses inactive sessions and a second consumer.
    pub async fn take_session_events(
        &self,
        key: &str,
    ) -> Result<BoundedEventStream<RuntimeEventFrame>, RuntimeError> {
        let live = self.live_session(key).await?;
        lock(&live.events, "runtime event receiver")?
            .take()
            .ok_or_else(|| {
                RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Lifecycle,
                    "runtime event stream already has a consumer",
                    false,
                )
            })
    }

    /// Returns one session's bounded durable event and delivery history.
    ///
    /// # Errors
    ///
    /// Returns an error when the history mutex is poisoned or its blocking task fails.
    pub async fn history_session(
        &self,
        key: &str,
    ) -> Result<Option<crate::HistorySession>, RuntimeError> {
        let services = self.inner.services.clone();
        let key = key.to_owned();
        blocking(move || Ok(lock(&services.history, "engine history")?.session(&key))).await
    }

    /// The history session's scalar state, without its retained events.
    ///
    /// # Errors
    ///
    /// Returns an error when the history mutex is poisoned or its blocking task fails.
    pub async fn history_session_state(
        &self,
        key: &str,
    ) -> Result<Option<crate::HistorySessionState>, RuntimeError> {
        let services = self.inner.services.clone();
        let key = key.to_owned();
        blocking(move || Ok(lock(&services.history, "engine history")?.session_state(&key))).await
    }

    /// The history session's state plus only the events after
    /// `after_sequence`.
    ///
    /// # Errors
    ///
    /// Returns an error when the history mutex is poisoned or its blocking task fails.
    pub async fn history_events_after(
        &self,
        key: &str,
        after_sequence: u64,
    ) -> Result<Option<(crate::HistorySessionState, Vec<crate::HistoryEvent>)>, RuntimeError> {
        let services = self.inner.services.clone();
        let key = key.to_owned();
        blocking(move || {
            Ok(lock(&services.history, "engine history")?
                .session_events_after(&key, after_sequence))
        })
        .await
    }

    /// Returns the complete bounded durable engine history snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the history mutex is poisoned or its blocking task fails.
    pub async fn history_snapshot(&self) -> Result<RuntimeHistorySnapshot, RuntimeError> {
        let services = self.inner.services.clone();
        blocking(move || Ok(lock(&services.history, "engine history")?.snapshot())).await
    }

    pub async fn send(&self, key: &str, text: String) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::Send(text)).await
    }

    pub async fn cancel_session(&self, key: &str) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::Cancel).await
    }

    pub async fn respond_to_approval(
        &self,
        key: &str,
        approval_id: String,
        verdict: ApprovalVerdict,
        response: ApprovalResponse,
    ) -> RuntimeDelivery {
        self.deliver(
            key,
            EffectOperation::Approval {
                approval_id,
                verdict,
                response,
            },
        )
        .await
    }

    pub async fn set_model(&self, key: &str, model: Option<String>) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::SetModel(model)).await
    }

    pub async fn set_effort(&self, key: &str, effort: Option<String>) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::SetEffort(effort)).await
    }

    pub async fn set_fast_mode(&self, key: &str, enabled: bool) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::SetFastMode(enabled))
            .await
    }

    pub async fn set_permission_mode(&self, key: &str, mode: String) -> RuntimeDelivery {
        self.deliver(key, EffectOperation::SetPermissionMode(mode))
            .await
    }

    pub async fn close_session(&self, key: &str) -> RuntimeDelivery {
        let delivery = self.deliver(key, EffectOperation::Close).await;
        if delivery.outcome.is_confirmed() {
            let live = self.inner.live.read().await.get(key).cloned();
            if let Some(mut pump) = live.as_ref().and_then(take_pump) {
                if tokio::time::timeout(PUMP_SHUTDOWN_TIMEOUT, &mut pump)
                    .await
                    .is_err()
                {
                    pump.abort();
                }
            }
            if let Some(live) = live {
                let mut attachments = self.inner.live.write().await;
                remove_if_current(&mut attachments, &live.descriptor.key, &live);
            }
        }
        delivery
    }

    async fn deliver(&self, key: &str, operation: EffectOperation) -> RuntimeDelivery {
        if let Err(error) = self.ensure_accepting() {
            return local_refusal(&error);
        }
        let live = match self.live_session(key).await {
            Ok(live) => live,
            Err(error) => return local_refusal(&error),
        };
        let kind = operation.kind();
        // Turn slot first, then write order; see `LiveSession::effects`.
        let _serial = match kind {
            RuntimeEffectKind::Approval | RuntimeEffectKind::Cancel => {
                live.serial_unless_parked().await
            }
            RuntimeEffectKind::Close => Some(live.serial_for_close().await),
            _ => Some(live.effects.lock().await),
        };
        let dispatch = live.dispatch.lock().await;
        if let Err(error) = gate_delivery(&live, kind) {
            return local_refusal(&error);
        }
        let before_close = live.state.load(Ordering::Acquire);
        if kind == RuntimeEffectKind::Close {
            live.state.store(LIVE_CLOSING, Ordering::Release);
        }
        let (outcome, _dispatch) = match operation {
            EffectOperation::Send(text) => live.send_in_order(text, dispatch).await,
            operation => (operation.invoke(live.handle.as_ref()).await, dispatch),
        };
        if matches!(outcome, DeliveryOutcome::OutcomeUnknown(_)) {
            live.state.store(LIVE_OUTCOME_UNKNOWN, Ordering::Release);
        } else if kind == RuntimeEffectKind::Close {
            live.state.store(
                if outcome.is_confirmed() {
                    LIVE_CLOSED
                } else {
                    before_close
                },
                Ordering::Release,
            );
        }
        let effect_id = Uuid::new_v4().to_string();
        let completed_at = self.inner.clock.now();
        let audit = match persist_effect(
            self.inner.services.clone(),
            key.to_owned(),
            effect_id.clone(),
            kind,
            completed_at,
            outcome.clone(),
        )
        .await
        {
            Ok(()) => RuntimeDeliveryAudit::Persisted { effect_id },
            Err(error) => {
                if live.state.load(Ordering::Acquire) != LIVE_OUTCOME_UNKNOWN {
                    live.state.store(LIVE_PERSISTENCE_FAILED, Ordering::Release);
                }
                RuntimeDeliveryAudit::PersistenceFailed { effect_id, error }
            }
        };
        RuntimeDelivery { outcome, audit }
    }

    async fn live_session(&self, key: &str) -> Result<Arc<LiveSession>, RuntimeError> {
        self.inner
            .live
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::definitely_unsent(
                    RuntimeErrorKind::SessionNotActive,
                    format!("session {key} has no live attachment"),
                    false,
                )
            })
    }
}

async fn close_live_for_shutdown(live: Arc<LiveSession>) -> Result<(), RuntimeError> {
    let _serial = live.serial_for_close().await;
    let _dispatch = live.dispatch.lock().await;
    if live.state.load(Ordering::Acquire) == LIVE_CLOSED {
        return Ok(());
    }
    live.state.store(LIVE_CLOSING, Ordering::Release);
    match live.handle.close().await {
        Ok(()) => {
            live.state.store(LIVE_CLOSED, Ordering::Release);
            Ok(())
        }
        Err(error) => {
            live.state.store(LIVE_OUTCOME_UNKNOWN, Ordering::Release);
            Err(engine_runtime_error(error))
        }
    }
}

async fn drain_shutdown_tasks(
    tasks: &mut JoinSet<Result<(), RuntimeError>>,
    deadline: tokio::time::Instant,
    failure: &mut Option<RuntimeError>,
    phase: &str,
) {
    while !tasks.is_empty() {
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(Ok(Ok(())))) => {}
            Ok(Some(Ok(Err(error)))) => record_shutdown_failure(failure, error),
            Ok(Some(Err(error))) => record_shutdown_failure(
                failure,
                RuntimeError::definitely_unsent(
                    RuntimeErrorKind::Internal,
                    format!("{phase} task failed during shutdown: {error}"),
                    false,
                ),
            ),
            Ok(None) => break,
            Err(_) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                record_shutdown_failure(failure, shutdown_timeout(phase));
                break;
            }
        }
    }
}

fn runtime_is_closing() -> RuntimeError {
    RuntimeError::definitely_unsent(
        RuntimeErrorKind::Lifecycle,
        "Interface is closing; no new provider action was taken.",
        false,
    )
}

fn catalogue_model_key(engine: &EngineId, value: &str) -> String {
    let value = value.trim();
    if engine.as_str() == "claude"
        && let Some(without_closing) = value.strip_suffix(']')
        && let Some(opening) = without_closing.rfind('[')
    {
        return without_closing[..opening].to_owned();
    }
    value.to_owned()
}

fn shutdown_timeout(phase: &str) -> RuntimeError {
    RuntimeError::definitely_unsent(
        RuntimeErrorKind::Lifecycle,
        format!("{phase} timed out during runtime shutdown"),
        false,
    )
}

fn record_shutdown_failure(slot: &mut Option<RuntimeError>, error: RuntimeError) {
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn take_pump(live: &Arc<LiveSession>) -> Option<JoinHandle<()>> {
    match live.pump.lock() {
        Ok(mut pump) => pump.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

#[derive(Clone)]
enum OpenRequest {
    Start(LaunchOptions),
    Resume {
        provider_session_id: String,
        cwd: String,
        options: ResumeOptions,
    },
}

struct PreparedOpen {
    handle: Box<dyn SessionHandle>,
    ingress_rx: mpsc::Receiver<ProviderIngress>,
    provider: ProviderSessionRef,
    metadata: OpenMetadata,
}

fn prompt_title(prompt: &str) -> Option<String> {
    let single_line = prompt
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let title: String = single_line.chars().take(80).collect();
    let title = title.trim_end();
    (!title.is_empty()).then(|| title.to_owned())
}

impl OpenRequest {
    fn metadata(&self) -> OpenMetadata {
        match self {
            Self::Start(options) => OpenMetadata {
                cwd: options.cwd.clone(),
                title: options.name.clone().or_else(|| {
                    // Chat and Code (a Threads project thread; an omitted
                    // kind is Code) are listed by title, and provider history
                    // can arrive long after the conversation first appears.
                    // Agent and Voice launches keep their own naming.
                    let seeded = matches!(
                        session_kind_of(options.kind),
                        SessionKind::Chat | SessionKind::Code
                    );
                    seeded.then(|| prompt_title(&options.prompt)).flatten()
                }),
                model: options.model.clone(),
                effort: options.effort.clone(),
                cloud: options.cloud.unwrap_or(false),
                setting_sources: options.setting_sources.clone(),
                disallowed_tools: options.disallowed_tools.clone(),
                starts_working: !options.prompt.is_empty(),
                kind: options.kind,
            },
            Self::Resume { cwd, options, .. } => OpenMetadata {
                cwd: cwd.clone(),
                title: None,
                model: options.model.clone(),
                effort: options.effort.clone(),
                cloud: false,
                setting_sources: options.setting_sources.clone(),
                disallowed_tools: options.disallowed_tools.clone(),
                starts_working: false,
                kind: options.kind,
            },
        }
    }

    fn resume_id(&self) -> Option<&str> {
        match self {
            Self::Start(_) => None,
            Self::Resume {
                provider_session_id,
                ..
            } => Some(provider_session_id),
        }
    }

    fn create_handle(
        self,
        adapter: &dyn EngineAdapter,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        match self {
            Self::Start(options) => adapter.create_session(options),
            Self::Resume {
                provider_session_id,
                cwd,
                options,
            } => adapter.resume_session(provider_session_id, cwd, options),
        }
    }
}

struct OpenMetadata {
    cwd: String,
    title: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    cloud: bool,
    setting_sources: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    starts_working: bool,
    kind: Option<SessionKind>,
}

impl OpenMetadata {
    fn summary(self, engine: EngineId, provider_session_id: String, at: &str) -> SessionSummary {
        SessionSummary {
            engine,
            title: self.title.unwrap_or_else(|| provider_session_id.clone()),
            session_id: provider_session_id,
            cwd: self.cwd,
            status: if self.starts_working {
                SessionStatus::Working
            } else {
                SessionStatus::Idle
            },
            cloud: self.cloud,
            model: self.model,
            effort: self.effort.map(Some),
            last_activity: at.to_owned(),
            first_seen: at.to_owned(),
            archived: false,
            size: None,
            title_from_summary: None,
            origin: Some(SessionOrigin::App),
            project_root: None,
            worktree_home: None,
            worktree_path: None,
            setting_sources: self.setting_sources,
            disallowed_tools: self.disallowed_tools,
            kind: self.kind,
            permission_mode: None,
        }
    }
}

pub(crate) fn remove_if_current<K, T>(
    entries: &mut HashMap<K, Arc<T>>,
    key: &K,
    entry: &Arc<T>,
) -> bool
where
    K: Eq + std::hash::Hash,
{
    if entries
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, entry))
    {
        entries.remove(key);
        true
    } else {
        false
    }
}

enum EffectOperation {
    Send(String),
    Cancel,
    Approval {
        approval_id: String,
        verdict: ApprovalVerdict,
        response: ApprovalResponse,
    },
    SetModel(Option<String>),
    SetEffort(Option<String>),
    SetFastMode(bool),
    SetPermissionMode(String),
    Close,
}

impl EffectOperation {
    const fn kind(&self) -> RuntimeEffectKind {
        match self {
            Self::Send(_) => RuntimeEffectKind::Send,
            Self::Cancel => RuntimeEffectKind::Cancel,
            Self::Approval { .. } => RuntimeEffectKind::Approval,
            Self::SetModel(_) => RuntimeEffectKind::SetModel,
            Self::SetEffort(_) => RuntimeEffectKind::SetEffort,
            Self::SetFastMode(_) => RuntimeEffectKind::SetFastMode,
            Self::SetPermissionMode(_) => RuntimeEffectKind::SetPermissionMode,
            Self::Close => RuntimeEffectKind::Close,
        }
    }

    async fn invoke(self, handle: &dyn SessionHandle) -> DeliveryOutcome<()> {
        match self {
            Self::Send(text) => handle.send(text).await,
            Self::Cancel => handle.interrupt().await,
            Self::Approval {
                approval_id,
                verdict,
                response,
            } => {
                handle
                    .respond_to_approval(approval_id, verdict, response)
                    .await
            }
            Self::SetModel(model) => handle.set_model(model).await,
            Self::SetEffort(effort) => handle.set_effort(effort).await,
            Self::SetFastMode(enabled) => handle.set_fast_mode(enabled).await,
            Self::SetPermissionMode(mode) => handle.set_permission_mode(mode).await,
            Self::Close => match handle.close().await {
                Ok(()) => DeliveryOutcome::Confirmed(()),
                Err(error) => DeliveryOutcome::from_error(error),
            },
        }
    }
}

/// A close is admitted from these states; every other effect only from
/// `LIVE_ACTIVE`.
const fn closable(state: u8) -> bool {
    matches!(
        state,
        LIVE_ACTIVE | LIVE_OUTCOME_UNKNOWN | LIVE_PERSISTENCE_FAILED
    )
}

fn gate_delivery(live: &LiveSession, kind: RuntimeEffectKind) -> Result<(), RuntimeError> {
    if kind == RuntimeEffectKind::Close && closable(live.state.load(Ordering::Acquire)) {
        return Ok(());
    }
    let (kind, message) = match live.state.load(Ordering::Acquire) {
        LIVE_ACTIVE => return Ok(()),
        LIVE_OUTCOME_UNKNOWN => (
            RuntimeErrorKind::Lifecycle,
            "session is quarantined after an unknown provider outcome; detach and reconcile before another effect",
        ),
        LIVE_PERSISTENCE_FAILED => (
            RuntimeErrorKind::History,
            "session is quarantined because durable event or effect projection failed",
        ),
        LIVE_CLOSING => (RuntimeErrorKind::Lifecycle, "session is closing"),
        LIVE_CLOSED => (RuntimeErrorKind::Lifecycle, "session is closed"),
        _ => (
            RuntimeErrorKind::Internal,
            "session has an invalid runtime state",
        ),
    };
    Err(RuntimeError::definitely_unsent(kind, message, false))
}

fn local_refusal(error: &RuntimeError) -> RuntimeDelivery {
    let engine_error = EngineError::definitely_unsent(
        EngineErrorKind::Lifecycle,
        error.message.clone(),
        error.retryable,
    );
    RuntimeDelivery {
        outcome: DeliveryOutcome::DefinitelyUnsent(engine_error),
        audit: RuntimeDeliveryAudit::NotApplicable,
    }
}

fn validate_ready_session(
    ready: &str,
    handle_session_id: &str,
    requested_resume_id: Option<&str>,
) -> Result<(), RuntimeError> {
    let safe = !ready.is_empty()
        && ready.len() <= 4_096
        && !ready.chars().any(char::is_control)
        && ready == handle_session_id
        && requested_resume_id.is_none_or(|requested| requested == ready);
    if safe {
        Ok(())
    } else {
        Err(RuntimeError::definitely_unsent(
            RuntimeErrorKind::Lifecycle,
            "provider readiness did not yield the exact expected session identifier",
            false,
        ))
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::Internal,
                format!("runtime storage task failed: {error}"),
                true,
            )
        })?
}

pub(crate) fn lock<'a, T>(
    mutex: &'a Mutex<T>,
    name: &str,
) -> Result<MutexGuard<'a, T>, RuntimeError> {
    mutex.lock().map_err(|_| {
        RuntimeError::definitely_unsent(
            RuntimeErrorKind::Internal,
            format!("{name} mutex is poisoned"),
            false,
        )
    })
}

// By-value signatures so these slot into `.map_err(...)` point-free; the
// bodies only borrow, hence the allow.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn registry_error(error: interface_registry::RegistryError) -> RuntimeError {
    RuntimeError::definitely_unsent(RuntimeErrorKind::Registry, error.to_string(), true)
}

#[allow(clippy::needless_pass_by_value)]
fn engine_runtime_error(error: EngineError) -> RuntimeError {
    RuntimeError::from_engine(&error)
}

pub(crate) fn engine_missing(engine: &EngineId) -> RuntimeError {
    RuntimeError::definitely_unsent(
        RuntimeErrorKind::EngineNotRegistered,
        format!("engine {engine} is not registered"),
        false,
    )
}

fn attachment_changing() -> RuntimeError {
    RuntimeError::definitely_unsent(
        RuntimeErrorKind::Lifecycle,
        "session attachment state is changing; try again",
        true,
    )
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
