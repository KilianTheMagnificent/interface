use std::{
    collections::VecDeque,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use engine_acp::{AcpAdapterConfig, BuiltinAcpProvider};
use engine_codex::CodexAdapterConfig;
use interface_contracts::engine::{
    EngineCapabilities, EngineEvent, EngineEventKind, EngineId, EngineInfo, EngineModels,
    EngineSessionListing, LaunchOptions, ModelOption, ResumeOptions, SessionKind, SessionOrigin,
    SessionStatus, SessionSummary,
};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EngineAdapter,
    EngineError, EngineErrorKind, EventSink, ReadySession, SessionHandle, bounded_event_stream,
};
use interface_orchestration::OrchestrationStore;
use interface_registry::{SessionRegistry, read_registry_file};
use interface_runtime::{
    EffectAuditDisposition, EngineHistoryStore, HistoryLimits, NativeAdapterConfiguration,
    ProviderReadiness, RuntimeBridgeCommand, RuntimeBridgeResponse, RuntimeClock,
    RuntimeConfiguration, RuntimeDeliveryAudit, RuntimeDeliveryState, RuntimeErrorKind,
    RuntimeEventFrame, RuntimeEventTermination, RuntimeServices, RuntimeStreamState,
    SessionOpenOutcome, SessionRuntime,
};
use tempfile::TempDir;
use tokio::sync::Notify;

struct Harness {
    runtime: SessionRuntime,
    control: Arc<FakeControl>,
    engine: EngineId,
    root: TempDir,
}

struct FakeControl {
    exact_session_id: String,
    provider_capacity: usize,
    ready_released: AtomicBool,
    ready_changed: Notify,
    created: AtomicUsize,
    created_changed: Notify,
    sinks: Mutex<Vec<EventSink<EngineEvent>>>,
    send_outcomes: Mutex<VecDeque<DeliveryOutcome<()>>>,
    interrupt_outcomes: Mutex<VecDeque<DeliveryOutcome<()>>>,
    send_calls: AtomicUsize,
    interrupt_calls: AtomicUsize,
    close_calls: AtomicUsize,
    dispose_calls: AtomicUsize,
    probe_calls: AtomicUsize,
    probe_fails: AtomicBool,
    probe_gate: Mutex<Option<Arc<Notify>>>,
    /// The file the fake adapter reports launching; unset, it has none.
    executable: OnceLock<PathBuf>,
    overflow_on_open: AtomicBool,
    listed: Mutex<Vec<SessionSummary>>,
    launched: Mutex<Vec<LaunchOptions>>,
    /// Sends park like ACP's `session/prompt`: the frame goes out, and the
    /// send resolves only when the turn ends (an interrupt or a close).
    park_sends: AtomicBool,
    turn_open: AtomicBool,
    turn_changed: Notify,
    /// Provider frames in the order the fake handle wrote them.
    frames: Mutex<Vec<&'static str>>,
}

impl FakeControl {
    fn new(provider_capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            exact_session_id: "provider-session-1".into(),
            provider_capacity,
            ready_released: AtomicBool::new(false),
            ready_changed: Notify::new(),
            created: AtomicUsize::new(0),
            created_changed: Notify::new(),
            sinks: Mutex::new(Vec::new()),
            send_outcomes: Mutex::new(VecDeque::new()),
            interrupt_outcomes: Mutex::new(VecDeque::new()),
            send_calls: AtomicUsize::new(0),
            interrupt_calls: AtomicUsize::new(0),
            close_calls: AtomicUsize::new(0),
            dispose_calls: AtomicUsize::new(0),
            probe_calls: AtomicUsize::new(0),
            probe_fails: AtomicBool::new(false),
            probe_gate: Mutex::new(None),
            executable: OnceLock::new(),
            overflow_on_open: AtomicBool::new(false),
            listed: Mutex::new(Vec::new()),
            launched: Mutex::new(Vec::new()),
            park_sends: AtomicBool::new(false),
            turn_open: AtomicBool::new(false),
            turn_changed: Notify::new(),
            frames: Mutex::new(Vec::new()),
        })
    }

    fn frame(&self, frame: &'static str) {
        self.frames.lock().expect("fake frame log").push(frame);
    }

    fn frames(&self) -> Vec<&'static str> {
        self.frames.lock().expect("fake frame log").clone()
    }

    fn end_turn(&self) {
        self.turn_open.store(false, Ordering::Release);
        self.turn_changed.notify_waiters();
    }

    async fn wait_turn_end(&self) {
        loop {
            // Registered before the check, so an end between the two wakes it.
            let changed = self.turn_changed.notified();
            if !self.turn_open.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }

    async fn wait_created(&self) {
        while self.created.load(Ordering::Acquire) == 0 {
            self.created_changed.notified().await;
        }
    }

    fn release_ready(&self) {
        self.ready_released.store(true, Ordering::Release);
        self.ready_changed.notify_waiters();
    }

    async fn wait_ready(&self) {
        while !self.ready_released.load(Ordering::Acquire) {
            self.ready_changed.notified().await;
        }
    }

    fn emit(&self, event: EngineEvent) {
        let sink = self
            .sinks
            .lock()
            .expect("fake sink mutex")
            .last()
            .expect("fake session was created")
            .clone();
        sink.emit(event).expect("fake provider event should fit");
    }

    fn close_current_stream(&self) {
        self.sinks
            .lock()
            .expect("fake sink mutex")
            .last()
            .expect("fake session was created")
            .close();
    }

    fn push_send(&self, outcome: DeliveryOutcome<()>) {
        self.send_outcomes
            .lock()
            .expect("fake send queue")
            .push_back(outcome);
    }

    fn push_interrupt(&self, outcome: DeliveryOutcome<()>) {
        self.interrupt_outcomes
            .lock()
            .expect("fake interrupt queue")
            .push_back(outcome);
    }

    fn fail_probe(&self, failing: bool) {
        self.probe_fails.store(failing, Ordering::Release);
    }

    fn set_listing(&self, sessions: Vec<SessionSummary>) {
        *self.listed.lock().expect("fake listing mutex") = sessions;
    }
}

struct FakeAdapter {
    id: EngineId,
    control: Arc<FakeControl>,
}

impl FakeAdapter {
    fn handle(&self) -> Box<dyn SessionHandle> {
        let index = self.control.created.fetch_add(1, Ordering::AcqRel) + 1;
        let (sink, stream) = bounded_event_stream(self.control.provider_capacity);
        self.control
            .sinks
            .lock()
            .expect("fake sink mutex")
            .push(sink.clone());
        if self.control.overflow_on_open.load(Ordering::Acquire) {
            sink.emit(message_event("discarded oldest provider event"))
                .expect("first fake provider event");
            sink.emit(message_event("retained latest provider event"))
                .expect("soft provider queue retains its latest event");
        }
        self.control.created_changed.notify_waiters();
        Box::new(FakeHandle {
            engine: self.id.clone(),
            current_session_id: Arc::new(Mutex::new(format!("pending-{index}"))),
            events: Some(stream),
            sink,
            control: Arc::clone(&self.control),
        })
    }
}

