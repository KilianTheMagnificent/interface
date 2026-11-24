use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
    time::Duration,
};

use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, ModelOption, ProviderCommand, ProviderCommandCatalogue,
        ProviderCommandScope, SandboxMode, SessionStatus, SessionSummary,
    },
    orchestration::OrchestrationEffectState,
};
use interface_engine::BoundedEventStream;
use interface_runtime::{RuntimeEventFrame, RuntimeEventTermination};
use serde_json::json;

use super::adversarial::{adversarial_review_command, message_processed};
use super::observer::ObservedCompletion;
use super::runtime_port::{
    engine_error_code, orchestration_launch_access, orchestration_setting_sources,
    runtime_error_code,
};
use super::*;

const AT: &str = "2026-08-27T12:00:00.000Z";

#[test]
fn unattended_launch_access_matches_each_provider_vocabulary() {
    assert_eq!(
        orchestration_launch_access("claude", "read-only"),
        (Some("plan".to_owned()), None)
    );
    assert_eq!(
        orchestration_launch_access("claude", "project"),
        (Some("auto".to_owned()), None)
    );
    assert_eq!(
        orchestration_launch_access("codex", "isolated"),
        (Some("never".to_owned()), Some(SandboxMode::WorkspaceWrite))
    );
    assert_eq!(
        orchestration_launch_access("codex", "read-only"),
        (Some("never".to_owned()), Some(SandboxMode::ReadOnly))
    );
    assert_eq!(
        orchestration_launch_access("acp:grok", "project"),
        (Some("auto".to_owned()), Some(SandboxMode::WorkspaceWrite))
    );
    assert_eq!(
        orchestration_launch_access("acp:qwen", "project"),
        (None, Some(SandboxMode::WorkspaceWrite))
    );
}

#[test]
fn unattended_claude_launch_loads_the_frozen_setting_sources_only_for_claude() {
    assert_eq!(
        orchestration_setting_sources("claude"),
        Some(vec![
            "user".to_owned(),
            "project".to_owned(),
            "local".to_owned()
        ])
    );
    assert_eq!(orchestration_setting_sources("codex"), None);
    assert_eq!(orchestration_setting_sources("acp:grok"), None);
}

#[test]
fn persisted_runtime_error_codes_are_pinned() {
    use interface_engine::EngineErrorKind as Engine;
    use interface_runtime::RuntimeErrorKind as Kind;
    let table = [
        (Kind::InvalidInput, "INVALIDINPUT"),
        (Kind::DuplicateEngine, "DUPLICATEENGINE"),
        (Kind::EngineNotRegistered, "ENGINENOTREGISTERED"),
        (Kind::SessionNotActive, "SESSIONNOTACTIVE"),
        (Kind::Lifecycle, "LIFECYCLE"),
        (Kind::Provider, "PROVIDER"),
        (Kind::Registry, "REGISTRY"),
        (Kind::History, "HISTORY"),
        (Kind::Internal, "INTERNAL"),
    ];
    for (kind, code) in table {
        assert_eq!(runtime_error_code(kind), code);
        assert_eq!(format!("{kind:?}").to_uppercase(), code);
    }
    let table = [
        (Engine::InvalidInput, "INVALIDINPUT"),
        (Engine::Unsupported, "UNSUPPORTED"),
        (Engine::Unavailable, "UNAVAILABLE"),
        (Engine::Lifecycle, "LIFECYCLE"),
        (Engine::Capacity, "CAPACITY"),
        (Engine::Timeout, "TIMEOUT"),
        (Engine::Transport, "TRANSPORT"),
        (Engine::Protocol, "PROTOCOL"),
        (Engine::Provider, "PROVIDER"),
        (Engine::Internal, "INTERNAL"),
    ];
    for (kind, code) in table {
        assert_eq!(engine_error_code(kind), code);
        assert_eq!(format!("{kind:?}").to_uppercase(), code);
    }
}

#[test]
fn adversarial_review_uses_provider_native_single_line_review_commands() {
    let codex = adversarial_review_command("codex");
    assert!(codex.starts_with("/review Review the current uncommitted changes"));
    assert!(!codex.contains('\n'));

    let claude = adversarial_review_command("claude");
    assert!(claude.starts_with("/review Review the builder's current work"));
    assert!(!claude.contains('\n'));
    assert!(claude.contains("Stay read-only"));
}

struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now(&self) -> String {
        AT.into()
    }
}

/// Returns a distinct timestamp per read so a test can prove which single
/// clock read reached both a record and the event that describes it.
#[derive(Default)]
struct SteppingClock {
    reads: AtomicUsize,
}

impl RuntimeClock for SteppingClock {
    fn now(&self) -> String {
        let read = self.reads.fetch_add(1, AtomicOrdering::SeqCst);
        format!("2026-08-27T12:00:00.{read:03}Z")
    }
}

struct FakeRuntime {
    launches: AtomicUsize,
    sends: AtomicUsize,
    stops: AtomicUsize,
    launch_requests: Mutex<Vec<RuntimeLaunchRequest>>,
    sent_messages: Mutex<Vec<(String, String)>>,
    desktop_summaries: Mutex<Vec<SessionSummary>>,
    desktop_active: Mutex<HashMap<String, bool>>,
    projected_events: Mutex<Vec<(String, EngineEvent)>>,
    launch: Mutex<VecDeque<ExternalOutcome<RuntimeLaunchSuccess>>>,
    deliveries: Mutex<VecDeque<ExternalOutcome<()>>>,
    stop_deliveries: Mutex<VecDeque<ExternalOutcome<()>>>,
    event_streams: Mutex<HashMap<String, BoundedEventStream<RuntimeEventFrame>>>,
    descriptions: Mutex<HashMap<String, RuntimeAgentDescription>>,
    engines: Vec<String>,
    history_engines: HashSet<String>,
    probe_results: HashMap<String, bool>,
    probes: AtomicUsize,
    command_discoveries: AtomicUsize,
    provider_commands: Mutex<HashMap<String, Vec<ProviderCommand>>>,
    models: HashMap<String, Vec<ModelOption>>,
    /// When set, every catalogue read waits here, so a read can only finish
    /// once every engine's read has started.
    model_barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl FakeRuntime {
    fn succeeding() -> Self {
        Self {
            launches: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            launch_requests: Mutex::new(Vec::new()),
            sent_messages: Mutex::new(Vec::new()),
            desktop_summaries: Mutex::new(Vec::new()),
            desktop_active: Mutex::new(HashMap::new()),
            projected_events: Mutex::new(Vec::new()),
            launch: Mutex::new(VecDeque::from([ExternalOutcome::Succeeded {
                value: RuntimeLaunchSuccess {
                    session_key: "codex#provider-one".into(),
                    provider_session_id: "provider-one".into(),
                    provider_effect_id: Some("provider-launch-one".into()),
                },
                provider_effect_id: Some("provider-launch-one".into()),
            }])),
            deliveries: Mutex::new(VecDeque::new()),
            stop_deliveries: Mutex::new(VecDeque::new()),
            event_streams: Mutex::new(HashMap::new()),
            descriptions: Mutex::new(HashMap::new()),
            engines: vec!["codex".into()],
            history_engines: HashSet::from(["codex".into()]),
            probe_results: HashMap::from([("codex".into(), true)]),
            probes: AtomicUsize::new(0),
            command_discoveries: AtomicUsize::new(0),
            provider_commands: Mutex::new(HashMap::new()),
            models: HashMap::from([(
                "codex".into(),
                vec![ModelOption {
                    value: "gpt-5.6".into(),
                    display_name: "GPT-5.6".into(),
                    description: Some("Fixture model".into()),
                    supports_effort: true,
                    effort_levels: vec!["high".into()],
                    is_default: Some(true),
                    supports_fast_mode: Some(true),
                }],
            )]),
            model_barrier: None,
        }
    }

    fn install_event_stream(
        &self,
        session_key: &str,
    ) -> interface_engine::EventSink<RuntimeEventFrame> {
        let (sink, stream) = interface_engine::bounded_event_stream(16);
        self.event_streams
            .lock()
            .expect("fake event streams")
            .insert(session_key.into(), stream);
        sink
    }
}

#[async_trait]
impl OrchestrationRuntimePort for FakeRuntime {
    async fn launch(&self, request: RuntimeLaunchRequest) -> ExternalOutcome<RuntimeLaunchSuccess> {
        self.launches.fetch_add(1, AtomicOrdering::SeqCst);
        self.launch_requests
            .lock()
            .expect("fake launch requests")
            .push(request.clone());
        let outcome = self
            .launch
            .lock()
            .expect("fake launch lock")
            .pop_front()
            .unwrap_or_else(|| ExternalOutcome::Unknown {
                code: "DUPLICATE_PROVIDER_ATTEMPT".into(),
                message: "launch was invoked more than once".into(),
            });
        if let ExternalOutcome::Succeeded { value, .. } = &outcome {
            self.desktop_active
                .lock()
                .expect("fake desktop active")
                .insert(value.session_key.clone(), true);
            self.desktop_summaries
                .lock()
                .expect("fake desktop summaries")
                .push(SessionSummary {
                    engine: request.engine_id.parse().expect("fake engine id"),
                    session_id: value.provider_session_id.clone(),
                    title: request.name,
                    cwd: request.canonical_cwd,
                    status: SessionStatus::Working,
                    cloud: false,
                    model: request.model,
                    effort: request.effort.map(Some),
                    last_activity: AT.into(),
                    first_seen: AT.into(),
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
                });
        }
        outcome
    }

