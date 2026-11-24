use std::{
    collections::HashSet,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{
        EngineCapabilities, EngineEvent, EngineEventKind, EngineId, EngineInfo, EngineModels,
        EngineSessionListing, EngineUsage, LaunchOptions, ProviderCommand, ProviderCommandScope,
        ResumeOptions, SessionKind, SessionOrigin, SessionStatus, SessionSummary,
    },
    ipc::{EngineOp, HistoryPage, ProviderListingFreshness, SessionSearchMatchKind},
};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EffectCertainty,
    EngineAdapter, EngineError, EngineErrorKind, EventSink, ForkBoundary, HistoryRequest,
    ProviderCommandRequest, ProviderCommandResult, ReadySession, SessionHandle,
    bounded_event_stream,
};
use interface_registry::SessionRegistry;
use interface_runtime::{
    EngineHistoryStore, HistoryLimits, ImportProgress, RuntimeErrorKind, RuntimeSendLiveOutcome,
    RuntimeServices, SessionOpenOutcome, SessionRuntime,
};
use tempfile::TempDir;
use tokio::sync::Notify;

struct Harness {
    runtime: SessionRuntime,
    root: TempDir,
}

struct FakeControl {
    id: EngineId,
    listing: Mutex<EngineSessionListing>,
    listing_error: Mutex<Option<EngineError>>,
    listing_calls: AtomicUsize,
    listing_reuse: Mutex<Duration>,
    operations: Vec<EngineOp>,
    history: AtomicBool,
    history_page: Mutex<HistoryPage>,
    history_requests: Mutex<Vec<(String, HistoryRequest)>>,
    block_listing: AtomicBool,
    listing_started: AtomicBool,
    listing_released: AtomicBool,
    listing_notify: Notify,
    block_rename: AtomicBool,
    rename_started: AtomicBool,
    rename_released: AtomicBool,
    rename_notify: Notify,
    rename_error: AtomicBool,
    commands_available: AtomicBool,
    command_error: AtomicBool,
    block_commands: AtomicBool,
    command_started: AtomicBool,
    command_released: AtomicBool,
    command_notify: Notify,
    command_calls: AtomicUsize,
    command_in_flight: AtomicUsize,
    command_max_in_flight: AtomicUsize,
    command_scopes: Mutex<Vec<ProviderCommandRequest>>,
    live_handles: AtomicBool,
    rename_calls: AtomicUsize,
    delete_calls: AtomicUsize,
    fork_calls: AtomicUsize,
    /// `fork_session` poisons this registry lock after making its copy.
    poison_on_fork: Mutex<Option<Arc<Mutex<SessionRegistry>>>>,
    /// `fork_session_at` answers `forked-at` instead of the default refusal.
    forks_at_turns: AtomicBool,
    fork_at_calls: Mutex<Vec<(String, Option<String>, ForkBoundary)>>,
    archive_calls: AtomicUsize,
    adopt_calls: AtomicUsize,
    adopt_in_flight: AtomicUsize,
    adopt_max_in_flight: AtomicUsize,
    failed_adoptions: Mutex<HashSet<String>>,
}

impl FakeControl {
    fn new(id: &str, sessions: Vec<SessionSummary>, operations: Vec<EngineOp>) -> Arc<Self> {
        Arc::new(Self {
            id: EngineId::new(id).expect("fake engine"),
            listing: Mutex::new(EngineSessionListing {
                sessions,
                complete: true,
                unavailable: None,
                error: None,
            }),
            listing_error: Mutex::new(None),
            listing_calls: AtomicUsize::new(0),
            listing_reuse: Mutex::new(Duration::ZERO),
            operations,
            history: AtomicBool::new(false),
            history_page: Mutex::new(HistoryPage::empty()),
            history_requests: Mutex::new(Vec::new()),
            block_listing: AtomicBool::new(false),
            listing_started: AtomicBool::new(false),
            listing_released: AtomicBool::new(false),
            listing_notify: Notify::new(),
            block_rename: AtomicBool::new(false),
            rename_started: AtomicBool::new(false),
            rename_released: AtomicBool::new(false),
            rename_notify: Notify::new(),
            rename_error: AtomicBool::new(false),
            commands_available: AtomicBool::new(false),
            command_error: AtomicBool::new(false),
            block_commands: AtomicBool::new(false),
            command_started: AtomicBool::new(false),
            command_released: AtomicBool::new(false),
            command_notify: Notify::new(),
            command_calls: AtomicUsize::new(0),
            command_in_flight: AtomicUsize::new(0),
            command_max_in_flight: AtomicUsize::new(0),
            command_scopes: Mutex::new(Vec::new()),
            live_handles: AtomicBool::new(false),
            rename_calls: AtomicUsize::new(0),
            delete_calls: AtomicUsize::new(0),
            fork_calls: AtomicUsize::new(0),
            poison_on_fork: Mutex::new(None),
            forks_at_turns: AtomicBool::new(false),
            fork_at_calls: Mutex::new(Vec::new()),
            archive_calls: AtomicUsize::new(0),
            adopt_calls: AtomicUsize::new(0),
            adopt_in_flight: AtomicUsize::new(0),
            adopt_max_in_flight: AtomicUsize::new(0),
            failed_adoptions: Mutex::new(HashSet::new()),
        })
    }

    fn with_listing_error(self: Arc<Self>, error: EngineError) -> Arc<Self> {
        *self.listing_error.lock().expect("listing error") = Some(error);
        self
    }

    fn enable_history(&self, page: HistoryPage) {
        self.history.store(true, Ordering::Release);
        *self.history_page.lock().expect("history page") = page;
    }

    fn block_next_listing(&self) {
        self.listing_started.store(false, Ordering::Release);
        self.listing_released.store(false, Ordering::Release);
        self.block_listing.store(true, Ordering::Release);
    }

    async fn wait_listing_started(&self) {
        while !self.listing_started.load(Ordering::Acquire) {
            self.listing_notify.notified().await;
        }
    }

    fn release_listing(&self) {
        self.listing_released.store(true, Ordering::Release);
        self.listing_notify.notify_waiters();
    }

    fn block_next_rename(&self) {
        self.rename_started.store(false, Ordering::Release);
        self.rename_released.store(false, Ordering::Release);
        self.block_rename.store(true, Ordering::Release);
    }