#[async_trait]
impl EngineAdapter for FakeAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }

    fn executable(&self) -> Option<PathBuf> {
        self.control.executable.get().cloned()
    }

    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        self.control.probe_calls.fetch_add(1, Ordering::AcqRel);
        let gate = self.control.probe_gate.lock().expect("probe gate").clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if self.control.probe_fails.load(Ordering::Acquire) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "fixture probe refused",
                false,
            ));
        }
        Ok(EngineInfo {
            auth: None,
            binary_path: "/fake/provider".into(),
            version: "fake-1".into(),
            login_identity: None,
            capabilities: EngineCapabilities {
                streaming: true,
                approvals: true,
                resume: true,
                fork: false,
                cloud_sessions: false,
                models: EngineModels::Models(Vec::new()),
                effort_levels: Vec::new(),
                permission_modes: vec!["default".into()],
                consultation_safety: None,
                steer: Some(true),
            },
            display_name: Some("Fake Provider".into()),
            model_catalog: Some(vec![ModelOption {
                value: "fake-model".into(),
                display_name: "Fake Model".into(),
                description: Some("Fixture model".into()),
                supports_effort: true,
                effort_levels: vec!["high".into()],
                is_default: Some(true),
                supports_fast_mode: Some(false),
            }]),
        })
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        Ok(EngineSessionListing {
            sessions: self.control.listed.lock().expect("fake listing").clone(),
            complete: true,
            unavailable: None,
            error: None,
        })
    }

    fn create_session(
        &self,
        options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        self.control
            .launched
            .lock()
            .expect("fake launch log")
            .push(options);
        Ok(self.handle())
    }

    fn resume_session(
        &self,
        _session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        Ok(self.handle())
    }

    async fn dispose(&self) -> Result<(), EngineError> {
        self.control.dispose_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

struct FakeHandle {
    engine: EngineId,
    current_session_id: Arc<Mutex<String>>,
    events: Option<BoundedEventStream<EngineEvent>>,
    sink: EventSink<EngineEvent>,
    control: Arc<FakeControl>,
}

#[async_trait]
impl SessionHandle for FakeHandle {
    fn engine(&self) -> &EngineId {
        &self.engine
    }

    fn session_id(&self) -> String {
        self.current_session_id
            .lock()
            .expect("fake session id")
            .clone()
    }

    async fn ready(&self) -> Result<ReadySession, EngineError> {
        self.control.wait_ready().await;
        self.current_session_id
            .lock()
            .expect("fake session id")
            .clone_from(&self.control.exact_session_id);
        Ok(ReadySession {
            session_id: self.control.exact_session_id.clone(),
        })
    }

    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        self.events.take().ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "fake stream already taken",
                false,
            )
        })
    }

    async fn send(&self, _text: String) -> DeliveryOutcome<()> {
        self.control.send_calls.fetch_add(1, Ordering::AcqRel);
        self.control
            .send_outcomes
            .lock()
            .expect("fake send queue")
            .pop_front()
            .unwrap_or(DeliveryOutcome::Confirmed(()))
    }

    async fn send_reporting_dispatch(
        &self,
        text: String,
        dispatched: tokio::sync::oneshot::Sender<()>,
    ) -> DeliveryOutcome<()> {
        if !self.control.park_sends.load(Ordering::Acquire) {
            let outcome = self.send(text).await;
            let _ = dispatched.send(());
            return outcome;
        }
        self.control.send_calls.fetch_add(1, Ordering::AcqRel);
        self.control.turn_open.store(true, Ordering::Release);
        self.control.frame("prompt");
        let _ = dispatched.send(());
        self.control.wait_turn_end().await;
        DeliveryOutcome::Confirmed(())
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        self.control.interrupt_calls.fetch_add(1, Ordering::AcqRel);
        self.control.frame("interrupt");
        self.control.end_turn();
        self.control
            .interrupt_outcomes
            .lock()
            .expect("fake interrupt queue")
            .pop_front()
            .unwrap_or(DeliveryOutcome::Confirmed(()))
    }

    async fn close(&self) -> Result<(), EngineError> {
        self.control.close_calls.fetch_add(1, Ordering::AcqRel);
        self.control.frame("close");
        self.control.end_turn();
        self.sink.close();
        Ok(())
    }

    async fn respond_to_approval(
        &self,
        _approval_id: String,
        _verdict: ApprovalVerdict,
        _response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        self.control.frame("approval");
        DeliveryOutcome::Confirmed(())
    }

    async fn set_model(&self, _model: Option<String>) -> DeliveryOutcome<()> {
        self.control.frame("set-model");
        DeliveryOutcome::Confirmed(())
    }

    async fn set_effort(&self, _effort: Option<String>) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }

    async fn set_fast_mode(&self, _enabled: bool) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }

    async fn set_permission_mode(&self, _mode: String) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
}

fn harness(
    provider_capacity: usize,
    history_limits: HistoryLimits,
    runtime_configuration: RuntimeConfiguration,
) -> Harness {
    harness_with_chat_home(
        provider_capacity,
        history_limits,
        runtime_configuration,
        None,
    )
}

fn harness_with_chat_home(
    provider_capacity: usize,
    history_limits: HistoryLimits,
    runtime_configuration: RuntimeConfiguration,
    chat_home: Option<PathBuf>,
) -> Harness {
    harness_with_clock(
        provider_capacity,
        history_limits,
        runtime_configuration,
        chat_home,
        None,
    )
}

/// Each reading is one second after the previous, so a paused-time test still
/// sees distinct, ordered event timestamps.
#[derive(Default)]
struct TickingClock(AtomicU64);

impl RuntimeClock for TickingClock {
    fn now(&self) -> String {
        let tick = self.0.fetch_add(1, Ordering::SeqCst);
        format!(
            "2026-09-23T{:02}:{:02}:{:02}.000Z",
            tick / 3_600 % 24,
            tick / 60 % 60,
            tick % 60
        )
    }
}

fn harness_with_clock(
    provider_capacity: usize,
    history_limits: HistoryLimits,
    runtime_configuration: RuntimeConfiguration,
    chat_home: Option<PathBuf>,
    clock: Option<Arc<dyn RuntimeClock>>,
) -> Harness {
    let root = private_tempdir();
    let sessions = SessionRegistry::open(root.path()).expect("session registry");
    let history =
        EngineHistoryStore::open(root.path(), history_limits).expect("engine history store");
    let services = RuntimeServices::new(sessions, history);
    let control = FakeControl::new(provider_capacity);
    let engine = EngineId::new("codex").expect("canonical fake engine id");
    let adapter = Arc::new(FakeAdapter {
        id: engine.clone(),
        control: Arc::clone(&control),
    });
    let mut builder = SessionRuntime::builder(services).configuration(runtime_configuration);
    if let Some(chat_home) = chat_home {
        builder = builder.chat_home(chat_home);
    }
    if let Some(clock) = clock {
        builder = builder.clock(clock);
    }
    builder
        .register_shared_adapter(adapter)
        .expect("fake adapter registration");
    let runtime = builder.build().expect("session runtime");
    Harness {
        runtime,
        control,
        engine,
        root,
    }
}

fn private_tempdir() -> TempDir {
    let root = tempfile::tempdir().expect("temporary runtime directory");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory mode");
    root
}

fn drift_registry_cache(root: &TempDir) {
    let path = root.path().join("registry.json");
    std::fs::write(&path, b"{\"sessions\":{}}")
        .expect("replace registry cache outside its snapshot");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("private replacement registry mode");
}

fn launch() -> LaunchOptions {
    LaunchOptions {
        cwd: "/tmp/interface-runtime-test".into(),
        prompt: "hello".into(),
        ..LaunchOptions::default()
    }
}