    async fn send(&self, session_key: &str, body: String) -> ExternalOutcome<()> {
        self.sends.fetch_add(1, AtomicOrdering::SeqCst);
        self.sent_messages
            .lock()
            .expect("fake sent messages")
            .push((session_key.into(), body));
        self.deliveries
            .lock()
            .expect("fake deliveries lock")
            .pop_front()
            .unwrap_or(ExternalOutcome::Succeeded {
                value: (),
                provider_effect_id: Some("provider-send".into()),
            })
    }

    async fn stop(&self, _session_key: &str) -> ExternalOutcome<()> {
        self.stops.fetch_add(1, AtomicOrdering::SeqCst);
        self.stop_deliveries
            .lock()
            .expect("fake stop deliveries")
            .pop_front()
            .unwrap_or(ExternalOutcome::Succeeded {
                value: (),
                provider_effect_id: Some("provider-stop".into()),
            })
    }

    async fn transcript(
        &self,
        _session_key: &str,
        lines: usize,
        _live: Option<&crate::collaboration::DesktopCollaborationProjection>,
    ) -> Result<Vec<String>, RuntimeError> {
        Ok((0..lines).map(|index| format!("line {index}")).collect())
    }

    async fn provider_command_catalogue(
        &self,
        scope: ProviderCommandScope,
    ) -> Result<ProviderCommandCatalogue, RuntimeError> {
        self.command_discoveries
            .fetch_add(1, AtomicOrdering::SeqCst);
        let commands = self
            .provider_commands
            .lock()
            .expect("fake provider commands");
        let available = commands.contains_key(scope.engine.as_str());
        Ok(ProviderCommandCatalogue {
            commands: commands
                .get(scope.engine.as_str())
                .cloned()
                .unwrap_or_default(),
            scope,
            available,
            reason: (!available).then(|| "Fake provider commands are unavailable.".into()),
        })
    }

    fn describe_session(
        &self,
        session_key: &str,
    ) -> Result<Option<RuntimeAgentDescription>, RuntimeError> {
        Ok(self
            .descriptions
            .lock()
            .expect("fake descriptions lock")
            .get(session_key)
            .cloned())
    }

    async fn take_events(
        &self,
        session_key: &str,
    ) -> Result<Option<BoundedEventStream<RuntimeEventFrame>>, RuntimeError> {
        Ok(self
            .event_streams
            .lock()
            .expect("fake event streams")
            .remove(session_key))
    }

    fn desktop_session_summaries(&self) -> Result<Option<Vec<SessionSummary>>, RuntimeError> {
        Ok(Some(
            self.desktop_summaries
                .lock()
                .expect("fake desktop summaries")
                .clone(),
        ))
    }

    fn desktop_session_is_active(&self, session_key: &str) -> Result<Option<bool>, RuntimeError> {
        Ok(Some(
            self.desktop_active
                .lock()
                .expect("fake desktop active")
                .get(session_key)
                .copied()
                .unwrap_or(false),
        ))
    }

    fn project_desktop_event(
        &self,
        session_key: &str,
        event: &EngineEvent,
    ) -> Result<(), RuntimeError> {
        self.projected_events
            .lock()
            .expect("fake projected events")
            .push((session_key.into(), event.clone()));
        if let Some(summary) = self
            .desktop_summaries
            .lock()
            .expect("fake desktop summaries")
            .iter_mut()
            .find(|summary| format!("{}#{}", summary.engine, summary.session_id) == session_key)
        {
            match &event.event {
                EngineEventKind::StatusChanged { status } => summary.status = *status,
                EngineEventKind::SessionClosed => summary.status = SessionStatus::Idle,
                _ => {}
            }
        }
        Ok(())
    }

    fn engine_ids(&self) -> Vec<String> {
        self.engines.clone()
    }

    fn supports_provider_history(&self, engine_id: &str) -> bool {
        self.history_engines.contains(engine_id)
    }

    async fn probe_orchestration_engine(&self, engine_id: &str) -> Option<RuntimeEngineProbe> {
        self.probes.fetch_add(1, AtomicOrdering::SeqCst);
        self.probe_results
            .get(engine_id)
            .copied()
            .map(|steer| RuntimeEngineProbe { steer })
    }

    async fn model_catalogue(&self, engine_id: &str) -> Option<Vec<ModelOption>> {
        if let Some(barrier) = &self.model_barrier {
            barrier.wait().await;
        }
        self.models.get(engine_id).cloned()
    }
}

fn capability(
    capabilities: &[AgentCapabilityV1],
    facet: OrchestrationCapabilityFacet,
) -> &AgentCapabilityV1 {
    capabilities
        .iter()
        .find(|capability| capability.facet == facet)
        .expect("capability facet")
}

// Paused time: each commit runs on the blocking pool with real fsyncs, and
// the clock only moves when the runtime is idle, never during a commit, so
// the burst always lands inside one debounce window however slow the disk.
#[tokio::test(start_paused = true)]
async fn orchestration_pushes_coalesce_within_the_window_and_flush_on_close() {
    let (push_sender, mut push_receiver) = mpsc::channel(64);
    let fixture = fixture_with_runtime_and_pushes(
        Arc::new(FakeCoordination),
        Arc::new(FakeRuntime::succeeding()),
        Some(push_sender),
    );
    for index in 0..5 {
        fixture
            .dispatcher
            .apply(OrchestrationMutation::RotateRuntimeEpoch {
                runtime_epoch: format!("epoch-{index}"),
                meta: fixture.dispatcher.meta("Rotated the runtime epoch"),
            })
            .await
            .expect("rotate the runtime epoch");
    }
    assert!(
        push_receiver.try_recv().is_err(),
        "a burst must not push before its window closes"
    );

    fixture.dispatcher.close().await.expect("close flushes");
    let mut events = Vec::new();
    while let Ok(frame) = push_receiver.try_recv() {
        if frame.operation == CHANNELS.orchestration_event {
            events.push(frame);
        }
    }
    assert_eq!(
        events.len(),
        1,
        "five applies inside one window are one push"
    );
    assert_eq!(events[0].payload["runtimeEpoch"], json!("epoch-4"));

    // close must stop both coordinator tasks, not just drain them: a
    // stopped task drops its receiver, so scheduling now reports the push
    // surface as gone instead of parking a frame on a task nobody reads.
    for coordinator in [
        fixture.dispatcher.orchestration_pushes.as_ref(),
        fixture.dispatcher.registry_pushes.as_ref(),
    ] {
        assert!(
            !coordinator.expect("coordinator").schedule(),
            "close must stop the coordinator task"
        );
    }
}

#[tokio::test]
async fn close_leaves_the_hosts_shared_registry_coalescer_to_its_owner() {
    let (push_sender, mut push_receiver) = mpsc::channel(64);
    let shared = SharedRegistryPushes(RegistryPushCoordinator::start(
        || CoalescedPush::Idle,
        push_sender.clone(),
    ));
    let mut fixture = fixture_with_runtime_and_pushes(
        Arc::new(FakeCoordination),
        Arc::new(FakeRuntime::succeeding()),
        Some(push_sender),
    );
    fixture.dispatcher = fixture
        .dispatcher
        .clone()
        .with_shared_registry_pushes(shared.clone());
    fixture.dispatcher.close().await.expect("close");
    // Its own orchestration coalescer stops; the shared registry one keeps
    // serving the other owners until the host closes it.
    assert!(
        !fixture
            .dispatcher
            .orchestration_pushes
            .as_ref()
            .expect("orchestration coalescer")
            .schedule()
    );
    assert!(
        shared.0.schedule(),
        "orchestration close stopped the shared coalescer"
    );
    shared.close().await;
    assert!(!shared.0.schedule());
    while push_receiver.try_recv().is_ok() {}
}

#[tokio::test]
async fn snapshot_is_memoized_until_the_store_revision_moves() {
    let fixture = fixture(Arc::new(FakeCoordination));
    let first = fixture.dispatcher.snapshot().expect("first snapshot");
    let second = fixture.dispatcher.snapshot().expect("second snapshot");
    assert!(Arc::ptr_eq(&first, &second));

    fixture
        .dispatcher
        .apply(OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch: "epoch-rotated".into(),
            meta: fixture.dispatcher.meta("Rotated the runtime epoch"),
        })
        .await
        .expect("rotate the runtime epoch");