    async fn wait_rename_started(&self) {
        while !self.rename_started.load(Ordering::Acquire) {
            self.rename_notify.notified().await;
        }
    }

    fn release_rename(&self) {
        self.rename_released.store(true, Ordering::Release);
        self.rename_notify.notify_waiters();
    }

    fn enable_commands(&self) {
        self.commands_available.store(true, Ordering::Release);
    }

    fn enable_live_handles(&self) {
        self.live_handles.store(true, Ordering::Release);
    }

    fn block_next_commands(&self) {
        self.command_started.store(false, Ordering::Release);
        self.command_released.store(false, Ordering::Release);
        self.block_commands.store(true, Ordering::Release);
    }

    async fn wait_commands_started(&self) {
        while !self.command_started.load(Ordering::Acquire) {
            self.command_notify.notified().await;
        }
    }

    fn release_commands(&self) {
        self.command_released.store(true, Ordering::Release);
        self.command_notify.notify_waiters();
    }
}

struct FakeAdapter {
    control: Arc<FakeControl>,
}

struct FakeLiveHandle {
    engine: EngineId,
    session_id: String,
    events: Option<BoundedEventStream<EngineEvent>>,
    sink: EventSink<EngineEvent>,
}

#[async_trait]
impl SessionHandle for FakeLiveHandle {
    fn engine(&self) -> &EngineId {
        &self.engine
    }

    fn session_id(&self) -> String {
        self.session_id.clone()
    }

    async fn ready(&self) -> Result<ReadySession, EngineError> {
        Ok(ReadySession {
            session_id: self.session_id.clone(),
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
        DeliveryOutcome::Confirmed(())
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }

    async fn close(&self) -> Result<(), EngineError> {
        self.sink.close();
        Ok(())
    }

    async fn respond_to_approval(
        &self,
        _approval_id: String,
        _verdict: ApprovalVerdict,
        _response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
}

fn fake_live_handle(engine: EngineId, session_id: String) -> Box<dyn SessionHandle> {
    let (sink, events) = bounded_event_stream(16);
    Box::new(FakeLiveHandle {
        engine,
        session_id,
        events: Some(events),
        sink,
    })
}

#[async_trait]
impl EngineAdapter for FakeAdapter {
    fn id(&self) -> &EngineId {
        &self.control.id
    }

    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        Ok(EngineInfo {
            auth: None,
            binary_path: "/fake/provider".into(),
            version: "fake-1".into(),
            login_identity: None,
            capabilities: EngineCapabilities {
                streaming: true,
                approvals: false,
                resume: true,
                fork: self.control.operations.contains(&EngineOp::ForkSession),
                cloud_sessions: false,
                models: EngineModels::Models(Vec::new()),
                effort_levels: Vec::new(),
                permission_modes: Vec::new(),
                consultation_safety: None,
                steer: None,
            },
            display_name: Some("Fake".into()),
            model_catalog: None,
        })
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        self.control.listing_calls.fetch_add(1, Ordering::AcqRel);
        if self.control.block_listing.swap(false, Ordering::AcqRel) {
            self.control.listing_started.store(true, Ordering::Release);
            self.control.listing_notify.notify_waiters();
            while !self.control.listing_released.load(Ordering::Acquire) {
                self.control.listing_notify.notified().await;
            }
        }
        if let Some(error) = self
            .control
            .listing_error
            .lock()
            .expect("listing error")
            .clone()
        {
            return Err(error);
        }
        Ok(self.control.listing.lock().expect("listing").clone())
    }

    fn listing_reuse(&self) -> Duration {
        *self.control.listing_reuse.lock().expect("listing reuse")
    }

    fn supported_ops(&self) -> Vec<EngineOp> {
        self.control.operations.clone()
    }

    fn supports_history(&self) -> bool {
        self.control.history.load(Ordering::Acquire)
    }

    async fn rename_session(
        &self,
        _session_id: String,
        _title: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control.rename_calls.fetch_add(1, Ordering::AcqRel);
        if self.control.block_rename.swap(false, Ordering::AcqRel) {
            self.control.rename_started.store(true, Ordering::Release);
            self.control.rename_notify.notify_waiters();
            while !self.control.rename_released.load(Ordering::Acquire) {
                self.control.rename_notify.notified().await;
            }
        }
        if self.control.rename_error.swap(false, Ordering::AcqRel) {
            return Err(EngineError::outcome_unknown(
                EngineErrorKind::Transport,
                "ENOENT: provider failed at '/private/provider/transcript.jsonl'",
                true,
            ));
        }
        Ok(())
    }

    async fn delete_session(
        &self,
        _session_id: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control.delete_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    async fn fork_session(
        &self,
        _session_id: String,
        _cwd: Option<String>,
    ) -> Result<String, EngineError> {
        self.control.fork_calls.fetch_add(1, Ordering::AcqRel);
        let poison = self.control.poison_on_fork.lock().expect("poison").take();
        if let Some(registry) = poison {
            let _ = std::thread::spawn(move || {
                let _held = registry.lock();
                panic!("poison the session registry after the provider fork");
            })
            .join();
        }
        Ok("forked".into())
    }

    async fn fork_session_at(
        &self,
        session_id: String,
        cwd: Option<String>,
        boundary: ForkBoundary,
    ) -> Result<String, EngineError> {
        if !self.control.forks_at_turns.load(Ordering::Acquire) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                "This provider cannot fork at an earlier turn.",
                false,
            ));
        }
        self.control
            .fork_at_calls
            .lock()
            .expect("fork-at calls")
            .push((session_id, cwd, boundary));
        Ok("forked-at".into())
    }

    async fn set_archived(
        &self,
        _session_id: String,
        _archived: bool,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control.archive_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    async fn adopt_session(
        &self,
        session_id: String,
        _adopt: bool,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control.adopt_calls.fetch_add(1, Ordering::AcqRel);
        let active = self.control.adopt_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.control
            .adopt_max_in_flight
            .fetch_max(active, Ordering::AcqRel);
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.control.adopt_in_flight.fetch_sub(1, Ordering::AcqRel);
        if self
            .control
            .failed_adoptions
            .lock()
            .expect("failed adoptions")
            .contains(&session_id)
        {
            return Err(EngineError::definitely_rejected(
                EngineErrorKind::Provider,
                "fake adoption refusal",
                false,
            ));
        }
        Ok(())
    }

    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        Ok(EngineUsage {
            engine: self.control.id.clone(),
            available: true,
            plan: Some("fake".into()),
            windows: Vec::new(),
            cost_usd: None,
            note: None,
            account: None,
            account_label: None,
        })
    }