fn listed_summary(engine: &EngineId, session_id: &str, cwd: &str) -> SessionSummary {
    SessionSummary {
        engine: engine.clone(),
        session_id: session_id.into(),
        title: session_id.into(),
        cwd: cwd.into(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: "2026-08-27T00:00:00.000Z".into(),
        first_seen: "2026-08-27T00:00:00.000Z".into(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: Some(SessionOrigin::Store),
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

fn message_event(text: &str) -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: text.into(),
            agent_key: None,
        },
    }
}

async fn open_ready(harness: &Harness) -> interface_runtime::RuntimeSession {
    harness.control.release_ready();
    match harness
        .runtime
        .start_session(&harness.engine, launch())
        .await
    {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected open outcome: {outcome:?}"),
    }
}

#[tokio::test]
async fn global_shutdown_quiesces_closes_disposes_and_is_idempotent() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;

    harness.runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 1);
    assert_eq!(harness.control.dispose_calls.load(Ordering::Acquire), 1);
    let refused = harness
        .runtime
        .send(&session.key, "after shutdown".into())
        .await;
    assert!(matches!(
        refused.outcome,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    assert_eq!(harness.control.send_calls.load(Ordering::Acquire), 0);
    assert!(matches!(
        harness
            .runtime
            .start_session(&harness.engine, launch())
            .await,
        SessionOpenOutcome::DefinitelyUnsent(_)
    ));
    assert_eq!(harness.control.created.load(Ordering::Acquire), 1);

    harness
        .runtime
        .shutdown()
        .await
        .expect("idempotent shutdown");
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 1);
    assert_eq!(harness.control.dispose_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn orchestration_models_probe_once_then_use_the_provider_catalogue() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());

    let first = harness
        .runtime
        .orchestration_model_catalogue(&harness.engine)
        .await
        .expect("provider catalogue");
    let second = harness
        .runtime
        .orchestration_model_catalogue(&harness.engine)
        .await
        .expect("cached provider catalogue");

    assert_eq!(first, second);
    assert_eq!(first[0].value, "fake-model");
    assert_eq!(harness.control.probe_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn concurrent_health_and_catalogue_reads_share_one_provider_probe() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let release = Arc::new(Notify::new());
    *harness.control.probe_gate.lock().expect("probe gate") = Some(Arc::clone(&release));
    let mut readers = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let runtime = harness.runtime.clone();
        let engine = harness.engine.clone();
        readers.spawn(async move { runtime.probe_provider(&engine).await });
    }
    let catalogue = {
        let runtime = harness.runtime.clone();
        let engine = harness.engine.clone();
        tokio::spawn(async move { runtime.orchestration_model_catalogue(&engine).await })
    };
    // All calls get a chance to contend while the first actual probe is held.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(harness.control.probe_calls.load(Ordering::Acquire), 1);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(result) = readers.join_next().await {
            assert!(matches!(
                result.expect("probe task"),
                ProviderReadiness::Ready { .. }
            ));
        }
        assert!(catalogue.await.expect("catalogue task").is_some());
    })
    .await
    .expect("shared probe completed");
    assert_eq!(harness.control.probe_calls.load(Ordering::Acquire), 1);
}

#[tokio::test(start_paused = true)]
async fn probe_reuses_a_fresh_success_but_always_retries_after_a_failure() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let executable = harness.root.path().join("fake-provider");
    std::fs::write(&executable, "v1").expect("fake executable");
    harness
        .control
        .executable
        .set(executable.clone())
        .expect("executable is set once");
    let probe = || harness.runtime.probe_provider(&harness.engine);
    let calls = || harness.control.probe_calls.load(Ordering::Acquire);

    harness.control.fail_probe(true);
    for _ in 0..2 {
        assert!(matches!(
            probe().await,
            ProviderReadiness::Unavailable { .. }
        ));
    }
    assert_eq!(calls(), 2);

    harness.control.fail_probe(false);
    for _ in 0..2 {
        assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    }
    assert_eq!(calls(), 3);

    // A success stands for five minutes while the executable is unchanged.
    tokio::time::advance(Duration::from_secs(4 * 60 + 59)).await;
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert_eq!(calls(), 3);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert_eq!(calls(), 4);

    // A replaced executable is asked again at once, then kept again.
    std::fs::write(&executable, "v2, a different build").expect("replace executable");
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert_eq!(calls(), 5);

    // The explicit refresh never takes a kept probe, and its failure drops
    // the kept success: the next read asks again instead of falling back.
    harness.control.fail_probe(true);
    let readiness = harness.runtime.probe_all_providers().await;
    assert!(matches!(
        readiness.as_slice(),
        [ProviderReadiness::Unavailable { .. }]
    ));
    assert!(matches!(
        probe().await,
        ProviderReadiness::Unavailable { .. }
    ));
    assert_eq!(calls(), 7);
    harness.control.fail_probe(false);
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    let readiness = harness.runtime.probe_all_providers().await;
    assert!(matches!(
        readiness.as_slice(),
        [ProviderReadiness::Ready { .. }]
    ));
    assert_eq!(calls(), 9);
}

#[tokio::test(start_paused = true)]
async fn probe_without_an_executable_to_watch_is_reused_for_seconds_only() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let probe = || harness.runtime.probe_provider(&harness.engine);
    let calls = || harness.control.probe_calls.load(Ordering::Acquire);

    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert_eq!(calls(), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(probe().await, ProviderReadiness::Ready { .. }));
    assert_eq!(calls(), 2);
}

#[tokio::test]
async fn explicit_refreshes_share_only_a_probe_that_began_after_they_asked() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let calls = || harness.control.probe_calls.load(Ordering::Acquire);

    // Asked together, before either probe began: one probe answers both.
    let (first, second) = tokio::join!(
        harness.runtime.probe_all_providers(),
        harness.runtime.probe_all_providers()
    );
    assert!(matches!(
        first.as_slice(),
        [ProviderReadiness::Ready { .. }]
    ));
    assert!(matches!(
        second.as_slice(),
        [ProviderReadiness::Ready { .. }]
    ));
    assert_eq!(calls(), 1);

    // Asked while a probe was already running: it waits and probes again.
    let release = Arc::new(Notify::new());
    *harness.control.probe_gate.lock().expect("probe gate") = Some(Arc::clone(&release));
    let running = {
        let runtime = harness.runtime.clone();
        tokio::spawn(async move { runtime.probe_all_providers().await })
    };
    while calls() < 2 {
        tokio::task::yield_now().await;
    }
    let later = {
        let runtime = harness.runtime.clone();
        tokio::spawn(async move { runtime.probe_all_providers().await })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    release.notify_one();
    running.await.expect("running refresh");
    while calls() < 3 {
        tokio::task::yield_now().await;
    }
    release.notify_one();
    later.await.expect("later refresh");
    assert_eq!(calls(), 3);
}

#[tokio::test]
async fn registers_only_after_readiness_then_projects_before_publishing() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let runtime = harness.runtime.clone();
    let engine = harness.engine.clone();
    let opening = tokio::spawn(async move { runtime.start_session(&engine, launch()).await });
    harness.control.wait_created().await;

    assert!(
        harness
            .runtime
            .list_sessions()
            .await
            .expect("sessions")
            .is_empty()
    );
    harness.control.emit(message_event("arrived before ready"));
    harness.control.release_ready();
    let session = match opening.await.expect("opening task") {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected open outcome: {outcome:?}"),
    };

    let mut events = harness
        .runtime
        .take_session_events(&session.key)
        .await
        .expect("runtime events");
    let frame = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("runtime event timeout")
        .expect("runtime stream")
        .expect("runtime event");
    assert!(
        matches!(
            frame,
            RuntimeEventFrame::Event { ref value }
                if value.sequence == 1
                    && matches!(value.event.event, EngineEventKind::MessageDelta { ref text, .. } if text == "arrived before ready")
        ),
        "unexpected runtime frame: {frame:?}"
    );

    let summaries = harness.runtime.list_sessions().await.expect("sessions");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, "provider-session-1");
    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.events.len(), 1);
    assert_eq!(history.events[0].sequence, 1);
}