    // The mutation leaves the snapshot the store just committed as the memo,
    // so the next read shares it instead of cloning the graph again.
    let seeded = fixture
        .dispatcher
        .snapshot_cache
        .lock()
        .expect("snapshot memo")
        .clone()
        .expect("apply seeds the memo");
    let third = fixture.dispatcher.snapshot().expect("third snapshot");
    assert!(Arc::ptr_eq(&seeded.1, &third));
    assert!(!Arc::ptr_eq(&first, &third));
    assert_eq!(third.runtime_epoch, "epoch-rotated");
    assert!(Arc::ptr_eq(
        &third,
        &fixture.dispatcher.snapshot().expect("fourth snapshot")
    ));
}

#[tokio::test]
async fn snapshot_reads_do_not_wait_for_a_commit_holding_the_store() {
    let fixture = fixture(Arc::new(FakeCoordination));
    fixture
        .dispatcher
        .apply(OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch: "epoch-rotated".into(),
            meta: fixture.dispatcher.meta("Rotated the runtime epoch"),
        })
        .await
        .expect("a commit seeds the memo");
    let committed = fixture.dispatcher.snapshot().expect("committed snapshot");
    let read_snapshot = |dispatcher: OrchestrationHostDispatcher| {
        let (sent, read) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(dispatcher.snapshot().map(|snapshot| snapshot.revision));
        });
        read
    };

    // A commit holds the store lock across its fsync.
    let held = fixture.store.lock().expect("store lock");
    let read = read_snapshot(fixture.dispatcher.clone());
    assert_eq!(
        read.recv_timeout(Duration::from_secs(5))
            .expect("the read did not wait for the store")
            .expect("memoized snapshot"),
        committed.revision
    );

    // A failed write forgets the memo, so reads take the store lock again
    // and fail closed with the store.
    fixture.dispatcher.forget_snapshot();
    let read = read_snapshot(fixture.dispatcher.clone());
    assert!(
        read.recv_timeout(Duration::from_millis(100)).is_err(),
        "after a failed write the read waits for the store"
    );
    drop(held);
    assert_eq!(
        read.recv_timeout(Duration::from_secs(5))
            .expect("the read finished once the store was free")
            .expect("snapshot read under the store lock"),
        committed.revision
    );
}

#[tokio::test]
async fn missing_adapter_marks_every_orchestration_capability_unavailable_without_probing() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.engines.clear();

    let capabilities = agent_capabilities(&runtime, "codex").await;

    assert_eq!(capabilities.len(), 6);
    assert!(capabilities.iter().all(|capability| {
        capability.state == OrchestrationCapabilityState::Unavailable
            && capability.reason.as_deref()
                == Some("No active Interface adapter is registered for this provider.")
    }));
    assert_eq!(runtime.probes.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn historyless_engine_only_withholds_the_transcript_capability() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.history_engines.clear();

    let capabilities = agent_capabilities(&runtime, "codex").await;

    assert_eq!(
        capability(&capabilities, OrchestrationCapabilityFacet::Transcript),
        &unavailable_capability(
            OrchestrationCapabilityFacet::Transcript,
            "This provider does not expose stored transcript replay.",
        )
    );
    for facet in [
        OrchestrationCapabilityFacet::Reference,
        OrchestrationCapabilityFacet::Message,
        OrchestrationCapabilityFacet::Wake,
        OrchestrationCapabilityFacet::Spawn,
        OrchestrationCapabilityFacet::Worktree,
    ] {
        assert_eq!(
            capability(&capabilities, facet).state,
            OrchestrationCapabilityState::Available
        );
    }
}

#[tokio::test]
async fn probe_failure_withholds_spawn_and_wake_but_not_unrelated_capabilities() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.probe_results.clear();

    let capabilities = agent_capabilities(&runtime, "codex").await;

    assert_eq!(
        capability(&capabilities, OrchestrationCapabilityFacet::Wake),
        &unavailable_capability(
            OrchestrationCapabilityFacet::Wake,
            "This provider cannot safely steer a live turn.",
        )
    );
    assert_eq!(
        capability(&capabilities, OrchestrationCapabilityFacet::Spawn),
        &unavailable_capability(
            OrchestrationCapabilityFacet::Spawn,
            "The provider could not be probed by the active Interface host.",
        )
    );
    for facet in [
        OrchestrationCapabilityFacet::Reference,
        OrchestrationCapabilityFacet::Transcript,
        OrchestrationCapabilityFacet::Message,
        OrchestrationCapabilityFacet::Worktree,
    ] {
        assert_eq!(
            capability(&capabilities, facet).state,
            OrchestrationCapabilityState::Available
        );
    }
}

#[tokio::test]
async fn successful_non_steerable_probe_keeps_spawn_but_withholds_wake() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.probe_results.insert("codex".into(), false);

    let capabilities = agent_capabilities(&runtime, "codex").await;

    assert_eq!(
        capability(&capabilities, OrchestrationCapabilityFacet::Wake).state,
        OrchestrationCapabilityState::Unavailable
    );
    assert_eq!(
        capability(&capabilities, OrchestrationCapabilityFacet::Spawn),
        &available_capability(OrchestrationCapabilityFacet::Spawn)
    );
}

#[tokio::test]
async fn history_and_steer_capable_provider_advertises_the_frozen_positive_surface() {
    let runtime = FakeRuntime::succeeding();

    let capabilities = agent_capabilities(&runtime, "codex").await;

    assert_eq!(
        capabilities
            .iter()
            .map(|capability| capability.facet)
            .collect::<Vec<_>>(),
        vec![
            OrchestrationCapabilityFacet::Reference,
            OrchestrationCapabilityFacet::Transcript,
            OrchestrationCapabilityFacet::Message,
            OrchestrationCapabilityFacet::Wake,
            OrchestrationCapabilityFacet::Spawn,
            OrchestrationCapabilityFacet::Worktree,
        ]
    );
    assert!(capabilities.iter().all(|capability| {
        capability.state == OrchestrationCapabilityState::Available && capability.reason.is_none()
    }));
    assert_eq!(runtime.probes.load(AtomicOrdering::SeqCst), 1);
}

struct FakeCoordination;