    async fn list_provider_commands(
        &self,
        scope: ProviderCommandRequest,
    ) -> Result<ProviderCommandResult, EngineError> {
        self.control.command_calls.fetch_add(1, Ordering::AcqRel);
        self.control
            .command_scopes
            .lock()
            .expect("command scopes")
            .push(scope.clone());
        let active = self
            .control
            .command_in_flight
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.control
            .command_max_in_flight
            .fetch_max(active, Ordering::AcqRel);
        if self.control.block_commands.swap(false, Ordering::AcqRel) {
            self.control.command_started.store(true, Ordering::Release);
            self.control.command_notify.notify_waiters();
            while !self.control.command_released.load(Ordering::Acquire) {
                self.control.command_notify.notified().await;
            }
        }
        self.control
            .command_in_flight
            .fetch_sub(1, Ordering::AcqRel);
        if self.control.command_error.swap(false, Ordering::AcqRel) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Provider,
                "provider transport failed at /private/provider/socket",
                true,
            ));
        }
        if !self.control.commands_available.load(Ordering::Acquire) {
            return Ok(ProviderCommandResult::unavailable("not in this fake"));
        }
        let model = scope.model.as_deref().unwrap_or("default");
        Ok(ProviderCommandResult {
            available: true,
            commands: vec![ProviderCommand {
                name: format!("for-{model}"),
                description: format!("Command for {model}"),
                argument_hint: None,
                aliases: None,
            }],
            reason: None,
        })
    }

    async fn read_history(
        &self,
        session_id: String,
        request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        self.control
            .history_requests
            .lock()
            .expect("history requests")
            .push((session_id, request));
        Ok(self
            .control
            .history_page
            .lock()
            .expect("history page")
            .clone())
    }

    fn create_session(
        &self,
        _options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        if self.control.live_handles.load(Ordering::Acquire) {
            return Ok(fake_live_handle(self.control.id.clone(), "created".into()));
        }
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "fake does not create sessions",
            false,
        ))
    }

    fn resume_session(
        &self,
        session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        if self.control.live_handles.load(Ordering::Acquire) {
            return Ok(fake_live_handle(self.control.id.clone(), session_id));
        }
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "fake does not resume sessions",
            false,
        ))
    }
}

fn summary(engine: &str, session_id: &str) -> SessionSummary {
    SessionSummary {
        engine: EngineId::new(engine).expect("engine"),
        session_id: session_id.into(),
        title: format!("Session {session_id}"),
        cwd: "/tmp/project".into(),
        status: SessionStatus::Idle,
        cloud: false,
        model: Some("fake-model".into()),
        effort: Some(Some("medium".into())),
        last_activity: "2026-08-27T12:00:00.000Z".into(),
        first_seen: "2026-08-27T12:00:00.000Z".into(),
        archived: false,
        size: Some(10),
        title_from_summary: None,
        origin: Some(SessionOrigin::Store),
        project_root: Some("/tmp/project".into()),
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

fn private_tempdir() -> TempDir {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private tempdir");
    root
}

fn harness(controls: &[Arc<FakeControl>]) -> Harness {
    let root = private_tempdir();
    let registry = SessionRegistry::open(root.path()).expect("registry");
    let history = EngineHistoryStore::open(root.path(), HistoryLimits::default()).expect("history");
    let services = RuntimeServices::new(registry, history);
    let mut builder = SessionRuntime::builder(services);
    for control in controls {
        builder
            .register_adapter(FakeAdapter {
                control: Arc::clone(control),
            })
            .expect("fake adapter");
    }
    Harness {
        runtime: builder.build().expect("runtime"),
        root,
    }
}

#[tokio::test]
async fn provider_history_reads_the_pre_port_store_with_legacy_cwd_and_limit() {
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    let expected = HistoryPage {
        availability: None,
        reason: None,
        events: vec![EngineEvent {
            at: Some("2026-08-26T12:00:00.000Z".into()),
            event: EngineEventKind::MessageDelta {
                text: "provider-owned pre-port transcript".into(),
                agent_key: None,
            },
        }],
        messages: Some(1),
        older_messages: 7,
    };
    claude.enable_history(expected.clone());
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("populate legacy registry projection");

    let page = harness
        .runtime
        .read_provider_history("claude#one", 12)
        .await
        .expect("provider-owned history");
    assert_eq!(page, expected);
    let requests = claude.history_requests.lock().expect("history requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "one");
    assert_eq!(requests[0].1.cwd, "/tmp/project");
    assert_eq!(requests[0].1.limit, 500, "legacy lower clamp");
}

#[tokio::test]
async fn the_thread_journal_import_asks_for_the_whole_history_not_the_renderers_page() {
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    claude.enable_history(HistoryPage {
        availability: None,
        reason: None,
        events: Vec::new(),
        messages: Some(0),
        older_messages: 0,
    });
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("populate legacy registry projection");
    harness
        .runtime
        .read_provider_history("claude#one", 50_000)
        .await
        .expect("renderer page");
    harness
        .runtime
        .read_provider_history_for_import("claude#one")
        .await
        .expect("import page");
    let requests = claude.history_requests.lock().expect("history requests");
    let limits = requests
        .iter()
        .map(|request| request.1.limit)
        .collect::<Vec<_>>();
    // The renderer's page stays capped; the import reads 800 or 8,000 items whole.
    assert_eq!(limits, [5_000, 20_000]);
    assert_eq!(interface_runtime::IMPORT_HISTORY_LIMIT, 20_000);
}

#[tokio::test]
async fn provider_history_capability_and_missing_engine_are_distinct_safe_results() {
    let qwen = FakeControl::new("acp:qwen", vec![summary("acp:qwen", "one")], Vec::new());
    let harness = harness(&[qwen]);
    let unavailable = harness
        .runtime
        .read_provider_history("acp:qwen#one", 750)
        .await
        .expect("unsupported history is a normal result");
    assert_eq!(
        unavailable.availability,
        Some(interface_contracts::ipc::HistoryAvailability::Unavailable)
    );
    assert!(
        unavailable
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("does not expose"))
    );

    let error = harness
        .runtime
        .read_provider_history("codex#missing", 750)
        .await
        .expect_err("unregistered provider");
    assert_eq!(error.kind, RuntimeErrorKind::EngineNotRegistered);
    assert!(error.replay_is_safe());
    assert!(!error.message.contains('/'));
}

#[tokio::test]
async fn listing_fanout_merges_success_and_isolates_provider_failure() {
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    let codex = FakeControl::new("codex", Vec::new(), Vec::new()).with_listing_error(
        EngineError::definitely_unsent(EngineErrorKind::Unavailable, "codex down", true),
    );
    let harness = harness(&[claude, codex]);

    let snapshot = harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("snapshot");
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].session_id, "one");
    assert_eq!(snapshot.providers.len(), 2);
    assert_eq!(snapshot.providers[0].engine.as_str(), "claude");
    assert_eq!(
        snapshot.providers[0].status,
        ProviderListingFreshness::Fresh
    );
    assert_eq!(snapshot.providers[1].engine.as_str(), "codex");
    assert_eq!(
        snapshot.providers[1].status,
        ProviderListingFreshness::Stale
    );
    assert!(!snapshot.providers[1].complete);
}