#[tokio::test]
async fn ordinary_create_and_listing_project_git_root_once_and_persist_it() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let repository = harness.root.path().join("repository");
    let nested = repository.join("nested").join("deeper");
    std::fs::create_dir_all(&nested).expect("nested repository directory");
    let initialized = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repository)
        .status()
        .expect("run git init");
    assert!(initialized.success(), "git init should succeed");
    let cwd = nested.to_string_lossy().into_owned();
    harness.control.set_listing(vec![listed_summary(
        &harness.engine,
        "stored-session",
        &cwd,
    )]);

    harness.control.release_ready();
    let mut options = launch();
    options.cwd.clone_from(&cwd);
    let opened = match harness
        .runtime
        .start_session(&harness.engine, options)
        .await
    {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected open outcome: {outcome:?}"),
    };
    harness
        .runtime
        .refresh_provider_sessions(&harness.engine)
        .await
        .expect("provider listing refresh");
    harness.runtime.shutdown().await.expect("runtime shutdown");

    let expected_root = std::fs::canonicalize(&repository)
        .expect("canonical repository")
        .to_string_lossy()
        .into_owned();
    let persisted = read_registry_file(harness.root.path(), "registry.json")
        .expect("restart-safe registry projection");
    for key in [opened.key.as_str(), "codex#stored-session"] {
        let summary = persisted
            .iter()
            .find(|summary| format!("{}#{}", summary.engine, summary.session_id) == key)
            .unwrap_or_else(|| panic!("persisted summary {key}"));
        assert_eq!(
            summary.project_root.as_deref(),
            Some(expected_root.as_str())
        );
    }
}

#[tokio::test]
async fn registry_cache_save_failure_does_not_reject_or_close_a_ready_provider() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    drift_registry_cache(&harness.root);

    let session = open_ready(&harness).await;

    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 0);
    assert!(
        harness
            .runtime
            .session_is_live(&session.key)
            .expect("live query")
    );
    assert_eq!(
        harness
            .runtime
            .services()
            .session_registry()
            .lock()
            .expect("session registry")
            .get(&session.key)
            .expect("in-memory registration survives cache failure")
            .session_id,
        session.session_id
    );
    let sent = harness
        .runtime
        .send(&session.key, "provider stays usable".into())
        .await;
    assert!(sent.outcome.is_confirmed());
    harness
        .runtime
        .shutdown()
        .await
        .expect("cache failure cannot fail runtime shutdown");
}

#[tokio::test]
async fn registry_cache_save_failure_does_not_quarantine_events_or_stream_close() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&session.key)
        .await
        .expect("runtime events");
    drift_registry_cache(&harness.root);

    harness
        .control
        .emit(message_event("cache failure is non-authoritative"));
    let frame = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event timeout")
        .expect("event stream")
        .expect("event frame");
    assert!(matches!(
        frame,
        RuntimeEventFrame::Event { ref value }
            if matches!(
                value.event.event,
                EngineEventKind::MessageDelta { ref text, .. }
                    if text == "cache failure is non-authoritative"
            )
    ));
    let sent = harness
        .runtime
        .send(&session.key, "still live".into())
        .await;
    assert!(sent.outcome.is_confirmed());
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 0);

    harness.control.close_current_stream();
    let terminal = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("terminal timeout")
        .expect("event stream")
        .expect("terminal frame");
    assert!(matches!(
        terminal,
        RuntimeEventFrame::Terminal {
            reason: RuntimeEventTermination::Closed,
            ..
        }
    ));
}

#[tokio::test]
async fn unknown_send_outcome_is_durable_and_quarantines_replay() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    harness.control.push_send(DeliveryOutcome::OutcomeUnknown(
        EngineError::outcome_unknown(
            EngineErrorKind::Transport,
            "connection ended after dispatch",
            true,
        ),
    ));
    let session = open_ready(&harness).await;

    let first = harness.runtime.send(&session.key, "effect".into()).await;
    assert!(matches!(first.outcome, DeliveryOutcome::OutcomeUnknown(_)));
    assert!(matches!(
        first.audit,
        RuntimeDeliveryAudit::Persisted { .. }
    ));
    assert!(!first.replay_is_safe());

    let second = harness
        .runtime
        .send(&session.key, "unsafe retry".into())
        .await;
    assert!(matches!(
        second.outcome,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
    assert_eq!(second.audit, RuntimeDeliveryAudit::NotApplicable);
    assert_eq!(harness.control.send_calls.load(Ordering::Acquire), 1);

    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.delivery_state, RuntimeDeliveryState::OutcomeUnknown);
    assert_eq!(history.effects.len(), 1);
    assert_eq!(
        history.effects[0].disposition,
        EffectAuditDisposition::OutcomeUnknown
    );

    let detached = harness.runtime.close_session(&session.key).await;
    assert!(detached.outcome.is_confirmed());
    let resumed = match harness
        .runtime
        .resume_session(
            &harness.engine,
            session.session_id,
            "/tmp/interface-runtime-test".into(),
            ResumeOptions::default(),
        )
        .await
    {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected resume outcome: {outcome:?}"),
    };
    let reconciled = harness
        .runtime
        .send(&resumed.key, "after reconcile".into())
        .await;
    assert!(reconciled.outcome.is_confirmed());
    let history = harness
        .runtime
        .history_session(&resumed.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.delivery_state, RuntimeDeliveryState::Open);
}

#[tokio::test]
async fn cancellation_preserves_provider_rejection_without_quarantining_session() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    harness
        .control
        .push_interrupt(DeliveryOutcome::DefinitelyRejected(
            EngineError::definitely_rejected(
                EngineErrorKind::Provider,
                "nothing is running",
                false,
            ),
        ));
    let session = open_ready(&harness).await;

    let cancelled = harness.runtime.cancel_session(&session.key).await;
    assert!(matches!(
        cancelled.outcome,
        DeliveryOutcome::DefinitelyRejected(_)
    ));
    assert_eq!(harness.control.interrupt_calls.load(Ordering::Acquire), 1);
    let subsequent = harness
        .runtime
        .send(&session.key, "still safe".into())
        .await;
    assert!(subsequent.outcome.is_confirmed());
    assert_eq!(harness.control.send_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn full_provider_queue_retains_its_latest_event_without_terminating() {
    let harness = harness(
        1,
        HistoryLimits::default(),
        RuntimeConfiguration {
            pre_ready_event_capacity: 2,
            subscriber_event_capacity: 4,
        },
    );
    harness
        .control
        .overflow_on_open
        .store(true, Ordering::Release);
    let session = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&session.key)
        .await
        .expect("runtime events");

    let first = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("first event timeout")
        .expect("runtime stream")
        .expect("first event");
    assert!(matches!(
        first,
        RuntimeEventFrame::Event { ref value }
            if matches!(
                value.event.event,
                EngineEventKind::MessageDelta { ref text, .. }
                    if text == "retained latest provider event"
            )
    ));

    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.stream_state, RuntimeStreamState::Active);
    assert_ne!(history.summary.status, SessionStatus::Failed);
    let sent = harness
        .runtime
        .send(&session.key, "after queue pressure".into())
        .await;
    assert!(sent.outcome.is_confirmed());
}