#[async_trait]
impl OrchestrationCoordinationPort for FakeCoordination {
    async fn submit(&self, _agent: &AgentNodeV1) -> Result<CoordinationSubmission, String> {
        Ok(CoordinationSubmission {
            submission_id: "submission-one".into(),
            changes: 2,
            state: "submitted".into(),
        })
    }

    async fn integrate(
        &self,
        _agent: &AgentNodeV1,
        _dry_run: bool,
    ) -> Result<CoordinationIntegration, String> {
        Ok(CoordinationIntegration {
            task_id: "task-one".into(),
        })
    }

    async fn abandon(&self, _agent: &AgentNodeV1) -> Result<(), String> {
        Ok(())
    }

    async fn acknowledge(&self, _agent: &AgentNodeV1, _keep: &str) -> Result<Vec<String>, String> {
        Ok(vec!["src/pending.rs".into()])
    }

    async fn status(&self, _canonical_project_root: &str) -> CoordinationStatusV1 {
        CoordinationStatusV1 {
            registered: true,
            active_tasks: 1,
            published_uncommitted_paths: vec!["src/a.rs".into()],
            required_checks: 2,
        }
    }

    async fn transfer_worktree(
        &self,
        _lease_id: &str,
        _source_agent_id: &str,
        _target_agent_id: &str,
        _source_owner_revision: u64,
    ) -> ExternalOutcome<()> {
        ExternalOutcome::Succeeded {
            value: (),
            provider_effect_id: Some("transfer-one".into()),
        }
    }
}

struct Fixture {
    temporary: tempfile::TempDir,
    dispatcher: OrchestrationHostDispatcher,
    store: Arc<Mutex<OrchestrationStore>>,
    runtime: Arc<FakeRuntime>,
}

fn fixture(coordination: Arc<dyn OrchestrationCoordinationPort>) -> Fixture {
    fixture_with_runtime(coordination, Arc::new(FakeRuntime::succeeding()))
}

fn fixture_with_runtime(
    coordination: Arc<dyn OrchestrationCoordinationPort>,
    runtime: Arc<FakeRuntime>,
) -> Fixture {
    fixture_with_runtime_and_pushes(coordination, runtime, None)
}

fn fixture_with_runtime_and_pushes(
    coordination: Arc<dyn OrchestrationCoordinationPort>,
    runtime: Arc<FakeRuntime>,
    pushes: Option<mpsc::Sender<PushFrame>>,
) -> Fixture {
    let temporary = tempfile::tempdir().expect("temporary orchestration root");
    let root = temporary.path().join("Interface");
    let store = Arc::new(Mutex::new(
        OrchestrationStore::open(&root, "host-test", "epoch-test", AT)
            .expect("orchestration store"),
    ));
    let runtime_port: Arc<dyn OrchestrationRuntimePort> = runtime.clone();
    let mut dispatcher = OrchestrationHostDispatcher::new(
        store.clone(),
        runtime_port,
        coordination,
        "epoch-test".into(),
    )
    .with_clock(Arc::new(FixedClock));
    if let Some(pushes) = pushes {
        dispatcher = dispatcher.with_pushes(pushes);
    }
    Fixture {
        temporary,
        dispatcher,
        store,
        runtime,
    }
}

fn fixture_with_clock(
    coordination: Arc<dyn OrchestrationCoordinationPort>,
    clock: Arc<dyn RuntimeClock>,
) -> Fixture {
    let mut fixture = fixture(coordination);
    fixture.dispatcher = fixture.dispatcher.clone().with_clock(clock);
    fixture
}

fn adversarial_runtime() -> Arc<FakeRuntime> {
    let mut runtime = FakeRuntime::succeeding();
    runtime.engines = vec!["claude".into(), "codex".into()];
    runtime.history_engines = HashSet::from(["claude".into(), "codex".into()]);
    runtime.probe_results = HashMap::from([("claude".into(), true), ("codex".into(), true)]);
    runtime
        .provider_commands
        .lock()
        .expect("fake provider commands")
        .insert(
            "claude".into(),
            vec![ProviderCommand {
                name: "review".into(),
                description: "Review the current project changes.".into(),
                argument_hint: Some("instructions".into()),
                aliases: None,
            }],
        );
    runtime.launch.lock().expect("fake launch queue").clear();
    runtime.launch.lock().expect("fake launch queue").extend([
        ExternalOutcome::Succeeded {
            value: RuntimeLaunchSuccess {
                session_key: "claude#reviewer-one".into(),
                provider_session_id: "reviewer-one".into(),
                provider_effect_id: Some("provider-reviewer-one".into()),
            },
            provider_effect_id: Some("provider-reviewer-one".into()),
        },
        ExternalOutcome::Succeeded {
            value: RuntimeLaunchSuccess {
                session_key: "codex#builder-one".into(),
                provider_session_id: "builder-one".into(),
                provider_effect_id: Some("provider-builder-one".into()),
            },
            provider_effect_id: Some("provider-builder-one".into()),
        },
    ]);
    Arc::new(runtime)
}

fn adversarial_payload(cwd: &Path, client_request_id: &str, objective: &str) -> Value {
    json!({
        "builderEngineId": "codex",
        "reviewerEngineId": "claude",
        "canonicalCwd": cwd,
        "objective": objective,
        "clientRequestId": client_request_id,
    })
}

fn emit_runtime_event(
    sink: &interface_engine::EventSink<RuntimeEventFrame>,
    session_key: &str,
    sequence: u64,
    event: EngineEventKind,
) {
    sink.emit(RuntimeEventFrame::Event {
        value: interface_runtime::RuntimeEvent {
            session_key: session_key.into(),
            sequence,
            received_at: AT.into(),
            event: EngineEvent {
                at: Some(AT.into()),
                event,
            },
        },
    })
    .expect("emit fake runtime event");
}

/// How long these helpers wait for a state the test drives. An upper bound
/// only: the gate runs four test binaries at once, often beside other
/// sessions' builds, and two seconds was not always enough for a turn of the
/// reducer to be scheduled.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