// Paused time: the cache writer's 1 s debounce is virtual.
#[tokio::test(start_paused = true)]
async fn an_identical_listing_leaves_the_registry_cache_and_revisions_alone() {
    use std::os::unix::fs::MetadataExt;

    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    let harness = harness(&[Arc::clone(&claude)]);
    let registry = harness.runtime.services().session_registry();
    let state = || {
        let registry = registry.lock().expect("registry");
        (
            registry.revision(),
            registry.content_revision(),
            registry.cache_is_current(),
        )
    };
    let path = harness.root.path().join("registry.json");
    let file = || {
        let metadata = std::fs::metadata(&path).expect("registry cache");
        (metadata.ino(), metadata.modified().expect("cache mtime"))
    };

    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("first listing");
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    let (revision, content, current) = state();
    assert!(current, "the first listing reaches the cache");
    let written = file();

    // Every window focus lists again; nothing changed, so nothing moves.
    for _ in 0..3 {
        harness
            .runtime
            .list_session_snapshot()
            .await
            .expect("identical listing");
        tokio::time::sleep(Duration::from_millis(1_600)).await;
        assert_eq!(state(), (revision, content, true));
        assert_eq!(file(), written);
    }

    // A changed row still advances both revisions and is written.
    claude.listing.lock().expect("listing").sessions[0].last_activity =
        "2026-08-27T12:05:00.000Z".into();
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("changed listing");
    let (changed, changed_content, _) = state();
    assert_eq!((changed, changed_content), (revision + 1, content + 1));
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    assert!(state().2, "the changed listing reaches the cache");
    assert_ne!(file().0, written.0);
    harness.runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn a_kept_listing_is_skipped_until_it_ages_fails_or_an_explicit_refresh() {
    let codex = FakeControl::new("codex", vec![summary("codex", "one")], Vec::new());
    *codex.listing_reuse.lock().expect("listing reuse") = Duration::from_secs(60);
    let claude = FakeControl::new("claude", vec![summary("claude", "two")], Vec::new());
    let harness = harness(&[Arc::clone(&codex), Arc::clone(&claude)]);
    let engine = EngineId::new("codex").expect("engine");
    let calls = |control: &FakeControl| control.listing_calls.load(Ordering::Acquire);
    let codex_status = |snapshot: &interface_contracts::ipc::SessionListingSnapshot| {
        snapshot
            .providers
            .iter()
            .find(|status| status.engine == engine)
            .cloned()
            .expect("codex status")
    };
    let has = |snapshot: &interface_contracts::ipc::SessionListingSnapshot, id: &str| {
        snapshot.sessions.iter().any(|row| row.session_id == id)
    };

    let first = harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("first");
    assert_eq!((calls(&codex), calls(&claude)), (1, 1));

    // Inside the window Codex is not asked again: its rows and its status
    // (with the time it really listed) stand. Claude is asked every time.
    codex
        .listing
        .lock()
        .expect("listing")
        .sessions
        .push(summary("codex", "external"));
    tokio::time::advance(Duration::from_secs(59)).await;
    let kept = harness.runtime.list_session_snapshot().await.expect("kept");
    assert_eq!((calls(&codex), calls(&claude)), (1, 2));
    assert!(!has(&kept, "external"));
    assert_eq!(codex_status(&kept), codex_status(&first));

    // Once it has aged, Codex is asked again.
    tokio::time::advance(Duration::from_secs(1)).await;
    let aged = harness.runtime.list_session_snapshot().await.expect("aged");
    assert_eq!(calls(&codex), 2);
    assert!(has(&aged, "external"));

    // The explicit refresh restarts the window at once.
    harness.runtime.list_session_snapshot().await.expect("kept");
    assert_eq!(calls(&codex), 2);
    harness.runtime.probe_all_providers().await;
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("fresh");
    harness.runtime.list_session_snapshot().await.expect("kept");
    assert_eq!(calls(&codex), 3);

    // A failed listing is never kept, and it drops the one before it.
    *codex.listing_error.lock().expect("listing error") = Some(EngineError::definitely_unsent(
        EngineErrorKind::Unavailable,
        "codex down",
        true,
    ));
    let error = harness
        .runtime
        .refresh_provider_sessions(&engine)
        .await
        .expect_err("single refresh fails");
    assert_eq!(error.message, "codex down");
    let failed = harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("failed");
    assert_eq!(calls(&codex), 5);
    assert_eq!(
        codex_status(&failed).status,
        ProviderListingFreshness::Stale
    );
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("failed");
    assert_eq!(calls(&codex), 6);
    *codex.listing_error.lock().expect("listing error") = None;

    // Neither is an incomplete one.
    codex.listing.lock().expect("listing").complete = false;
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("incomplete");
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("incomplete");
    assert_eq!(calls(&codex), 8);

    // A complete single-provider refresh starts a window of its own.
    codex.listing.lock().expect("listing").complete = true;
    harness
        .runtime
        .refresh_provider_sessions(&engine)
        .await
        .expect("single refresh");
    harness.runtime.list_session_snapshot().await.expect("kept");
    assert_eq!(calls(&codex), 9);
}

#[tokio::test]
async fn a_refresh_asked_after_an_explicit_refresh_never_joins_an_earlier_one() {
    let codex = FakeControl::new("codex", vec![summary("codex", "one")], Vec::new());
    *codex.listing_reuse.lock().expect("listing reuse") = Duration::from_secs(60);
    let claude = FakeControl::new("claude", vec![summary("claude", "two")], Vec::new());
    let harness = harness(&[Arc::clone(&codex), Arc::clone(&claude)]);
    harness
        .runtime
        .list_session_snapshot()
        .await
        .expect("first");
    assert_eq!(codex.listing_calls.load(Ordering::Acquire), 1);

    // A focus refresh that kept Codex's listing is still running (Claude is
    // slow) when an explicit refresh comes, and a listing is asked after it.
    claude.block_next_listing();
    let running = {
        let runtime = harness.runtime.clone();
        tokio::spawn(async move { runtime.list_session_snapshot().await })
    };
    claude.wait_listing_started().await;
    harness.runtime.probe_all_providers().await;
    let asked = {
        let runtime = harness.runtime.clone();
        tokio::spawn(async move { runtime.list_session_snapshot().await })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    claude.release_listing();
    running
        .await
        .expect("running task")
        .expect("running refresh");
    asked.await.expect("asked task").expect("asked refresh");
    assert_eq!(
        codex.listing_calls.load(Ordering::Acquire),
        2,
        "the later refresh listed Codex itself"
    );
}

#[tokio::test]
async fn cold_listing_never_demotes_exact_live_waiting_status() {
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    claude.enable_live_handles();
    let engine = EngineId::new("claude").expect("engine");
    assert!(matches!(
        harness
            .runtime
            .resume_session(
                &engine,
                "one".into(),
                "/tmp/project".into(),
                ResumeOptions::default(),
            )
            .await,
        SessionOpenOutcome::Ready(_)
    ));
    let registry = harness.runtime.services().session_registry();
    {
        let mut registry = registry.lock().expect("registry");
        let revision = registry.revision();
        let revision = registry
            .apply_event(
                revision,
                "claude#one",
                &EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged {
                        status: SessionStatus::Waiting,
                    },
                },
                "2026-08-27T12:01:00.000Z",
            )
            .expect("waiting event");
        registry.save(revision).expect("save waiting state");
    }
    claude.listing.lock().expect("listing").sessions[0].status = SessionStatus::Working;

    let sessions = harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("fresh listing");
    assert_eq!(sessions[0].status, SessionStatus::Waiting);
}

#[tokio::test]
async fn command_discovery_coalesces_serializes_and_refuses_unsafe_scopes() {
    let claude = FakeControl::new("claude", Vec::new(), Vec::new());
    claude.enable_commands();
    let harness = harness(&[Arc::clone(&claude)]);
    let cwd = canonical_path(harness.root.path());
    let one = ProviderCommandScope {
        engine: EngineId::new("claude").expect("engine"),
        cwd: cwd.clone(),
        model: Some("one".into()),
        setting_sources: Some(vec!["project".into(), "project".into()]),
    };
    let mut two = one.clone();
    two.model = Some("two".into());
    claude.block_next_commands();
    let runtime = harness.runtime.clone();
    let first_scope = one.clone();
    let first = tokio::spawn(async move { runtime.list_provider_commands(first_scope).await });
    claude.wait_commands_started().await;
    // The coalescing lease is claimed synchronously on a request's first
    // poll, before the blocking folder check, so one poll is enough to make
    // the duplicate a waiter on the running discovery; no clock involved.
    let mut duplicate = Box::pin(harness.runtime.list_provider_commands(one.clone()));
    let joined = std::future::poll_fn(|context| {
        std::task::Poll::Ready(duplicate.as_mut().poll(context).is_pending())
    })
    .await;
    assert!(
        joined,
        "an identical request waits for the running discovery"
    );
    let runtime = harness.runtime.clone();
    let second = tokio::spawn(async move { runtime.list_provider_commands(two).await });
    // However far the second scope gets, the running discovery holds the
    // per-engine lock, so nothing else reaches the provider before the release.
    tokio::task::yield_now().await;
    assert_eq!(claude.command_calls.load(Ordering::Acquire), 1);
    assert_eq!(claude.command_max_in_flight.load(Ordering::Acquire), 1);
    claude.release_commands();

    let first = first.await.expect("first task").expect("first commands");
    let duplicate = duplicate.await.expect("duplicate commands");
    let second = second.await.expect("second task").expect("second commands");
    assert_eq!(first, duplicate);
    assert_eq!(first.scope.setting_sources, Some(vec!["project".into()]));
    assert_eq!(second.scope.model.as_deref(), Some("two"));
    assert_eq!(claude.command_calls.load(Ordering::Acquire), 2);
    assert_eq!(claude.command_max_in_flight.load(Ordering::Acquire), 1);

    let exact_model = "😀".repeat(128);
    let mut exact_utf16_model = one.clone();
    exact_utf16_model.model = Some(exact_model.clone());
    let exact = harness
        .runtime
        .list_provider_commands(exact_utf16_model)
        .await
        .expect("256 UTF-16 code units are accepted");
    assert_eq!(exact.scope.model.as_deref(), Some(exact_model.as_str()));

    let calls_before_refusal = claude.command_calls.load(Ordering::Acquire);
    let mut oversized_utf16_model = one.clone();
    oversized_utf16_model.model = Some("😀".repeat(129));
    let error = harness
        .runtime
        .list_provider_commands(oversized_utf16_model)
        .await
        .expect_err("258 UTF-16 code units must be refused");
    assert_eq!(error.kind, RuntimeErrorKind::InvalidInput);
    let mut invalid_model = one.clone();
    invalid_model.model = Some(" one".into());
    let error = harness
        .runtime
        .list_provider_commands(invalid_model)
        .await
        .expect_err("invalid model must be refused");
    assert_eq!(error.kind, RuntimeErrorKind::InvalidInput);
    let linked = std::path::PathBuf::from(&cwd).join("linked-project");
    std::os::unix::fs::symlink(&cwd, &linked).expect("test symlink");
    let mut unsafe_scope = one.clone();
    unsafe_scope.cwd = linked.to_string_lossy().into_owned();
    let error = harness
        .runtime
        .list_provider_commands(unsafe_scope)
        .await
        .expect_err("symbolic root must be refused");
    assert_eq!(error.kind, RuntimeErrorKind::InvalidInput);
    assert_eq!(
        claude.command_calls.load(Ordering::Acquire),
        calls_before_refusal
    );

    claude.command_error.store(true, Ordering::Release);
    let mut failed_scope = one;
    failed_scope.model = Some("failure".into());
    let unavailable = harness
        .runtime
        .list_provider_commands(failed_scope)
        .await
        .expect("provider failure is unavailable metadata");
    assert!(!unavailable.available);
    assert_eq!(
        unavailable.reason.as_deref(),
        Some("Interface could not read commands from this provider for the selected launch.")
    );
    assert!(!unavailable.reason.unwrap().contains("/private/"));
}

#[tokio::test]
async fn session_search_limit_matches_javascript_utf16_code_units() {
    let harness = harness(&[]);
    let accepted = harness
        .runtime
        .search_sessions("😀".repeat(128))
        .await
        .expect("256 UTF-16 code units are accepted");
    assert!(accepted.hits.is_empty());

    let error = harness
        .runtime
        .search_sessions("😀".repeat(129))
        .await
        .expect_err("258 UTF-16 code units must be refused");
    assert_eq!(error.kind, RuntimeErrorKind::InvalidInput);
}

#[tokio::test]
async fn provider_confirmed_rename_survives_registry_cache_save_failure() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::RenameSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    std::fs::write(harness.root.path().join("registry.json"), b"{}").expect("external drift");

    harness
        .runtime
        .rename_stored_session("claude#one", "Changed".into())
        .await
        .expect("provider-confirmed rename must not depend on cache persistence");
    assert_eq!(claude.rename_calls.load(Ordering::Acquire), 1);
    let sessions = harness.runtime.services().session_registry();
    let title = sessions
        .lock()
        .expect("registry")
        .get("claude#one")
        .expect("row retained")
        .title
        .clone();
    assert_eq!(title, "Changed");
    harness
        .runtime
        .shutdown()
        .await
        .expect("cache failure cannot fail runtime shutdown");
}