#[tokio::test]
async fn history_retention_reports_the_exact_omitted_sequence() {
    let harness = harness(
        8,
        HistoryLimits {
            events_per_session: 2,
            effects_per_session: 2,
            ..HistoryLimits::default()
        },
        RuntimeConfiguration::default(),
    );
    let session = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&session.key)
        .await
        .expect("runtime events");
    for text in ["one", "two", "three"] {
        harness.control.emit(message_event(text));
        let _ = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event timeout")
            .expect("runtime stream")
            .expect("runtime event");
    }
    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.omitted_events_through, Some(1));
    assert_eq!(
        history
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[tokio::test]
async fn bridge_commands_are_serde_typed_and_transport_neutral() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let command = RuntimeBridgeCommand::StartSession {
        engine: harness.engine.clone(),
        options: launch(),
    };
    let encoded = serde_json::to_vec(&command).expect("serialize bridge command");
    let decoded: RuntimeBridgeCommand =
        serde_json::from_slice(&encoded).expect("deserialize bridge command");
    assert_eq!(decoded, command);

    let response = harness
        .runtime
        .handle_bridge_command(RuntimeBridgeCommand::RegisteredEngines)
        .await;
    assert!(matches!(
        response,
        RuntimeBridgeResponse::RegisteredEngines(ref engines) if engines == &[harness.engine]
    ));
    serde_json::to_vec(&response).expect("serialize bridge response");
}

#[tokio::test]
async fn close_then_resume_uses_the_exact_provider_session_id() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let opened = open_ready(&harness).await;
    let closed = harness.runtime.close_session(&opened.key).await;
    assert!(closed.outcome.is_confirmed());

    let resumed = match harness
        .runtime
        .resume_session(
            &harness.engine,
            opened.session_id.clone(),
            "/tmp/interface-runtime-test".into(),
            ResumeOptions::default(),
        )
        .await
    {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected resume outcome: {outcome:?}"),
    };
    assert_eq!(resumed.session_id, opened.session_id);
    assert_eq!(resumed.key, opened.key);
    let history = harness
        .runtime
        .history_session(&resumed.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.attachment_count, 2);
}

#[tokio::test]
async fn naturally_closed_session_can_resume_and_send_in_the_same_runtime() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let opened = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&opened.key)
        .await
        .expect("runtime events");

    harness.control.close_current_stream();
    let terminal = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("terminal event timeout")
        .expect("runtime stream")
        .expect("terminal frame");
    assert!(matches!(
        terminal,
        RuntimeEventFrame::Terminal {
            reason: RuntimeEventTermination::Closed,
            ..
        }
    ));
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 1);
    assert!(
        !harness
            .runtime
            .session_is_live(&opened.key)
            .expect("live query after terminal")
    );

    let resumed = match harness
        .runtime
        .resume_session(
            &harness.engine,
            opened.session_id,
            "/tmp/interface-runtime-test".into(),
            ResumeOptions::default(),
        )
        .await
    {
        SessionOpenOutcome::Ready(session) => session,
        outcome => panic!("unexpected resume outcome: {outcome:?}"),
    };
    let sent = harness
        .runtime
        .send(&resumed.key, "cold follow-up".into())
        .await;
    assert!(sent.outcome.is_confirmed());
    assert_eq!(harness.control.send_calls.load(Ordering::Acquire), 1);
    let history = harness
        .runtime
        .history_session(&resumed.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.attachment_count, 2);
}

#[tokio::test]
async fn definitive_provider_identity_violation_closes_and_evicts_before_terminal() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let opened = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&opened.key)
        .await
        .expect("runtime events");

    harness.control.emit(EngineEvent {
        at: None,
        event: EngineEventKind::SessionMeta {
            session_id: "different-provider-session".into(),
            model: "fake-model".into(),
            cwd: "/tmp/interface-runtime-test".into(),
            title: None,
        },
    });
    let terminal = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("terminal event timeout")
        .expect("runtime stream")
        .expect("terminal frame");
    assert!(matches!(
        terminal,
        RuntimeEventFrame::Terminal {
            reason: RuntimeEventTermination::PersistenceFailed,
            ..
        }
    ));
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 1);
    assert!(
        !harness
            .runtime
            .session_is_live(&opened.key)
            .expect("live query after quarantine")
    );
}

#[test]
fn native_registration_constructs_all_three_adapter_families_without_probing() {
    let root = private_tempdir();
    let sessions = SessionRegistry::open(root.path()).expect("session registry");
    let history = EngineHistoryStore::open(root.path(), HistoryLimits::default())
        .expect("engine history store");
    let services = RuntimeServices::new(sessions, history);
    let qwen = AcpAdapterConfig::builtin(
        BuiltinAcpProvider::Qwen,
        PathBuf::from("/tmp/fake-qwen-acp"),
    )
    .expect("validated fake ACP configuration");
    let mut builder = SessionRuntime::builder(services);
    builder
        .register_native_adapters(NativeAdapterConfiguration {
            codex: Some(CodexAdapterConfig::new("/tmp/fake-codex")),
            claude_executable: Some(PathBuf::from("/tmp/fake-claude")),
            acp: vec![qwen],
        })
        .expect("native adapter registration");
    let runtime = builder.build().expect("native session runtime");
    assert_eq!(
        runtime
            .registered_engines()
            .into_iter()
            .map(|engine| engine.to_string())
            .collect::<Vec<_>>(),
        vec!["acp:qwen", "claude", "codex"]
    );
}

#[test]
fn services_reuse_the_canonical_orchestration_store() {
    let root = private_tempdir();
    let sessions = SessionRegistry::open(root.path()).expect("session registry");
    let history = EngineHistoryStore::open(root.path(), HistoryLimits::default())
        .expect("engine history store");
    let orchestration = OrchestrationStore::open(
        root.path(),
        "runtime-host",
        "runtime-epoch",
        "2026-08-27T12:00:00.000Z",
    )
    .expect("orchestration store");
    let services = RuntimeServices::new(sessions, history).with_orchestration(orchestration);
    assert!(services.orchestration().is_some());
}

fn chat_unavailable() -> String {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/session-kind-contract.json"
    ))
    .expect("session-kind contract fixture");
    fixture["chatUnavailable"]
        .as_str()
        .expect("chatUnavailable")
        .to_owned()
}

#[tokio::test]
async fn start_summary_carries_launch_kind() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    harness.control.release_ready();
    let mut options = launch();
    options.kind = Some(SessionKind::Agent);
    match harness
        .runtime
        .start_session(&harness.engine, options)
        .await
    {
        SessionOpenOutcome::Ready(_) => {}
        outcome => panic!("unexpected open outcome: {outcome:?}"),
    }
    let summaries = harness.runtime.list_sessions().await.expect("sessions");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].kind, Some(SessionKind::Agent));
}

#[tokio::test]
async fn chat_titles_use_the_prompt_before_provider_history_is_available() {
    for (prompt, name, expected) in [
        (
            "  Explain this idea  ".to_owned(),
            None,
            Some("Explain this idea".to_owned()),
        ),
        ("界".repeat(100), None, Some("界".repeat(80))),
        (
            "An internal preamble".to_owned(),
            Some("My task".to_owned()),
            Some("My task".to_owned()),
        ),
        ("  ".to_owned(), None, None),
    ] {
        let chat_home = private_tempdir();
        let harness = harness_with_chat_home(
            8,
            HistoryLimits::default(),
            RuntimeConfiguration::default(),
            Some(chat_home.path().to_owned()),
        );
        harness.control.release_ready();
        let options = LaunchOptions {
            prompt,
            name,
            kind: Some(SessionKind::Chat),
            ..launch()
        };
        let SessionOpenOutcome::Ready(opened) = harness
            .runtime
            .start_session(&harness.engine, options)
            .await
        else {
            panic!("session failed to open");
        };
        let summaries = harness.runtime.list_sessions().await.expect("sessions");
        assert_eq!(summaries[0].title, expected.unwrap_or(opened.session_id));
        harness.runtime.shutdown().await.expect("shutdown");
    }
}