async fn wait_for_agent_state(
    store: &Arc<Mutex<OrchestrationStore>>,
    agent_id: &str,
    state: AgentNodeState,
) {
    tokio::time::timeout(WAIT_LIMIT, async {
        loop {
            let current = store
                .lock()
                .expect("orchestration store")
                .snapshot()
                .expect("snapshot")
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .map(|agent| agent.state);
            if current == Some(state) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("agent state transition timed out");
}

async fn wait_for_send_count(runtime: &FakeRuntime, expected: usize) {
    tokio::time::timeout(WAIT_LIMIT, async {
        while runtime.sends.load(AtomicOrdering::SeqCst) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("send count timed out");
}

async fn wait_for_projected_event_count(runtime: &FakeRuntime, expected: usize) {
    tokio::time::timeout(WAIT_LIMIT, async {
        while runtime
            .projected_events
            .lock()
            .expect("fake projected events")
            .len()
            != expected
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("projected event count timed out");
}

fn create_payload(cwd: &Path, client_request_id: &str) -> Value {
    json!({
        "engineId": "codex",
        "name": "Builder",
        "prompt": "Implement the reviewed change.",
        "canonicalCwd": cwd,
        "accessMode": "project",
        "clientRequestId": client_request_id,
    })
}

#[tokio::test]
async fn duplicate_agent_id_discards_the_worktree_it_prepared() {
    struct CountingWorktrees {
        checkout: std::path::PathBuf,
        discarded: AtomicUsize,
    }

    #[async_trait]
    impl WorktreeService for CountingWorktrees {
        async fn create(
            &self,
            cwd: String,
            name: Option<String>,
        ) -> Result<interface_registry::Worktree, crate::worktree::WorktreeServiceError> {
            self.create_initialized(cwd, name).await
        }

        async fn create_initialized(
            &self,
            cwd: String,
            _name: Option<String>,
        ) -> Result<interface_registry::Worktree, crate::worktree::WorktreeServiceError> {
            let source = std::path::PathBuf::from(cwd);
            Ok(interface_registry::Worktree {
                path: self.checkout.clone(),
                session_cwd: self.checkout.clone(),
                branch: "interface/agent".into(),
                repo_root: source.clone(),
                source_cwd: source,
                dirty_source: false,
            })
        }

        async fn persist(
            &self,
            _key: &str,
            _worktree: &interface_registry::Worktree,
        ) -> Result<(), crate::worktree::WorktreeServiceError> {
            Ok(())
        }

        async fn discard(&self, _worktree: interface_registry::Worktree) -> Option<String> {
            self.discarded.fetch_add(1, AtomicOrdering::SeqCst);
            None
        }
    }

    let fixture = fixture(Arc::new(FakeCoordination));
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let worktrees = Arc::new(CountingWorktrees {
        checkout: fixture.temporary.path().join("checkout"),
        discarded: AtomicUsize::new(0),
    });
    let service: Arc<dyn WorktreeService> = worktrees.clone();
    let dispatcher = fixture.dispatcher.with_worktrees(service);

    let mut first = create_payload(&cwd, "first-agent");
    first["agentId"] = json!("agent_duplicate");
    dispatcher
        .dispatch(CHANNELS.create_orchestration_agent, first)
        .await
        .expect("create the first agent");

    let mut duplicate = create_payload(&cwd, "second-agent");
    duplicate["agentId"] = json!("agent_duplicate");
    duplicate["accessMode"] = json!("isolated");
    let error = dispatcher
        .dispatch(CHANNELS.create_orchestration_agent, duplicate)
        .await
        .expect_err("a duplicate agent id is refused");

    assert_eq!(error.code, "orchestration-conflict");
    assert_eq!(
        worktrees.discarded.load(AtomicOrdering::SeqCst),
        1,
        "the refused launch must not leak the checkout it prepared"
    );
}

#[test]
fn exact_orchestration_owner_table_is_frozen_and_unique() {
    assert_eq!(ORCHESTRATION_OPERATIONS.len(), 23);
    assert_eq!(
        ORCHESTRATION_OPERATIONS
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        23
    );
    assert_eq!(
        ORCHESTRATION_OPERATIONS.first(),
        Some(&"iface:getOrchestrationSnapshot")
    );
    assert_eq!(
        ORCHESTRATION_OPERATIONS.last(),
        Some(&"iface:getOrchestratorModels")
    );
}

#[tokio::test]
async fn adversarial_review_start_is_idempotent_conflicts_on_changed_payload_and_is_read_only() {
    let runtime = adversarial_runtime();
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let payload = adversarial_payload(&cwd, "pair-one", "Implement the bounded change.");

    let first = fixture
        .dispatcher
        .dispatch(CHANNELS.start_adversarial_review, payload.clone())
        .await
        .expect("start adversarial review");
    assert_eq!(first["version"], 1);
    assert_eq!(first["builder"]["engineId"], "codex");
    assert_eq!(first["builder"]["scope"]["accessMode"], "project");
    assert_eq!(first["reviewer"]["engineId"], "claude");
    assert_eq!(first["reviewer"]["scope"]["accessMode"], "read-only");
    assert_eq!(first["reviewer"]["state"], "working");
    assert!(first["builder"]["sessionKey"].as_str().is_some());
    assert!(first["reviewer"]["sessionKey"].as_str().is_some());
    {
        let launch_requests = runtime
            .launch_requests
            .lock()
            .expect("fake launch requests");
        assert_eq!(launch_requests.len(), 2);
        assert_eq!(launch_requests[0].engine_id, "claude");
        assert_eq!(launch_requests[0].access_mode, "read-only");
        assert_eq!(launch_requests[1].engine_id, "codex");
        assert_eq!(launch_requests[1].access_mode, "project");
    }

    let replay = fixture
        .dispatcher
        .dispatch(CHANNELS.start_adversarial_review, payload)
        .await
        .expect("idempotent pair replay");
    assert_eq!(replay["builder"]["agentId"], first["builder"]["agentId"]);
    assert_eq!(replay["reviewer"]["agentId"], first["reviewer"]["agentId"]);
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 2);

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.start_adversarial_review,
            adversarial_payload(&cwd, "pair-one", "A changed objective."),
        )
        .await
        .expect_err("changed payload must conflict");
    assert_eq!(error.code, "orchestration-conflict");
    assert!(error.replay_safe);
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(
        runtime.command_discoveries.load(AtomicOrdering::SeqCst),
        1,
        "idempotent/conflicting replays must not depend on a changed provider catalogue"
    );
}

#[tokio::test]
async fn adversarial_review_rejects_unsupported_engine_pair_before_provider_contact() {
    let runtime = adversarial_runtime();
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.start_adversarial_review,
            json!({
                "builderEngineId": "codex",
                "reviewerEngineId": "gemini",
                "canonicalCwd": cwd,
                "objective": "Review the implementation.",
                "clientRequestId": "unsupported-pair",
            }),
        )
        .await
        .expect_err("unsupported pair");

    assert_eq!(error.code, INVALID_PAYLOAD_CODE);
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(runtime.probes.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(runtime.command_discoveries.load(AtomicOrdering::SeqCst), 0);
    assert!(
        fixture
            .store
            .lock()
            .expect("orchestration store")
            .snapshot()
            .expect("snapshot")
            .agents
            .is_empty()
    );
}

#[tokio::test]
async fn adversarial_review_rejects_claude_without_review_before_provider_launch() {
    let runtime = adversarial_runtime();
    runtime
        .provider_commands
        .lock()
        .expect("fake provider commands")
        .insert("claude".into(), Vec::new());
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.start_adversarial_review,
            adversarial_payload(
                &cwd,
                "claude-without-review",
                "Refuse before either agent launches.",
            ),
        )
        .await
        .expect_err("Claude without /review must fail closed");

    assert_eq!(error.code, "orchestration-blocked");
    assert!(error.message.contains("did not advertise /review"));
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(runtime.probes.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(runtime.command_discoveries.load(AtomicOrdering::SeqCst), 1);
    assert!(
        fixture
            .store
            .lock()
            .expect("orchestration store")
            .snapshot()
            .expect("snapshot")
            .agents
            .is_empty()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn adversarial_review_duplicate_idle_edges_send_each_handoff_exactly_once() {
    let runtime = adversarial_runtime();
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let started = fixture
        .dispatcher
        .dispatch(
            CHANNELS.start_adversarial_review,
            adversarial_payload(&cwd, "pair-lifecycle", "Implement and verify the change."),
        )
        .await
        .expect("start adversarial review");
    let builder_id = started["builder"]["agentId"]
        .as_str()
        .expect("builder id")
        .to_owned();
    let reviewer_id = started["reviewer"]["agentId"]
        .as_str()
        .expect("reviewer id")
        .to_owned();

    fixture
        .dispatcher
        .settle_observed_agent(
            &builder_id,
            ObservedCompletion::Idle,
            "builder idle".into(),
            Some("builder transcript".into()),
        )
        .await;
    fixture
        .dispatcher
        .settle_observed_agent(
            &builder_id,
            ObservedCompletion::Idle,
            "duplicate builder idle".into(),
            Some("builder transcript".into()),
        )
        .await;
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 0);

    fixture
        .dispatcher
        .settle_observed_agent(
            &reviewer_id,
            ObservedCompletion::Idle,
            "reviewer ready".into(),
            Some("reviewer is waiting for the review packet".into()),
        )
        .await;
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 1);

    fixture
        .dispatcher
        .settle_observed_agent(
            &reviewer_id,
            ObservedCompletion::Idle,
            "reviewer idle".into(),
            Some("reviewer found one defect".into()),
        )
        .await;
    fixture
        .dispatcher
        .settle_observed_agent(
            &reviewer_id,
            ObservedCompletion::Idle,
            "duplicate reviewer idle".into(),
            Some("reviewer found one defect".into()),
        )
        .await;
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 2);

    fixture
        .dispatcher
        .settle_observed_agent(
            &builder_id,
            ObservedCompletion::Idle,
            "builder corrected result".into(),
            Some("builder corrected transcript".into()),
        )
        .await;
    fixture
        .dispatcher
        .settle_observed_agent(
            &builder_id,
            ObservedCompletion::Idle,
            "duplicate final idle".into(),
            Some("builder corrected transcript".into()),
        )
        .await;

    let snapshot = fixture
        .store
        .lock()
        .expect("orchestration store")
        .snapshot()
        .expect("snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(
        snapshot
            .effects
            .iter()
            .filter(|effect| effect.kind == OrchestrationEffectKind::SendMessage)
            .count(),
        2
    );
    assert!(snapshot.messages.iter().all(message_processed));
    assert_eq!(
        find_agent(&snapshot, &builder_id).expect("builder").state,
        AgentNodeState::Completed
    );
    assert_eq!(
        find_agent(&snapshot, &reviewer_id).expect("reviewer").state,
        AgentNodeState::Completed
    );
    let sent = runtime.sent_messages.lock().expect("fake sent messages");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].0, "claude#reviewer-one");
    assert!(sent[0].1.starts_with("/review "));
    assert!(!sent[0].1.contains('\n'));
    assert_eq!(sent[1].0, "codex#builder-one");
    assert!(sent[1].1.contains("untrusted evidence"));
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 2);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn adversarial_sessions_are_registry_visible_and_fan_out_events_during_handoff() {
    let runtime = adversarial_runtime();
    let reviewer_events = runtime.install_event_stream("claude#reviewer-one");
    let builder_events = runtime.install_event_stream("codex#builder-one");
    let (push_sender, mut push_receiver) = mpsc::channel(128);
    let fixture = fixture_with_runtime_and_pushes(
        Arc::new(FakeCoordination),
        runtime.clone(),
        Some(push_sender),
    );
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let started = fixture
        .dispatcher
        .dispatch(
            CHANNELS.start_adversarial_review,
            adversarial_payload(&cwd, "pair-visible", "Implement and review the change."),
        )
        .await
        .expect("start visible adversarial pair");
    let builder_id = started["builder"]["agentId"]
        .as_str()
        .expect("builder id")
        .to_owned();
    let reviewer_id = started["reviewer"]["agentId"]
        .as_str()
        .expect("reviewer id")
        .to_owned();
    let builder_key = started["builder"]["sessionKey"]
        .as_str()
        .expect("builder key")
        .to_owned();
    let reviewer_key = started["reviewer"]["sessionKey"]
        .as_str()
        .expect("reviewer key")
        .to_owned();

    let registry = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let frame = push_receiver.recv().await.expect("registry push channel");
            if frame.operation == CHANNELS.registry
                && frame
                    .payload
                    .as_array()
                    .is_some_and(|items| items.len() == 2)
            {
                return frame.payload;
            }
        }
    })
    .await
    .expect("two-session registry push timed out");
    let visible_keys = registry
        .as_array()
        .expect("registry array")
        .iter()
        .map(|summary| {
            format!(
                "{}#{}",
                summary["engine"].as_str().expect("engine"),
                summary["sessionId"].as_str().expect("session id")
            )
        })
        .collect::<HashSet<_>>();
    assert_eq!(
        visible_keys,
        HashSet::from([builder_key.clone(), reviewer_key.clone()])
    );

    emit_runtime_event(
        &builder_events,
        &builder_key,
        1,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    emit_runtime_event(
        &builder_events,
        &builder_key,
        2,
        EngineEventKind::MessageDelta {
            text: "builder result".into(),
            agent_key: None,
        },
    );
    emit_runtime_event(
        &builder_events,
        &builder_key,
        3,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    wait_for_agent_state(&fixture.store, &builder_id, AgentNodeState::Idle).await;
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 0);

    emit_runtime_event(
        &reviewer_events,
        &reviewer_key,
        1,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    wait_for_send_count(runtime.as_ref(), 1).await;
    emit_runtime_event(
        &reviewer_events,
        &reviewer_key,
        2,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    emit_runtime_event(
        &reviewer_events,
        &reviewer_key,
        3,
        EngineEventKind::MessageDelta {
            text: "review finding".into(),
            agent_key: None,
        },
    );
    emit_runtime_event(
        &reviewer_events,
        &reviewer_key,
        4,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    wait_for_send_count(runtime.as_ref(), 2).await;
    emit_runtime_event(
        &builder_events,
        &builder_key,
        4,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    emit_runtime_event(
        &builder_events,
        &builder_key,
        5,
        EngineEventKind::MessageDelta {
            text: "corrected result".into(),
            agent_key: None,
        },
    );
    emit_runtime_event(
        &builder_events,
        &builder_key,
        6,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    wait_for_agent_state(&fixture.store, &builder_id, AgentNodeState::Completed).await;
    wait_for_agent_state(&fixture.store, &reviewer_id, AgentNodeState::Completed).await;

    let mut event_keys = HashSet::new();
    while let Ok(frame) = push_receiver.try_recv() {
        if frame.operation == CHANNELS.event
            && let Some(key) = frame.payload["key"].as_str()
        {
            event_keys.insert(key.to_owned());
        }
    }
    assert!(event_keys.contains(&builder_key));
    assert!(event_keys.contains(&reviewer_key));
    assert_eq!(runtime.sends.load(AtomicOrdering::SeqCst), 2);
}

#[tokio::test]
async fn orchestration_event_owner_suppresses_internal_close_and_projects_terminal_once() {
    let runtime = Arc::new(FakeRuntime::succeeding());
    let events = runtime.install_event_stream("codex#provider-one");
    let (push_sender, mut push_receiver) = mpsc::channel(64);
    let fixture = fixture_with_runtime_and_pushes(
        Arc::new(FakeCoordination),
        runtime.clone(),
        Some(push_sender),
    );
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "visible-close"),
        )
        .await
        .expect("create visible agent");
    // Both orchestration pumps coalesce behind a 50 ms window now, so the
    // drain has to outlast it or a create_agent frame lands on the
    // suppression assertion below.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while push_receiver.try_recv().is_ok() {}

    emit_runtime_event(
        &events,
        "codex#provider-one",
        1,
        EngineEventKind::SessionClosed,
    );
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(
        runtime
            .projected_events
            .lock()
            .expect("fake projected events")
            .is_empty(),
        "provider-internal close must remain suppressed"
    );
    assert!(push_receiver.try_recv().is_err());

    runtime
        .desktop_active
        .lock()
        .expect("fake desktop active")
        .insert("codex#provider-one".into(), false);
    events
        .emit(RuntimeEventFrame::Terminal {
            session_key: "codex#provider-one".into(),
            reason: RuntimeEventTermination::Closed,
            message: None,
        })
        .expect("emit terminal frame");
    wait_for_projected_event_count(runtime.as_ref(), 1).await;

    let mut closes = 0;
    let mut saw_idle_registry = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while closes == 0 || !saw_idle_registry {
            let frame = push_receiver.recv().await.expect("desktop push");
            if frame.operation == CHANNELS.event
                && matches!(
                    serde_json::from_value::<EngineEvent>(frame.payload["event"].clone())
                        .expect("engine event")
                        .event,
                    EngineEventKind::SessionClosed
                )
            {
                closes += 1;
            }
            if frame.operation == CHANNELS.registry
                && frame.payload.as_array().is_some_and(|summaries| {
                    summaries.iter().any(|summary| {
                        summary["sessionId"] == "provider-one" && summary["status"] == "idle"
                    })
                })
            {
                saw_idle_registry = true;
            }
        }
    })
    .await
    .expect("terminal desktop projection timed out");
    while let Ok(frame) = push_receiver.try_recv() {
        if frame.operation == CHANNELS.event
            && matches!(
                serde_json::from_value::<EngineEvent>(frame.payload["event"].clone())
                    .expect("engine event")
                    .event,
                EngineEventKind::SessionClosed
            )
        {
            closes += 1;
        }
    }
    assert_eq!(closes, 1);
    assert_eq!(
        runtime
            .projected_events
            .lock()
            .expect("fake projected events")
            .len(),
        1
    );
}

#[tokio::test]
async fn orchestration_stream_end_projects_close_but_active_replacement_suppresses_it() {
    let runtime = Arc::new(FakeRuntime::succeeding());
    let events = runtime.install_event_stream("codex#provider-one");
    let (push_sender, mut push_receiver) = mpsc::channel(64);
    let fixture = fixture_with_runtime_and_pushes(
        Arc::new(FakeCoordination),
        runtime.clone(),
        Some(push_sender),
    );
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "stream-end-close"),
        )
        .await
        .expect("create visible agent");
    while push_receiver.try_recv().is_ok() {}

    fixture
        .dispatcher
        .project_desktop_terminal_close("codex#provider-one")
        .await;
    assert!(
        runtime
            .projected_events
            .lock()
            .expect("fake projected events")
            .is_empty(),
        "a live replacement suppresses a stale terminal close"
    );

    runtime
        .desktop_active
        .lock()
        .expect("fake desktop active")
        .insert("codex#provider-one".into(), false);
    drop(events);
    wait_for_projected_event_count(runtime.as_ref(), 1).await;
    let close_push = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let frame = push_receiver.recv().await.expect("desktop push");
            if frame.operation == CHANNELS.event
                && matches!(
                    serde_json::from_value::<EngineEvent>(frame.payload["event"].clone())
                        .expect("engine event")
                        .event,
                    EngineEventKind::SessionClosed
                )
            {
                return frame;
            }
        }
    })
    .await
    .expect("stream-end close push timed out");
    assert_eq!(close_push.payload["key"], "codex#provider-one");
}