#[tokio::test]
async fn provider_operation_errors_preserve_certainty_without_exposing_paths() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::RenameSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    claude.rename_error.store(true, Ordering::Release);

    let error = harness
        .runtime
        .rename_stored_session("claude#one", "Changed".into())
        .await
        .expect_err("unknown provider outcome");
    assert_eq!(error.kind, RuntimeErrorKind::Provider);
    assert_eq!(error.certainty, EffectCertainty::OutcomeUnknown);
    assert!(!error.replay_is_safe());
    assert!(!error.message.contains("/private/"));
    assert!(error.message.contains("do not blindly replay"));
    let registry = harness.runtime.services().session_registry();
    assert_eq!(
        registry
            .lock()
            .expect("registry")
            .get("claude#one")
            .expect("row retained")
            .title,
        "Session one"
    );
}

#[tokio::test]
async fn lifecycle_race_refuses_second_effect_before_provider_contact() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::RenameSession, EngineOp::DeleteSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    claude.block_next_rename();
    let runtime = harness.runtime.clone();
    let rename = tokio::spawn(async move {
        runtime
            .rename_stored_session("claude#one", "Changed".into())
            .await
    });
    claude.wait_rename_started().await;
    let runtime = harness.runtime.clone();
    let duplicate = tokio::spawn(async move {
        runtime
            .rename_stored_session("claude#one", "Changed".into())
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(claude.rename_calls.load(Ordering::Acquire), 1);

    let error = harness
        .runtime
        .rename_stored_session("claude#one", "Other".into())
        .await
        .expect_err("different rename must not coalesce");
    assert_eq!(error.kind, RuntimeErrorKind::Lifecycle);

    let error = harness
        .runtime
        .delete_stored_session("claude#one")
        .await
        .expect_err("delete must not overlap rename");
    assert_eq!(error.kind, RuntimeErrorKind::Lifecycle);
    assert_eq!(claude.delete_calls.load(Ordering::Acquire), 0);
    claude.release_rename();
    rename.await.expect("rename task").expect("rename");
    duplicate
        .await
        .expect("duplicate task")
        .expect("coalesced rename");
    assert_eq!(claude.rename_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn slow_predelete_listing_cannot_resurrect_deleted_row() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::DeleteSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    claude.block_next_listing();
    let runtime = harness.runtime.clone();
    let listing = tokio::spawn(async move { runtime.list_managed_sessions().await });
    claude.wait_listing_started().await;
    harness
        .runtime
        .delete_stored_session("claude#one")
        .await
        .expect("delete");
    claude.release_listing();

    assert!(
        listing
            .await
            .expect("listing task")
            .expect("listing")
            .is_empty()
    );
    assert!(
        harness
            .runtime
            .services()
            .session_registry()
            .lock()
            .expect("registry")
            .all()
            .is_empty()
    );
}

#[tokio::test]
async fn slow_single_provider_refresh_cannot_resurrect_deleted_row() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::DeleteSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    claude.block_next_listing();
    let runtime = harness.runtime.clone();
    let refresh = tokio::spawn(async move {
        runtime
            .refresh_provider_sessions(&EngineId::new("claude").expect("engine"))
            .await
    });
    claude.wait_listing_started().await;
    harness
        .runtime
        .delete_stored_session("claude#one")
        .await
        .expect("delete");
    claude.release_listing();

    let listing = refresh.await.expect("refresh task").expect("refresh");
    assert_eq!(
        listing.sessions.len(),
        1,
        "the provider's answer is returned"
    );
    assert!(
        harness
            .runtime
            .services()
            .session_registry()
            .lock()
            .expect("registry")
            .all()
            .is_empty()
    );
}

#[tokio::test]
async fn adopt_all_is_bounded_partial_and_reports_every_progress_edge() {
    let sessions = (0..7)
        .map(|index| summary("claude", &format!("s{index}")))
        .collect::<Vec<_>>();
    let claude = FakeControl::new("claude", sessions, vec![EngineOp::AdoptSession]);
    claude
        .failed_adoptions
        .lock()
        .expect("failed adoptions")
        .insert("s3".into());
    let codex = FakeControl::new("codex", vec![summary("codex", "local")], Vec::new());
    let harness = harness(&[Arc::clone(&claude), codex]);
    let progress = Arc::new(Mutex::new(Vec::<ImportProgress>::new()));
    let captured = Arc::clone(&progress);
    let sink: Arc<dyn interface_runtime::ImportProgressSink> =
        Arc::new(move |event| captured.lock().expect("progress").push(event));

    let report = harness
        .runtime
        .adopt_all(true, Some(sink))
        .await
        .expect("import report");
    assert_eq!(report.total, 8);
    assert_eq!(report.ok, 6);
    assert_eq!(report.local, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(claude.adopt_max_in_flight.load(Ordering::Acquire) <= 4);
    let progress = progress.lock().expect("progress");
    assert_eq!(progress.len(), 9);
    assert_eq!(progress[0].done, 0);
    assert_eq!(progress.last().expect("last progress").done, 8);
}

#[tokio::test]
async fn send_live_never_cold_resumes_a_missing_attachment() {
    let harness = harness(&[FakeControl::new("claude", Vec::new(), Vec::new())]);
    assert!(matches!(
        harness
            .runtime
            .send_live("claude#missing", "hello".into())
            .await,
        RuntimeSendLiveOutcome::NotLive
    ));
}

#[tokio::test]
async fn search_uses_explicit_history_capability_and_maps_replay_events() {
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], Vec::new());
    claude.enable_history(searchable_history_fixture());
    let codex = FakeControl::new("codex", vec![summary("codex", "two")], Vec::new());
    let harness = harness(&[claude, codex]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");

    let search = harness
        .runtime
        .search_sessions("provider boundary".into())
        .await
        .expect("search");
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].key, "claude#one");
    assert_eq!(search.hits[0].matches.len(), 2);
    assert_eq!(search.hits[0].matches[0].kind, SessionSearchMatchKind::User);
    assert_eq!(search.hits[0].matches[1].kind, SessionSearchMatchKind::Tool);
    let codex = search
        .providers
        .iter()
        .find(|notice| notice.engine.as_str() == "codex")
        .expect("codex notice");
    assert!(!codex.complete);
    assert!(codex.error.is_some());
}