/// 6 Oct 2026: a project thread started from the Threads card (kind Code,
/// sent as an omitted kind) was titled with its session id until the next
/// provider listing.
#[tokio::test]
async fn code_launch_titles_use_the_prompt_at_once_and_agent_launches_do_not() {
    for (kind, seeded) in [
        (None, true),
        (Some(SessionKind::Code), true),
        (Some(SessionKind::Agent), false),
    ] {
        let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
        harness.control.release_ready();
        let options = LaunchOptions {
            prompt: "  Fix the flaky login test  ".into(),
            kind,
            ..launch()
        };
        let SessionOpenOutcome::Ready(opened) = harness
            .runtime
            .start_session(&harness.engine, options)
            .await
        else {
            panic!("session failed to open");
        };
        // Read the registry itself: no listing has run.
        let registered = harness
            .runtime
            .services()
            .session_registry()
            .lock()
            .expect("session registry")
            .get(&opened.key)
            .expect("registered session")
            .clone();
        let expected = if seeded {
            "Fix the flaky login test".to_owned()
        } else {
            opened.session_id.clone()
        };
        assert_eq!(registered.title, expected, "kind {kind:?}");
        harness.runtime.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn prompt_seeded_titles_are_single_line_and_bounded_without_changing_the_prompt() {
    for kind in [None, Some(SessionKind::Code), Some(SessionKind::Chat)] {
        for (prompt, expected) in [
            (
                "  Review\tthe changes\n\nExpected outputs:\n- review.md\r\nRequired checks:\n- cargo test\u{0007} ".to_owned(),
                Some("Review the changes Expected outputs: - review.md Required checks: - cargo test".to_owned()),
            ),
            (format!("\n{}\n", "界".repeat(100)), Some("界".repeat(80))),
            ("\t\n\u{0007}\r".to_owned(), None),
        ] {
            let chat_home = private_tempdir();
            let harness = harness_with_chat_home(
                8,
                HistoryLimits::default(),
                RuntimeConfiguration::default(),
                Some(chat_home.path().to_owned()),
            );
            harness.control.release_ready();
            let options = LaunchOptions {
                prompt: prompt.clone(),
                kind,
                ..launch()
            };
            let SessionOpenOutcome::Ready(opened) = harness
                .runtime
                .start_session(&harness.engine, options)
                .await
            else {
                panic!("session failed to open");
            };
            let title = harness
                .runtime
                .services()
                .session_registry()
                .lock()
                .unwrap()
                .get(&opened.key)
                .unwrap()
                .title
                .clone();
            assert_eq!(title, expected.unwrap_or(opened.session_id));
            assert!(!title.chars().any(char::is_control));
            assert!(title.chars().count() <= 80);
            assert_eq!(harness.control.launched.lock().unwrap()[0].prompt, prompt);
            harness.runtime.shutdown().await.expect("shutdown");
        }
    }
}

#[tokio::test]
async fn chat_launch_is_pinned_to_chat_home_and_never_creates_a_worktree() {
    let chat_home = private_tempdir();
    let chat_home_path = chat_home.path().to_path_buf();
    let expected_cwd = chat_home_path.to_string_lossy().into_owned();
    let harness = harness_with_chat_home(
        8,
        HistoryLimits::default(),
        RuntimeConfiguration::default(),
        Some(chat_home_path),
    );
    harness.control.release_ready();
    let mut options = launch();
    options.kind = Some(SessionKind::Chat);
    options.worktree = Some(true);
    options.cwd = "/tmp/a-real-project".into();
    match harness
        .runtime
        .start_session(&harness.engine, options)
        .await
    {
        SessionOpenOutcome::Ready(_) => {}
        outcome => panic!("unexpected open outcome: {outcome:?}"),
    }
    let launched = harness
        .control
        .launched
        .lock()
        .expect("fake launch log")
        .clone();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].cwd, expected_cwd);
    assert_eq!(launched[0].worktree, None);
    assert_eq!(launched[0].kind, Some(SessionKind::Chat));
    let summaries = harness.runtime.list_sessions().await.expect("sessions");
    assert_eq!(summaries[0].kind, Some(SessionKind::Chat));
    assert_eq!(summaries[0].cwd, expected_cwd);
    assert_eq!(summaries[0].title, "hello");
}

#[tokio::test]
async fn chat_launch_without_chat_home_is_definitely_unsent() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let mut options = launch();
    options.kind = Some(SessionKind::Chat);
    options.worktree = Some(true);
    let error = match harness
        .runtime
        .start_session(&harness.engine, options)
        .await
    {
        SessionOpenOutcome::DefinitelyUnsent(error) => error,
        outcome => panic!("expected definitely-unsent chat refusal, got {outcome:?}"),
    };
    assert_eq!(error.kind, RuntimeErrorKind::InvalidInput);
    assert_eq!(error.message, chat_unavailable());
    assert!(!error.retryable);
    assert!(error.replay_is_safe());
    assert_eq!(harness.control.created.load(Ordering::Acquire), 0);
    assert!(
        harness
            .control
            .launched
            .lock()
            .expect("fake launch log")
            .is_empty()
    );
}

#[test]
fn pin_chat_launch_is_a_noop_for_code_and_agent() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    for kind in [None, Some(SessionKind::Code), Some(SessionKind::Agent)] {
        let mut options = launch();
        options.kind = kind;
        options.worktree = Some(true);
        options.cwd = "/tmp/project".into();
        harness
            .runtime
            .pin_chat_launch(&mut options)
            .expect("non-chat pin");
        assert_eq!(options.cwd, "/tmp/project");
        assert_eq!(options.worktree, Some(true));
    }
}