#[tokio::test]
async fn adversarial_review_returns_visible_partial_failure_without_retrying_launch() {
    let runtime = adversarial_runtime();
    {
        let mut launches = runtime.launch.lock().expect("fake launch queue");
        launches.pop_back();
        launches.push_back(ExternalOutcome::DefinitelyUnsent {
            code: "BUILDER_UNAVAILABLE".into(),
            message: "builder could not start".into(),
        });
    }
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let payload = adversarial_payload(&cwd, "pair-partial", "Implement the change.");

    let partial = fixture
        .dispatcher
        .dispatch(CHANNELS.start_adversarial_review, payload.clone())
        .await
        .expect("partial result remains visible");
    assert_eq!(partial["reviewer"]["state"], "working");
    assert_eq!(partial["builder"]["state"], "failed");
    assert!(partial["reviewer"]["sessionKey"].as_str().is_some());
    assert!(partial["builder"].get("sessionKey").is_none());
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 2);

    let replay = fixture
        .dispatcher
        .dispatch(CHANNELS.start_adversarial_review, payload)
        .await
        .expect("partial replay");
    assert_eq!(replay["builder"]["state"], "failed");
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 2);
    assert!(
        fixture
            .store
            .lock()
            .expect("orchestration store")
            .snapshot()
            .expect("snapshot")
            .effects
            .iter()
            .any(|effect| {
                effect.kind == OrchestrationEffectKind::SpawnAgent
                    && effect.state == OrchestrationEffectState::Failed
            })
    );
}