#[tokio::test]
async fn remaining_management_family_preserves_identity_and_capability_semantics() {
    let operations = vec![
        EngineOp::RenameSession,
        EngineOp::DeleteSession,
        EngineOp::ForkSession,
        EngineOp::SetArchived,
        EngineOp::AdoptSession,
        EngineOp::ReadUsage,
    ];
    let claude = FakeControl::new("claude", vec![summary("claude", "one")], operations.clone());
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");

    assert_eq!(harness.runtime.engine_ops()[0].ops, operations);
    let usage = harness.runtime.read_all_usage().await;
    assert_eq!(usage.len(), 1);
    assert!(usage[0].available);
    assert_eq!(usage[0].engine.as_str(), "claude");
    let commands = harness
        .runtime
        .list_provider_commands(ProviderCommandScope {
            engine: EngineId::new("claude").expect("engine"),
            cwd: canonical_path(harness.root.path()),
            model: Some("fake-model".into()),
            setting_sources: None,
        })
        .await
        .expect("commands");
    assert!(!commands.available);
    assert!(commands.commands.is_empty());

    harness
        .runtime
        .archive_session("claude#one", true)
        .await
        .expect("local archive");
    assert_eq!(
        harness
            .runtime
            .list_archived_sessions()
            .await
            .unwrap()
            .len(),
        1
    );
    harness
        .runtime
        .archive_session("claude#one", false)
        .await
        .expect("local restore");
    harness
        .runtime
        .set_engine_archived("claude#one", true)
        .await
        .expect("provider archive");
    harness
        .runtime
        .set_engine_archived("claude#one", false)
        .await
        .expect("provider restore");
    assert_eq!(claude.archive_calls.load(Ordering::Acquire), 2);

    harness
        .runtime
        .adopt_stored_session("claude#one", true)
        .await
        .expect("adopt");
    let fork_key = harness
        .runtime
        .fork_stored_session("claude#one")
        .await
        .expect("fork");
    assert_eq!(fork_key, "claude#forked");
    assert_eq!(claude.fork_calls.load(Ordering::Acquire), 1);
    let registry = harness.runtime.services().session_registry();
    let fork = registry
        .lock()
        .expect("registry")
        .get(&fork_key)
        .expect("fork projection")
        .clone();
    assert_eq!(fork.session_id, "forked");
    assert_eq!(fork.cwd, "/tmp/project");
    assert_eq!(fork.model.as_deref(), Some("fake-model"));
    assert_eq!(
        fork.effort.as_ref().and_then(Clone::clone).as_deref(),
        Some("medium")
    );
    assert_eq!(fork.origin, Some(SessionOrigin::App));
    assert!(!fork.archived);

    harness
        .runtime
        .forget_session(&fork_key)
        .await
        .expect("forget fork");
    let registry = harness.runtime.services().session_registry();
    let registry = registry.lock().expect("registry");
    assert!(registry.get(&fork_key).is_none());
    assert!(registry.is_forgotten(&fork_key));
}