#[tokio::test]
async fn probe_all_providers_returns_readiness_in_engine_order() {
    let root = private_tempdir();
    let sessions = SessionRegistry::open(root.path()).expect("session registry");
    let history = EngineHistoryStore::open(root.path(), HistoryLimits::default())
        .expect("engine history store");
    let services = RuntimeServices::new(sessions, history);
    let codex = FakeControl::new(8);
    let claude = FakeControl::new(8);
    let mut builder = SessionRuntime::builder(services);
    builder
        .register_adapter(FakeAdapter {
            id: EngineId::new("codex").expect("engine"),
            control: Arc::clone(&codex),
        })
        .expect("codex adapter");
    builder
        .register_adapter(FakeAdapter {
            id: EngineId::new("claude").expect("engine"),
            control: Arc::clone(&claude),
        })
        .expect("claude adapter");
    let runtime = builder.build().expect("session runtime");

    let readiness = runtime.probe_all_providers().await;
    assert_eq!(
        readiness
            .iter()
            .map(|item| match item {
                ProviderReadiness::Ready { engine, .. }
                | ProviderReadiness::Unavailable { engine, .. } => engine.to_string(),
            })
            .collect::<Vec<_>>(),
        vec!["claude".to_owned(), "codex".to_owned()]
    );
    assert!(
        readiness
            .iter()
            .all(|item| matches!(item, ProviderReadiness::Ready { .. }))
    );
    assert_eq!(codex.probe_calls.load(Ordering::Acquire), 1);
    assert_eq!(claude.probe_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn persist_event_projects_meta_config_and_closed_without_an_extra_event_clone() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    // An Agent launch starts untitled (its session id), so the provider's
    // meta title is what names it; a Code launch keeps its prompt seed.
    harness.control.release_ready();
    let SessionOpenOutcome::Ready(opened) = harness
        .runtime
        .start_session(
            &harness.engine,
            LaunchOptions {
                kind: Some(SessionKind::Agent),
                ..launch()
            },
        )
        .await
    else {
        panic!("session failed to open");
    };
    let mut events = harness
        .runtime
        .take_session_events(&opened.key)
        .await
        .expect("runtime events");

    harness.control.emit(EngineEvent {
        at: Some("2026-08-27T12:00:00.000Z".into()),
        event: EngineEventKind::SessionMeta {
            session_id: opened.session_id.clone(),
            model: "updated-model".into(),
            cwd: "/tmp/new-cwd".into(),
            title: Some("Updated title".into()),
        },
    });
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("meta timeout")
        .expect("runtime stream")
        .expect("meta frame");

    harness.control.emit(EngineEvent {
        at: Some("2026-08-27T12:00:01.000Z".into()),
        event: EngineEventKind::ConfigChanged {
            model: Some("config-model".into()),
            effort: Some(Some("high".into())),
        },
    });
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("config timeout")
        .expect("runtime stream")
        .expect("config frame");

    let after_config = harness
        .runtime
        .services()
        .session_registry()
        .lock()
        .expect("session registry")
        .get(&opened.key)
        .expect("registered session")
        .clone();
    assert_eq!(after_config.model.as_deref(), Some("config-model"));
    assert_eq!(after_config.effort, Some(Some("high".into())));
    assert_eq!(after_config.cwd, "/tmp/new-cwd");
    assert_eq!(after_config.title, "Updated title");
    assert!(after_config.project_root.is_none());

    harness.control.emit(EngineEvent {
        at: Some("2026-08-27T12:00:02.000Z".into()),
        event: EngineEventKind::SessionClosed,
    });
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("closed timeout")
        .expect("runtime stream")
        .expect("closed frame");

    let after_close = harness
        .runtime
        .services()
        .session_registry()
        .lock()
        .expect("session registry")
        .get(&opened.key)
        .expect("registered session")
        .clone();
    assert_eq!(after_close.status, SessionStatus::Idle);
}

// Paused time: the 1.6 s waits are virtual, so the cadence cannot drift
// with machine load (spawn_blocking saves hold the clock while they run).
#[tokio::test(start_paused = true)]
async fn activity_only_events_defer_the_registry_cache_until_shutdown() {
    async fn next_received_at(events: &mut BoundedEventStream<RuntimeEventFrame>) -> String {
        match tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("runtime event timeout")
            .expect("runtime stream")
            .expect("runtime event")
        {
            RuntimeEventFrame::Event { value } => value.received_at,
            frame @ RuntimeEventFrame::Terminal { .. } => {
                panic!("unexpected runtime frame: {frame:?}")
            }
        }
    }
    fn persisted(harness: &Harness, key: &str) -> SessionSummary {
        read_registry_file(harness.root.path(), "registry.json")
            .expect("registry cache")
            .into_iter()
            .find(|summary| format!("{}#{}", summary.engine, summary.session_id) == key)
            .expect("persisted session")
    }

    let harness = harness_with_clock(
        8,
        HistoryLimits::default(),
        RuntimeConfiguration::default(),
        None,
        Some(Arc::new(TickingClock::default())),
    );
    let opened = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&opened.key)
        .await
        .expect("runtime events");

    // A status change still reaches the cache on the 1 s writer.
    harness.control.emit(EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Waiting,
        },
    });
    let waiting_at = next_received_at(&mut events).await;
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    let saved = persisted(&harness, &opened.key);
    assert_eq!(saved.status, SessionStatus::Waiting);
    assert_eq!(saved.last_activity, waiting_at);

    // A delta while Waiting moves only last_activity: no 1 s rewrite.
    harness.control.emit(message_event("still waiting"));
    let activity_at = next_received_at(&mut events).await;
    assert_ne!(activity_at, waiting_at);
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    assert_eq!(persisted(&harness, &opened.key).last_activity, waiting_at);

    // Shutdown persists it (the terminal projection may stamp a later time).
    harness.runtime.shutdown().await.expect("runtime shutdown");
    assert!(persisted(&harness, &opened.key).last_activity >= activity_at);
}

/// The host's quit-time worktree release can redirect a session home after
/// provider shutdown already flushed the cache; its final flush writes it.
#[tokio::test]
async fn a_registry_change_after_shutdown_reaches_the_cache_when_flushed() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let opened = open_ready(&harness).await;
    harness.runtime.shutdown().await.expect("runtime shutdown");

    let services = harness.runtime.services();
    {
        let registry = services.session_registry();
        let mut registry = registry.lock().expect("session registry");
        let mut summary = registry.get(&opened.key).cloned().expect("summary");
        summary.cwd = "/restored/home".into();
        let revision = registry.revision();
        registry
            .upsert_all(revision, &harness.engine, [summary])
            .expect("registry write");
    }
    services.schedule_registry_cache_save();
    services.flush_registry_cache().await;

    let persisted = read_registry_file(harness.root.path(), "registry.json")
        .expect("registry cache")
        .into_iter()
        .find(|summary| format!("{}#{}", summary.engine, summary.session_id) == opened.key)
        .expect("persisted session");
    assert_eq!(persisted.cwd, "/restored/home");
}

/// How long the registry cache writer holds the session-registry lock for one
/// save of a realistic cache (about 1,000 rows, about 800 KB on disk).
#[test]
#[ignore = "manual performance measurement"]
fn measure_registry_save_lock_hold() {
    let root = private_tempdir();
    let mut registry = SessionRegistry::open(root.path()).expect("session registry");
    let engine = EngineId::new("claude").expect("engine");
    let rows = (0..1_000)
        .map(|index| {
            let mut row = listed_summary(
                &engine,
                &format!("{index:08}-1111-4111-8111-111111111111"),
                &format!(
                    "/Users/someone/Desktop/Projects/project-{}/src/area",
                    index % 40
                ),
            );
            row.title = format!(
                "Session {index}: {}",
                "investigate the failing build ".repeat(12)
            );
            row.model = Some("claude-opus-4-1".into());
            row.effort = Some(Some("high".into()));
            row.project_root = Some(format!(
                "/Users/someone/Desktop/Projects/project-{}",
                index % 40
            ));
            row.title_from_summary = Some(true);
            row.size = Some(1_234_567);
            row
        })
        .collect::<Vec<_>>();
    registry
        .upsert_all(registry.revision(), &engine, rows)
        .expect("rows");
    let registry = Mutex::new(registry);
    let key = format!("{engine}#{:08}-1111-4111-8111-111111111111", 0);
    // An unchanged revision (or unchanged bytes) is not written again, so every
    // measured save follows a real change.
    let change = |registry: &mut SessionRegistry, label: String| {
        let revision = registry.revision();
        registry
            .set_title(revision, &key, label)
            .expect("change before save");
    };
    let mut whole = Vec::new();
    let mut held = Vec::new();
    for round in 0..30 {
        change(
            &mut registry.lock().expect("registry lock"),
            format!("whole {round}"),
        );
        // The old shape: the whole save under the lock.
        let started = std::time::Instant::now();
        let mut guard = registry.lock().expect("registry lock");
        let revision = guard.revision();
        guard.save(revision).expect("save");
        drop(guard);
        whole.push(started.elapsed());
        change(
            &mut registry.lock().expect("registry lock"),
            format!("held {round}"),
        );
        // The writer's shape: only the copy under the lock.
        let started = std::time::Instant::now();
        let guard = registry.lock().expect("registry lock");
        let save = guard.prepare_save(guard.revision()).expect("prepare");
        drop(guard);
        held.push(started.elapsed());
        save.write().expect("write");
    }
    let bytes = std::fs::metadata(root.path().join("registry.json"))
        .expect("registry file")
        .len();
    whole.sort();
    held.sort();
    eprintln!(
        "{bytes}-byte registry, 30 saves: whole save under the lock min {:?} median {:?} max {:?}; copy-only hold min {:?} median {:?} max {:?}",
        whole[0], whole[15], whole[29], held[0], held[15], held[29]
    );
}