#[tokio::test]
async fn manual_spawn_is_blocked_before_state_or_provider_contact_when_probe_failed() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.probe_results.clear();
    let runtime = Arc::new(runtime);
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "probe-failed-create"),
        )
        .await
        .expect_err("spawn capability must be proven before provider contact");

    assert_eq!(error.message, "Provider lacks spawn capability.");
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 0);
    assert!(
        fixture
            .store
            .lock()
            .expect("orchestration store")
            .snapshot()
            .expect("snapshot")
            .agents
            .is_empty()
    );
}

#[tokio::test]
async fn existing_session_registration_survives_probe_failure_with_honest_capabilities() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.probe_results.clear();
    let runtime = Arc::new(runtime);
    runtime
        .descriptions
        .lock()
        .expect("fake descriptions")
        .insert(
            "codex#existing".into(),
            RuntimeAgentDescription {
                engine_id: "codex".into(),
                provider_session_id: "existing".into(),
                canonical_cwd: "/tmp".into(),
                canonical_project_root: Some("/tmp".into()),
                name: Some("Existing session".into()),
                state: AgentNodeState::Idle,
            },
        );
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), runtime.clone());

    fixture
        .dispatcher
        .dispatch(
            CHANNELS.register_orchestration_session,
            json!({
                "sessionKey": "codex#existing",
                "clientRequestId": "register-existing",
            }),
        )
        .await
        .expect("registration does not require spawn capability");
    let snapshot = fixture
        .store
        .lock()
        .expect("orchestration store")
        .snapshot()
        .expect("snapshot");
    let agent = snapshot.agents.first().expect("registered agent");

    assert_eq!(agent.state, AgentNodeState::Idle);
    assert_eq!(
        capability(&agent.capabilities, OrchestrationCapabilityFacet::Spawn).state,
        OrchestrationCapabilityState::Unavailable
    );
    assert_eq!(
        capability(&agent.capabilities, OrchestrationCapabilityFacet::Reference).state,
        OrchestrationCapabilityState::Available
    );
    assert_eq!(runtime.launches.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn mutation_events_reuse_the_single_clock_read_of_the_record_they_describe() {
    let fixture = fixture_with_clock(
        Arc::new(FakeCoordination),
        Arc::new(SteppingClock::default()),
    );
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "clock-agent"),
        )
        .await
        .expect("create agent");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.send_orchestration_message,
            json!({
                "recipientAgentId": created["agentId"],
                "body": "Please inspect the failing test.",
                "expectReply": false,
                "clientRequestId": "clock-message",
            }),
        )
        .await
        .expect("send message");
    let snapshot = fixture
        .store
        .lock()
        .expect("store")
        .snapshot()
        .expect("snapshot");
    let event_at = |summary: &str| {
        snapshot
            .events
            .iter()
            .find(|event| event.summary == summary)
            .unwrap_or_else(|| panic!("event {summary} was appended"))
    };

    let intent = event_at("Registered manual agent Builder");
    assert_eq!(intent.actor.kind, OrchestrationActorKind::Human);
    assert_eq!(intent.at, snapshot.agents[0].created_at);
    assert_eq!(
        intent.at,
        snapshot
            .effects
            .iter()
            .find(|effect| effect.kind == OrchestrationEffectKind::SpawnAgent)
            .expect("spawn reservation")
            .reserved_at
    );
    // A second helper-built event must not reuse the first event's read.
    assert_ne!(intent.at, event_at("Starting manual agent").at);

    let requested = event_at("Requested agent message");
    assert_eq!(requested.actor.kind, OrchestrationActorKind::Human);
    assert_eq!(requested.at, snapshot.messages[0].created_at);
    assert_eq!(requested.at, snapshot.messages[0].receipts[0].at);

    let queued = event_at("Message Queued");
    assert_eq!(queued.actor.kind, OrchestrationActorKind::Host);
    assert_eq!(queued.at, snapshot.messages[0].receipts[1].at);
}