#[tokio::test]
async fn fork_keeps_the_source_session_kind() {
    let mut source = summary("claude", "one");
    source.kind = Some(SessionKind::Chat);
    let claude = FakeControl::new("claude", vec![source], vec![EngineOp::ForkSession]);
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    let fork_key = harness
        .runtime
        .fork_stored_session("claude#one")
        .await
        .expect("fork");
    let registry = harness.runtime.services().session_registry();
    let fork = registry
        .lock()
        .expect("registry")
        .get(&fork_key)
        .expect("fork projection")
        .clone();
    assert_eq!(fork.kind, Some(SessionKind::Chat));
    assert_eq!(fork.session_id, "forked");
}

#[tokio::test]
async fn a_registry_failure_after_the_provider_fork_is_outcome_unknown() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::ForkSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    *claude.poison_on_fork.lock().expect("poison") =
        Some(harness.runtime.services().session_registry());

    let error = harness
        .runtime
        .fork_stored_session("claude#one")
        .await
        .expect_err("the copy exists but cannot be registered");
    assert_eq!(claude.fork_calls.load(Ordering::Acquire), 1);
    assert_eq!(error.certainty, EffectCertainty::OutcomeUnknown);
    assert!(!error.replay_is_safe());
}

#[tokio::test]
async fn fork_at_a_turn_refuses_definitely_unsent_when_the_provider_cannot() {
    let claude = FakeControl::new(
        "claude",
        vec![summary("claude", "one")],
        vec![EngineOp::ForkSession],
    );
    let harness = harness(&[Arc::clone(&claude)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    let engine: EngineId = "claude".parse().expect("engine id");
    let error = harness
        .runtime
        .fork_session_at(&engine, "one", kept("one", 0))
        .await
        .expect_err("the fake provider cannot fork at a turn");
    assert_eq!(error.kind, RuntimeErrorKind::Provider);
    assert_eq!(error.certainty, EffectCertainty::DefinitelyUnsent);
    assert!(!error.retryable);
    assert!(
        error.message.contains("cannot fork at an earlier turn"),
        "{}",
        error.message
    );
    assert_eq!(claude.fork_calls.load(Ordering::Acquire), 0);
    let registry = harness.runtime.services().session_registry();
    assert!(
        registry
            .lock()
            .expect("registry")
            .get("claude#forked")
            .is_none()
    );
}

#[tokio::test]
async fn fork_at_a_turn_registers_the_copy_in_a_thread_worktree() {
    let mut source = summary("codex", "one");
    source.kind = Some(SessionKind::Code);
    source.cwd = "/tmp/.interface-worktrees/fix-login-a1b2c3".into();
    let codex = FakeControl::new("codex", vec![source], vec![EngineOp::ForkSession]);
    codex.forks_at_turns.store(true, Ordering::Release);
    let harness = harness(&[Arc::clone(&codex)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    let engine: EngineId = "codex".parse().expect("engine id");
    let fork_key = harness
        .runtime
        .fork_session_at(&engine, "one", kept("two", 1))
        .await
        .expect("fork at a turn");
    assert_eq!(fork_key, "codex#forked-at");
    assert_eq!(
        *codex.fork_at_calls.lock().expect("fork-at calls"),
        [(
            "one".to_owned(),
            Some("/tmp/.interface-worktrees/fix-login-a1b2c3".to_owned()),
            kept("two", 1)
        )]
    );
    assert_eq!(codex.fork_calls.load(Ordering::Acquire), 0);
    let registry = harness.runtime.services().session_registry();
    let fork = registry
        .lock()
        .expect("registry")
        .get(&fork_key)
        .expect("fork projection")
        .clone();
    assert_eq!(fork.session_id, "forked-at");
    assert_eq!(fork.cwd, "/tmp/.interface-worktrees/fix-login-a1b2c3");
    assert_eq!(fork.kind, Some(SessionKind::Code));
    assert_eq!(fork.model.as_deref(), Some("fake-model"));
    assert_eq!(fork.origin, Some(SessionOrigin::App));
    assert_eq!(fork.worktree_home, None);
    assert!(!fork.archived);
}

#[tokio::test]
async fn fork_at_a_turn_refuses_a_bad_boundary_and_managed_isolation_before_the_provider() {
    let mut isolated = summary("codex", "isolated");
    isolated.cwd = "/tmp/.interface-worktrees/managed-s0/app".into();
    isolated.worktree_home = Some("/tmp/project".into());
    let codex = FakeControl::new(
        "codex",
        vec![summary("codex", "one"), isolated],
        vec![EngineOp::ForkSession],
    );
    codex.forks_at_turns.store(true, Ordering::Release);
    let harness = harness(&[Arc::clone(&codex)]);
    harness
        .runtime
        .list_managed_sessions()
        .await
        .expect("initial listing");
    let engine: EngineId = "codex".parse().expect("engine id");

    let unnamed = harness
        .runtime
        .fork_session_at(
            &engine,
            "one",
            ForkBoundary {
                prompt_digest: "not a digest".into(),
                later_same_digest: 0,
            },
        )
        .await
        .expect_err("a boundary that names no prompt is refused");
    assert_eq!(unnamed.kind, RuntimeErrorKind::InvalidInput);
    assert_eq!(unnamed.certainty, EffectCertainty::DefinitelyUnsent);
    assert!(
        unnamed.message.contains("by its digest"),
        "{}",
        unnamed.message
    );

    let registry = harness.runtime.services().session_registry();
    assert_eq!(
        registry
            .lock()
            .expect("registry")
            .get("codex#isolated")
            .and_then(|summary| summary.worktree_home.clone())
            .as_deref(),
        Some("/tmp/project")
    );
    let managed = harness
        .runtime
        .fork_session_at(&engine, "isolated", kept("one", 0))
        .await
        .expect_err("a managed worktree session must finish isolation first");
    assert_eq!(managed.kind, RuntimeErrorKind::Lifecycle);
    assert_eq!(managed.certainty, EffectCertainty::DefinitelyUnsent);
    assert!(
        managed.message.contains("Finish isolation"),
        "{}",
        managed.message
    );
    assert!(
        codex
            .fork_at_calls
            .lock()
            .expect("fork-at calls")
            .is_empty()
    );
}

fn kept(prompt: &str, later_same_digest: u32) -> ForkBoundary {
    ForkBoundary {
        prompt_digest: interface_engine::prompt_digest(prompt),
        later_same_digest,
    }
}

fn canonical_path(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .expect("canonical test path")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn bridge_management_commands_keep_camel_case_transport_shape() {
    let value = serde_json::to_value(interface_runtime::RuntimeBridgeCommand::SetEngineArchived {
        session_key: "codex#thread".into(),
        archived: true,
    })
    .expect("serialize command");
    assert_eq!(
        value,
        serde_json::json!({
            "method": "setEngineArchived",
            "params": { "session_key": "codex#thread", "archived": true }
        })
    );
}

fn searchable_history_fixture() -> HistoryPage {
    HistoryPage {
        availability: None,
        reason: None,
        events: vec![
            EngineEvent {
                at: None,
                event: EngineEventKind::UserMessage {
                    text: "Find the frozen provider boundary".into(),
                    agent_key: None,
                    images: None,
                    image_references: None,
                },
            },
            EngineEvent {
                at: None,
                event: EngineEventKind::ToolResult {
                    call_id: "call".into(),
                    ok: true,
                    summary: "provider boundary located".into(),
                    agent_key: None,
                },
            },
        ],
        messages: Some(1),
        older_messages: 0,
    }
}