/// Starts a send that parks like ACP's `session/prompt` and waits until its
/// frame is out.
async fn park_a_send(
    harness: &Harness,
    key: &str,
) -> tokio::task::JoinHandle<interface_runtime::RuntimeDelivery> {
    harness.control.park_sends.store(true, Ordering::Release);
    let runtime = harness.runtime.clone();
    let key = key.to_owned();
    let send = tokio::spawn(async move { runtime.send(&key, "a long turn".into()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.control.frames().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the prompt frame went out");
    send
}

// Real threads: the bypass must hold under parallel scheduling too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approval_and_cancel_reach_a_send_parked_on_its_turn() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;
    let send = park_a_send(&harness, &session.key).await;

    // An approval answers the parked turn instead of waiting for it to end.
    let approval = tokio::time::timeout(
        Duration::from_secs(2),
        harness.runtime.respond_to_approval(
            &session.key,
            "approval-1".into(),
            ApprovalVerdict::Allow,
            ApprovalResponse::default(),
        ),
    )
    .await
    .expect("the approval did not wait for the turn");
    assert!(approval.outcome.is_confirmed());
    assert!(!send.is_finished(), "the turn is still running");

    // Turn-shaping effects still wait for the turn.
    let set_model = {
        let runtime = harness.runtime.clone();
        let key = session.key.clone();
        tokio::spawn(async move { runtime.set_model(&key, Some("fake-model".into())).await })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        !set_model.is_finished(),
        "a setting change waits for the turn"
    );

    // Stop reaches the provider and ends the turn; the send keeps its outcome.
    let cancel = tokio::time::timeout(
        Duration::from_secs(2),
        harness.runtime.cancel_session(&session.key),
    )
    .await
    .expect("the cancel did not wait for the turn");
    assert!(cancel.outcome.is_confirmed());
    let settled = tokio::time::timeout(Duration::from_secs(2), send)
        .await
        .expect("the send settled after the cancel")
        .expect("send task");
    assert!(settled.outcome.is_confirmed());
    assert!(matches!(
        settled.audit,
        RuntimeDeliveryAudit::Persisted { .. }
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), set_model)
            .await
            .expect("the setting change ran after the turn")
            .expect("set model task")
            .outcome
            .is_confirmed()
    );
    assert_eq!(
        harness.control.frames(),
        ["prompt", "approval", "interrupt", "set-model"]
    );
    assert_eq!(harness.control.send_calls.load(Ordering::Acquire), 1);
    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(
        history.effects.len(),
        4,
        "every effect keeps its own audit row"
    );
}

// Real threads: the bypass must hold under parallel scheduling too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_cancels_a_parked_send_first_then_closes() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;
    let send = park_a_send(&harness, &session.key).await;

    let closed = tokio::time::timeout(
        Duration::from_secs(2),
        harness.runtime.close_session(&session.key),
    )
    .await
    .expect("the close did not wait for the whole turn");
    assert!(closed.outcome.is_confirmed());
    let settled = tokio::time::timeout(Duration::from_secs(2), send)
        .await
        .expect("the send settled")
        .expect("send task");
    assert!(settled.outcome.is_confirmed());
    assert_eq!(harness.control.frames(), ["prompt", "interrupt", "close"]);
    assert!(
        !harness
            .runtime
            .session_is_live(&session.key)
            .expect("live map")
    );
}

#[tokio::test]
async fn shutdown_cancels_a_parked_send_before_closing() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;
    let send = park_a_send(&harness, &session.key).await;

    tokio::time::timeout(Duration::from_secs(5), harness.runtime.shutdown())
        .await
        .expect("shutdown did not wait for the whole turn")
        .expect("runtime shutdown");
    assert!(
        tokio::time::timeout(Duration::from_secs(2), send)
            .await
            .expect("the send settled")
            .expect("send task")
            .outcome
            .is_confirmed()
    );
    assert_eq!(harness.control.frames(), ["prompt", "interrupt", "close"]);
    assert_eq!(harness.control.dispose_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_cancel_with_no_parked_send_still_waits_its_turn() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;

    // Without a parked send, effects run one at a time in call order.
    let cancel = harness.runtime.cancel_session(&session.key).await;
    let close = harness.runtime.close_session(&session.key).await;

    assert!(cancel.outcome.is_confirmed());
    assert!(close.outcome.is_confirmed());
    assert_eq!(harness.control.frames(), ["interrupt", "close"]);
}

fn status_event(status: SessionStatus) -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged { status },
    }
}

#[tokio::test]
async fn an_overflowing_subscriber_fails_the_session_closed() {
    let harness = harness(
        8,
        HistoryLimits::default(),
        RuntimeConfiguration {
            subscriber_event_capacity: 2,
            ..RuntimeConfiguration::default()
        },
    );
    let session = open_ready(&harness).await;
    let mut events = harness
        .runtime
        .take_session_events(&session.key)
        .await
        .expect("runtime events");

    // Control events are never dropped: the third one cannot fit.
    for status in [
        SessionStatus::Working,
        SessionStatus::Idle,
        SessionStatus::Working,
    ] {
        harness.control.emit(status_event(status));
    }

    tokio::time::timeout(Duration::from_secs(2), async {
        while harness
            .runtime
            .session_is_live(&session.key)
            .expect("live map")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the overflowed session left the live map");
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 1);
    assert!(matches!(
        events.recv().await,
        Err(interface_engine::EventStreamError::Overflow { capacity: 2 })
    ));
    let history = harness
        .runtime
        .history_session(&session.key)
        .await
        .expect("history query")
        .expect("history session");
    assert_eq!(history.stream_state, RuntimeStreamState::Overflow);
    assert!(
        history
            .terminal_message
            .as_deref()
            .is_some_and(|message| message.contains("subscriber exceeded its 2-event capacity")),
        "{:?}",
        history.terminal_message
    );
    // Later effects are refused as never sent.
    let refused = harness.runtime.send(&session.key, "after".into()).await;
    assert!(matches!(
        refused.outcome,
        DeliveryOutcome::DefinitelyUnsent(_)
    ));
}

#[tokio::test]
async fn a_dropped_subscriber_is_not_an_overflow() {
    let harness = harness(
        8,
        HistoryLimits::default(),
        RuntimeConfiguration {
            subscriber_event_capacity: 2,
            ..RuntimeConfiguration::default()
        },
    );
    let session = open_ready(&harness).await;
    drop(
        harness
            .runtime
            .take_session_events(&session.key)
            .await
            .expect("runtime events"),
    );

    for status in [
        SessionStatus::Working,
        SessionStatus::Idle,
        SessionStatus::Working,
    ] {
        harness.control.emit(status_event(status));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let history = harness
                .runtime
                .history_session(&session.key)
                .await
                .expect("history query")
                .expect("history session");
            if history.events.len() == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the pump persisted every event");

    assert!(
        harness
            .runtime
            .session_is_live(&session.key)
            .expect("live map")
    );
    assert_eq!(harness.control.close_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_dropped_parked_send_leaves_nothing_marked_parked() {
    let harness = harness(8, HistoryLimits::default(), RuntimeConfiguration::default());
    let session = open_ready(&harness).await;
    let send = park_a_send(&harness, &session.key).await;
    send.abort();
    assert!(send.await.is_err_and(|error| error.is_cancelled()));

    // No send is parked any more: the close takes its turn and cancels nothing.
    let closed = tokio::time::timeout(
        Duration::from_secs(2),
        harness.runtime.close_session(&session.key),
    )
    .await
    .expect("the close was not held by the dropped send");
    assert!(closed.outcome.is_confirmed());
    assert_eq!(harness.control.frames(), ["prompt", "close"]);
}