#[tokio::test]
async fn provider_effects_are_reserved_once_and_unknown_delivery_is_never_replayed() {
    let fixture = fixture(Arc::new(FakeCoordination));
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");

    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "create-one"),
        )
        .await
        .expect("create agent");
    assert!(created["agentId"].as_str().is_some());
    assert_eq!(created["state"], "working");
    assert!(created.get("providerSessionId").is_none());
    assert_eq!(fixture.runtime.launches.load(AtomicOrdering::SeqCst), 1);

    fixture
        .runtime
        .deliveries
        .lock()
        .expect("delivery queue")
        .push_back(ExternalOutcome::Unknown {
            code: "TRANSPORT_EOF".into(),
            message: "provider outcome is unknown".into(),
        });
    let agent_id = created["agentId"].as_str().expect("agent id");
    let message_payload = json!({
        "recipientAgentId": agent_id,
        "body": "Please inspect the failing test.",
        "expectReply": true,
        "clientRequestId": "message-one",
    });
    let first = fixture
        .dispatcher
        .dispatch(CHANNELS.send_orchestration_message, message_payload.clone())
        .await
        .expect("unknown message remains a durable result");
    assert_eq!(
        first["receipts"]
            .as_array()
            .and_then(|items| items.last())
            .and_then(|receipt| receipt.get("state")),
        Some(&Value::String("unknown".into()))
    );
    let replay = fixture
        .dispatcher
        .dispatch(CHANNELS.send_orchestration_message, message_payload)
        .await
        .expect("idempotent message replay");
    assert_eq!(replay["messageId"], first["messageId"]);
    assert_eq!(fixture.runtime.sends.load(AtomicOrdering::SeqCst), 1);

    let snapshot = fixture
        .store
        .lock()
        .expect("store")
        .snapshot()
        .expect("snapshot");
    assert!(snapshot.effects.iter().any(|effect| {
        effect.kind == OrchestrationEffectKind::SendMessage
            && effect.state == OrchestrationEffectState::Unknown
    }));
}

#[tokio::test]
async fn policy_lineage_coordination_and_model_results_match_legacy_shapes() {
    let fixture = fixture(Arc::new(FakeCoordination));
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "create-orchestrator"),
        )
        .await
        .expect("create orchestrator");
    let agent_id = created["agentId"].as_str().expect("agent id");

    let policy = fixture
        .dispatcher
        .dispatch(
            CHANNELS.set_orchestrator_policy,
            json!({
                "agentId": agent_id,
                "autonomy": "paused",
                "clientRequestId": "policy-one",
            }),
        )
        .await
        .expect("set policy");
    assert_eq!(policy["policy"]["autonomy"], "paused");

    let submit = fixture
        .dispatcher
        .dispatch(
            CHANNELS.submit_coordinated_worker,
            json!({"agentId": agent_id, "clientRequestId": "submit-one"}),
        )
        .await
        .expect("submit");
    assert_eq!(
        submit,
        json!({"submissionId":"submission-one","changes":2,"state":"submitted"})
    );
    let integrate = fixture
        .dispatcher
        .dispatch(
            CHANNELS.integrate_coordinated_worker,
            json!({
                "agentId": agent_id,
                "dryRun": true,
                "clientRequestId": "integrate-one",
            }),
        )
        .await
        .expect("integrate claim");
    assert_eq!(
        integrate,
        json!({
            "agentId": agent_id,
            "taskId": "task-one",
            "dryRun": true,
            "state": "verifying",
        })
    );
    let models = fixture
        .dispatcher
        .dispatch(CHANNELS.get_orchestrator_models, Value::Null)
        .await
        .expect("cached model catalogue");
    assert_eq!(models["catalogue"], "live");
    assert_eq!(models["engines"][0]["default"], "gpt-5.6");
    assert_eq!(models["synthetic"]["ultracode"], ULTRACODE_NOTE);
}

#[tokio::test]
async fn orchestrator_models_ask_every_provider_together() {
    let mut runtime = FakeRuntime::succeeding();
    runtime.engines = vec!["codex".into(), "claude".into()];
    // Each catalogue read waits for the other to start, so reading them one
    // after another could never finish.
    runtime.model_barrier = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let fixture = fixture_with_runtime(Arc::new(FakeCoordination), Arc::new(runtime));
    let models = tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .dispatcher
            .dispatch(CHANNELS.get_orchestrator_models, Value::Null),
    )
    .await
    .expect("catalogues are read concurrently")
    .expect("model catalogue");
    assert_eq!(models["engines"][0]["engineId"], "codex");
    assert_eq!(models["engines"][0]["default"], "gpt-5.6");
    assert_eq!(models["engines"][1]["engineId"], "claude");
    assert_eq!(models["engines"][1]["ok"], false);
}

#[tokio::test]
async fn recovery_marks_unattempted_effect_definitely_unsent_before_rotating_epoch() {
    let temporary = tempfile::tempdir().expect("temporary orchestration root");
    let root = temporary.path().join("Interface");
    let mut initial =
        OrchestrationStore::open(&root, "host-recovery", "epoch-old", AT).expect("store");
    let agent = AgentNodeV1 {
        version: 1,
        agent_id: "agent-old".into(),
        name: Some("Old worker".into()),
        engine_id: "codex".into(),
        session_key: None,
        provider_session_id: None,
        runtime_epoch: "epoch-old".into(),
        parent_agent_id: None,
        role: None,
        depth: 0,
        state: AgentNodeState::Planned,
        capabilities: derived_capabilities(true, Some(RuntimeEngineProbe { steer: true })),
        scope: AgentScopeV1 {
            canonical_cwd: temporary.path().to_string_lossy().into_owned(),
            canonical_project_root: Some(temporary.path().to_string_lossy().into_owned()),
            access_mode: "project".into(),
            scope_digest: "a".repeat(64),
            worktree_lease_id: None,
        },
        created_at: AT.into(),
        updated_at: AT.into(),
    };
    let effect = EffectReservationV1 {
        version: 1,
        effect_id: "effect-old".into(),
        idempotency_key: "manual-spawn:old".into(),
        kind: OrchestrationEffectKind::SpawnAgent,
        target_kind: OrchestrationEffectTargetKind::Agent,
        target_id: agent.agent_id.clone(),
        state: OrchestrationEffectState::Reserved,
        attempts: 0,
        request_hash: "b".repeat(64),
        runtime_epoch: "epoch-old".into(),
        reserved_at: AT.into(),
        started_at: None,
        settled_at: None,
        provider_effect_id: None,
        error_code: None,
        reconciliation_note: None,
    };
    initial
        .apply(
            0,
            OrchestrationMutation::CreateAgentIntent {
                agent,
                effect,
                meta: OrchestrationMutationMetaV1 {
                    event_id: "event-old".into(),
                    at: AT.into(),
                    actor: OrchestrationActorV1 {
                        kind: OrchestrationActorKind::Host,
                        actor_id: "old-host".into(),
                    },
                    summary: "Reserved old launch".into(),
                },
            },
        )
        .expect("seed old effect");
    let store = Arc::new(Mutex::new(initial));
    let runtime: Arc<dyn OrchestrationRuntimePort> = Arc::new(FakeRuntime::succeeding());
    let dispatcher = OrchestrationHostDispatcher::new(
        store.clone(),
        runtime,
        Arc::new(UnavailableCoordination),
        "epoch-new".into(),
    )
    .with_clock(Arc::new(FixedClock));

    dispatcher.recover().await.expect("recovery");

    let snapshot = store.lock().expect("store").snapshot().expect("snapshot");
    assert_eq!(snapshot.runtime_epoch, "epoch-new");
    assert_eq!(snapshot.agents[0].state, AgentNodeState::Cancelled);
    assert_eq!(snapshot.effects[0].state, OrchestrationEffectState::Failed);
    assert_eq!(
        snapshot.effects[0].error_code.as_deref(),
        Some("HOST_STOPPED_BEFORE_DISPATCH")
    );
    assert_eq!(
        snapshot.events.last().expect("last event").kind,
        "host.runtime-epoch-rotated"
    );
}

#[tokio::test]
async fn stop_agent_accepts_the_camel_case_agent_id_the_app_sends() {
    let fixture = fixture(Arc::new(FakeCoordination));
    let cwd = fixture.temporary.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("workspace");
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_orchestration_agent,
            create_payload(&cwd, "create-stop"),
        )
        .await
        .expect("create agent");
    let agent_id = created["agentId"].as_str().expect("agent id");

    // The Agent Canvas Stop buttons send exactly this payload.
    let stopped = fixture
        .dispatcher
        .dispatch(
            CHANNELS.stop_orchestration_agent,
            json!({ "agentId": agent_id }),
        )
        .await
        .expect("camelCase stop request decodes and stops the agent");
    assert_eq!(stopped["agentId"], agent_id);
    assert_eq!(stopped["state"], "cancelled");
    assert_eq!(fixture.runtime.stops.load(AtomicOrdering::SeqCst), 1);
}
