use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, EngineId, LaunchOptions, ResumeOptions, SessionKind,
        SessionStatus,
    },
    ipc::{ALL_CHANNELS, EXTRA_CHANNELS, as_history_page, unavailable_history},
};
use interface_engine::bounded_event_stream;
use interface_runtime::{
    HistoryEvent, HistorySession, RuntimeDeliveryAudit, RuntimeDeliveryDisposition,
    RuntimeDeliveryReceipt, RuntimeDeliveryState, RuntimeEvent, RuntimeSession, RuntimeStreamState,
};
use serde_json::json;

use super::*;

struct FakeRuntime {
    commands: Mutex<Vec<RuntimeBridgeCommand>>,
    responses: Mutex<VecDeque<RuntimeBridgeResponse>>,
    summaries: Mutex<HashMap<String, SessionSummary>>,
    event_streams: Mutex<HashMap<String, BoundedEventStream<RuntimeEventFrame>>>,
    inactive: Mutex<HashSet<String>>,
    projected: Mutex<Vec<(String, EngineEvent)>>,
    send_must_start_after: Option<Arc<AtomicBool>>,
    event_stream_failure: bool,
    summary_read_failure: bool,
    associate_worktree_failure: bool,
    chat_home: Option<PathBuf>,
    /// The registry content revision; `None` is unknown.
    revision: Mutex<Option<u64>>,
    /// A `ListSessions` finds a new session (bumps `revision`).
    listing_finds: AtomicBool,
}

#[derive(Default)]
struct FakeCollaborationHooks {
    reject_mutations: bool,
    guards: Mutex<Vec<(String, String, bool)>>,
    started: Arc<AtomicBool>,
    starts: Mutex<Vec<String>>,
    settlements: Mutex<Vec<(String, String, ProjectedSendOutcome)>>,
    events: Mutex<Vec<(String, EngineEvent)>>,
    registry_changes: Mutex<Vec<String>>,
}

struct FakeWorktrees {
    prepared: Worktree,
    persisted: Mutex<Vec<(String, Worktree)>>,
    discarded: Mutex<Vec<Worktree>>,
    persist_failure: bool,
}

#[derive(Default)]
struct FakeAutomaticWorktreeRelease {
    scheduled: Mutex<Vec<String>>,
}

impl AutomaticWorktreeRelease for FakeAutomaticWorktreeRelease {
    fn schedule_session_release(&self, session_key: String) {
        self.scheduled
            .lock()
            .expect("automatic worktree release lock")
            .push(session_key);
    }
}

#[async_trait]
impl WorktreeService for FakeWorktrees {
    async fn create(
        &self,
        _cwd: String,
        _name: Option<String>,
    ) -> Result<Worktree, crate::worktree::WorktreeServiceError> {
        Ok(self.prepared.clone())
    }

    async fn create_initialized(
        &self,
        cwd: String,
        name: Option<String>,
    ) -> Result<Worktree, crate::worktree::WorktreeServiceError> {
        self.create(cwd, name).await
    }

    async fn persist(
        &self,
        key: &str,
        worktree: &Worktree,
    ) -> Result<(), crate::worktree::WorktreeServiceError> {
        if self.persist_failure {
            return Err(crate::worktree::WorktreeServiceError::new(
                "injected managed-worktree persistence failure",
            ));
        }
        self.persisted
            .lock()
            .expect("fake persisted worktrees")
            .push((key.to_owned(), worktree.clone()));
        Ok(())
    }

    async fn discard(&self, worktree: Worktree) -> Option<String> {
        self.discarded
            .lock()
            .expect("fake discarded worktrees")
            .push(worktree);
        None
    }
}

impl FakeRuntime {
    fn new(response: RuntimeBridgeResponse) -> Self {
        Self::scripted([response])
    }

    fn scripted(responses: impl IntoIterator<Item = RuntimeBridgeResponse>) -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().collect()),
            summaries: Mutex::new(HashMap::new()),
            event_streams: Mutex::new(HashMap::new()),
            inactive: Mutex::new(HashSet::new()),
            projected: Mutex::new(Vec::new()),
            send_must_start_after: None,
            event_stream_failure: false,
            summary_read_failure: false,
            associate_worktree_failure: false,
            chat_home: None,
            revision: Mutex::new(None),
            listing_finds: AtomicBool::new(false),
        }
    }

    fn with_send_start_fence(mut self, started: Arc<AtomicBool>) -> Self {
        self.send_must_start_after = Some(started);
        self
    }

    fn with_projection_failures(mut self) -> Self {
        self.event_stream_failure = true;
        self.summary_read_failure = true;
        self.associate_worktree_failure = true;
        self
    }

    fn with_summary(self, key: &str, summary: SessionSummary) -> Self {
        self.summaries
            .lock()
            .expect("fake summaries lock")
            .insert(key.to_owned(), summary);
        self
    }

    fn with_inactive(self, key: &str) -> Self {
        self.inactive
            .lock()
            .expect("fake inactive lock")
            .insert(key.to_owned());
        self
    }

    fn with_chat_home(mut self, path: PathBuf) -> Self {
        self.chat_home = Some(path);
        self
    }

    fn with_event_stream(self, key: &str, events: BoundedEventStream<RuntimeEventFrame>) -> Self {
        self.event_streams
            .lock()
            .expect("fake event streams lock")
            .insert(key.to_owned(), events);
        self
    }

    fn commands(&self) -> Vec<RuntimeBridgeCommand> {
        self.commands.lock().expect("fake command lock").clone()
    }

    fn projected(&self) -> Vec<(String, EngineEvent)> {
        self.projected.lock().expect("fake projected lock").clone()
    }
}

#[async_trait]
impl CollaborationSessionHooks for FakeCollaborationHooks {
    async fn guard_session_mutation(
        &self,
        session_key: &str,
        action: &str,
        allow_safe_stop: bool,
    ) -> Result<(), DispatchError> {
        self.guards
            .lock()
            .expect("fake collaboration guards")
            .push((session_key.to_owned(), action.to_owned(), allow_safe_stop));
        if self.reject_mutations {
            return Err(DispatchError::new(
                DEFINITELY_UNSENT_CODE,
                "The collaboration owns this session; no provider action was taken.",
                false,
                true,
            ));
        }
        Ok(())
    }

    async fn guard_bulk_mutation(&self, _action: &str) -> Result<(), DispatchError> {
        Ok(())
    }

    fn user_send_started(&self, session_key: &str) -> Result<(), DispatchError> {
        self.starts
            .lock()
            .expect("fake collaboration starts")
            .push(session_key.to_owned());
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), DispatchError> {
        self.settlements
            .lock()
            .expect("fake collaboration settlements")
            .push((session_key.to_owned(), text.to_owned(), outcome));
        Ok(())
    }

    fn observe_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), DispatchError> {
        self.events
            .lock()
            .expect("fake collaboration events")
            .push((session_key.to_owned(), event.clone()));
        Ok(())
    }

    fn observe_registry_change(&self, session_key: &str) -> Result<(), DispatchError> {
        self.registry_changes
            .lock()
            .expect("fake collaboration registry changes")
            .push(session_key.to_owned());
        Ok(())
    }
}

#[async_trait]
impl RuntimeCommandHandler for FakeRuntime {
    async fn handle_runtime_command(&self, command: RuntimeBridgeCommand) -> RuntimeBridgeResponse {
        if matches!(command, RuntimeBridgeCommand::Send { .. })
            && let Some(started) = &self.send_must_start_after
        {
            assert!(
                started.load(Ordering::SeqCst),
                "collaboration send projection must start before provider contact"
            );
        }
        if matches!(command, RuntimeBridgeCommand::ListSessions)
            && self.listing_finds.load(Ordering::SeqCst)
            && let Some(revision) = self.revision.lock().expect("fake revision lock").as_mut()
        {
            *revision += 1;
        }
        self.commands
            .lock()
            .expect("fake command lock")
            .push(command);
        self.responses
            .lock()
            .expect("fake responses lock")
            .pop_front()
            .expect("scripted fake response")
    }

    async fn read_provider_history(
        &self,
        session_key: &str,
        limit: u64,
    ) -> Result<HistoryPage, RuntimeError> {
        match self
            .handle_runtime_command(RuntimeBridgeCommand::HistorySession {
                session_key: session_key.to_owned(),
            })
            .await
        {
            RuntimeBridgeResponse::HistorySession(history) => Ok(history.map_or_else(
                || unavailable_history("No durable Rust history is available for this session."),
                |history| {
                    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
                    let skipped = history.events.len().saturating_sub(limit);
                    let mut page = as_history_page(
                        history
                            .events
                            .into_iter()
                            .skip(skipped)
                            .map(|event| event.event)
                            .collect(),
                    );
                    page.older_messages = history
                        .omitted_events_through
                        .unwrap_or(0)
                        .saturating_add(u64::try_from(skipped).unwrap_or(u64::MAX));
                    page
                },
            )),
            RuntimeBridgeResponse::Error(error) => Err(error),
            _ => Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Internal,
                "fake provider history response mismatch",
                false,
            )),
        }
    }

    fn session_summary(&self, session_key: &str) -> Result<Option<SessionSummary>, RuntimeError> {
        Ok(self
            .summaries
            .lock()
            .expect("fake summaries lock")
            .get(session_key)
            .cloned())
    }

    fn session_summaries(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        if self.summary_read_failure {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Registry,
                "injected registry snapshot failure",
                false,
            ));
        }
        Ok(self
            .summaries
            .lock()
            .expect("fake summaries lock")
            .values()
            .cloned()
            .collect())
    }

    fn registry_content_revision(&self) -> Option<u64> {
        *self.revision.lock().expect("fake revision lock")
    }

    fn session_is_active(&self, session_key: &str) -> Result<bool, RuntimeError> {
        Ok(!self
            .inactive
            .lock()
            .expect("fake inactive lock")
            .contains(session_key))
    }

    fn project_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), RuntimeError> {
        self.projected
            .lock()
            .expect("fake projected lock")
            .push((session_key.to_owned(), event.clone()));
        if let EngineEventKind::StatusChanged { status } = event.event
            && let Some(summary) = self
                .summaries
                .lock()
                .expect("fake summaries lock")
                .get_mut(session_key)
        {
            summary.status = status;
        }
        Ok(())
    }

    fn associate_worktree(
        &self,
        session_key: &str,
        worktree: &Worktree,
    ) -> Result<(), RuntimeError> {
        if self.associate_worktree_failure {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Registry,
                "injected worktree projection failure",
                false,
            ));
        }
        if let Some(summary) = self
            .summaries
            .lock()
            .expect("fake summaries lock")
            .get_mut(session_key)
        {
            summary.worktree_home = Some(worktree.source_cwd.to_string_lossy().into_owned());
            summary.worktree_path = Some(worktree.session_cwd.to_string_lossy().into_owned());
            summary.project_root = Some(worktree.repo_root.to_string_lossy().into_owned());
        }
        Ok(())
    }

    async fn take_runtime_events(
        &self,
        session_key: &str,
    ) -> Result<BoundedEventStream<RuntimeEventFrame>, RuntimeError> {
        if self.event_stream_failure {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Lifecycle,
                "injected event projection failure",
                false,
            ));
        }
        if let Some(events) = self
            .event_streams
            .lock()
            .expect("fake event streams lock")
            .remove(session_key)
        {
            return Ok(events);
        }
        let (sink, events) = bounded_event_stream(4);
        sink.close();
        Ok(events)
    }

    fn pin_chat_launch(&self, options: &mut LaunchOptions) -> Result<(), RuntimeError> {
        SessionRuntime::pin_chat_launch_with(self.chat_home.as_deref(), options)
    }
}

fn engine(value: &str) -> EngineId {
    EngineId::new(value).expect("valid test engine")
}

fn summary(engine_id: &str, session_id: &str, cwd: &str) -> SessionSummary {
    SessionSummary {
        engine: engine(engine_id),
        session_id: session_id.into(),
        title: "Test session".into(),
        cwd: cwd.into(),
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

fn isolated_worktree() -> Worktree {
    Worktree {
        path: PathBuf::from("/repo/.interface-worktrees/build-a1"),
        session_cwd: PathBuf::from("/repo/.interface-worktrees/build-a1/src"),
        branch: "iface/build-a1".into(),
        repo_root: PathBuf::from("/repo"),
        source_cwd: PathBuf::from("/repo/src"),
        dirty_source: true,
    }
}

type DispatchCase = (&'static str, Value, RuntimeBridgeCommand);

fn session_lifecycle_cases() -> Vec<DispatchCase> {
    vec![
        (
            CHANNELS.list_sessions,
            Value::Null,
            RuntimeBridgeCommand::ListSessions,
        ),
        (
            CHANNELS.probe_engines,
            json!({}),
            RuntimeBridgeCommand::ProbeAllProviders,
        ),
        (
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/project", "prompt": "build it"}
            }),
            RuntimeBridgeCommand::StartSession {
                engine: engine("codex"),
                options: LaunchOptions {
                    cwd: "/project".into(),
                    prompt: "build it".into(),
                    ..LaunchOptions::default()
                },
            },
        ),
        (
            CHANNELS.send,
            json!({"sessionKey": "codex#one", "text": "continue"}),
            RuntimeBridgeCommand::Send {
                session_key: "codex#one".into(),
                text: "continue".into(),
            },
        ),
        (
            CHANNELS.interrupt,
            json!({"sessionKey": "codex#one"}),
            RuntimeBridgeCommand::CancelSession {
                session_key: "codex#one".into(),
            },
        ),
        (
            CHANNELS.close_session,
            json!({"sessionKey": "codex#one"}),
            RuntimeBridgeCommand::CloseSession {
                session_key: "codex#one".into(),
            },
        ),
    ]
}

fn approval_configuration_and_history_cases() -> Vec<DispatchCase> {
    vec![
        (
            CHANNELS.respond,
            json!({
                "sessionKey": "codex#one",
                "approvalId": "approval-1",
                "verdict": "allow",
                "note": "approved",
                "answers": {"first": "yes", "second": "no"}
            }),
            RuntimeBridgeCommand::RespondToApproval {
                session_key: "codex#one".into(),
                approval_id: "approval-1".into(),
                verdict: BridgeApprovalVerdict::Allow,
                response: BridgeApprovalResponse {
                    note: Some("approved".into()),
                    answers: vec![
                        ("first".into(), "yes".into()),
                        ("second".into(), "no".into()),
                    ],
                },
            },
        ),
        (
            CHANNELS.set_model,
            json!({"sessionKey": "codex#one", "model": "gpt-5"}),
            RuntimeBridgeCommand::SetModel {
                session_key: "codex#one".into(),
                model: Some("gpt-5".into()),
            },
        ),
        (
            CHANNELS.set_effort,
            json!({"sessionKey": "codex#one", "effort": null}),
            RuntimeBridgeCommand::SetEffort {
                session_key: "codex#one".into(),
                effort: None,
            },
        ),
        (
            CHANNELS.set_fast_mode,
            json!({"sessionKey": "codex#one", "enabled": true}),
            RuntimeBridgeCommand::SetFastMode {
                session_key: "codex#one".into(),
                enabled: true,
            },
        ),
        (
            CHANNELS.set_permission_mode,
            json!({"sessionKey": "codex#one", "mode": "on-request"}),
            RuntimeBridgeCommand::SetPermissionMode {
                session_key: "codex#one".into(),
                mode: "on-request".into(),
            },
        ),
        (
            CHANNELS.read_history,
            json!({"sessionKey": "codex#one", "options": {"limit": 750}}),
            RuntimeBridgeCommand::HistorySession {
                session_key: "codex#one".into(),
            },
        ),
    ]
}

fn confirmed_delivery() -> RuntimeBridgeResponse {
    RuntimeBridgeResponse::Delivery(RuntimeDeliveryReceipt {
        disposition: RuntimeDeliveryDisposition::Confirmed,
        error: None,
        audit: RuntimeDeliveryAudit::NotApplicable,
    })
}

fn response_and_legacy_result(operation: &str) -> (RuntimeBridgeResponse, Value) {
    if operation == CHANNELS.list_sessions {
        return (RuntimeBridgeResponse::Sessions(Vec::new()), json!([]));
    }
    if operation == CHANNELS.probe_engines {
        return (
            RuntimeBridgeResponse::ProvidersReadiness(Vec::new()),
            json!([]),
        );
    }
    if operation == CHANNELS.create_session {
        return (
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
                key: "codex#created".into(),
                engine: engine("codex"),
                session_id: "created".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            })),
            json!({"key": "codex#created"}),
        );
    }
    if operation == CHANNELS.read_history {
        return (
            RuntimeBridgeResponse::HistorySession(Box::new(None)),
            json!({
                "availability": "unavailable",
                "reason": "No durable Rust history is available for this session.",
                "events": [],
                "olderMessages": 0
            }),
        );
    }
    if operation == CHANNELS.set_model {
        return (
            confirmed_delivery(),
            json!({"key": "codex#one", "resumed": false}),
        );
    }
    (confirmed_delivery(), Value::Null)
}

#[tokio::test]
async fn settled_session_activity_retries_then_proves_inactive() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let result = settled_session_activity(
        || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            if attempt < 2 {
                Err(RuntimeError::definitely_unsent(
                    RuntimeErrorKind::History,
                    "crossing attachment edge",
                    true,
                ))
            } else {
                Ok(Some(false))
            }
        },
        "codex#one",
        "could not prove that the terminal attachment left the runtime",
    )
    .await;
    assert_eq!(result, Some(false));
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn settled_session_activity_stops_without_retrying_when_the_port_has_no_answer() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let result = settled_session_activity(
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
        "codex#one",
        "could not prove orchestration session attachment closure",
    )
    .await;
    assert_eq!(result, None);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn documented_camel_case_payloads_map_to_exact_runtime_commands() {
    let cases = session_lifecycle_cases()
        .into_iter()
        .chain(approval_configuration_and_history_cases());

    for (operation, payload, expected) in cases {
        let (response, expected_result) = response_and_legacy_result(operation);
        let runtime = Arc::new(if operation == CHANNELS.interrupt {
            FakeRuntime::new(response)
                .with_summary("codex#one", summary("codex", "one", "/project"))
        } else {
            FakeRuntime::new(response)
        });
        let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

        let encoded = dispatcher
            .dispatch(operation, payload)
            .await
            .expect("mapped operation");

        assert_eq!(runtime.commands(), vec![expected], "{operation}");
        assert_eq!(encoded, expected_result, "{operation}");
    }
}

#[derive(Default)]
struct CountingRounds(std::sync::atomic::AtomicUsize);

impl RegistryRoundObserver for CountingRounds {
    fn registry_round(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// 6 Oct 2026 (Threads review): a session only a listing found (a terminal
/// CLI run) got no thread record until some other registry change, since a
/// listing schedules no registry push. A listing that changed the registry
/// now tells the thread host's signal, and still pushes nothing.
#[tokio::test]
async fn a_listing_that_changed_the_registry_tells_the_listing_observer_without_a_push() {
    let runtime = Arc::new(FakeRuntime::scripted([
        RuntimeBridgeResponse::Sessions(Vec::new()),
        RuntimeBridgeResponse::Sessions(Vec::new()),
        RuntimeBridgeResponse::ProvidersReadiness(Vec::new()),
        RuntimeBridgeResponse::Sessions(Vec::new()),
    ]));
    *runtime.revision.lock().unwrap() = Some(7);
    runtime.listing_finds.store(true, Ordering::SeqCst);
    let rounds = Arc::new(CountingRounds::default());
    let (push_sender, mut frames) = mpsc::channel(8);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender)
        .with_listing_observer(rounds.clone());
    let told = || rounds.0.load(Ordering::SeqCst);

    dispatcher
        .dispatch(CHANNELS.list_sessions, Value::Null)
        .await
        .unwrap();
    assert_eq!(told(), 1, "a listing that found a session");
    // Nothing new, and not a listing: nobody is told.
    runtime.listing_finds.store(false, Ordering::SeqCst);
    dispatcher
        .dispatch(CHANNELS.list_sessions, Value::Null)
        .await
        .unwrap();
    dispatcher
        .dispatch(CHANNELS.probe_engines, json!({}))
        .await
        .unwrap();
    assert_eq!(told(), 1);
    // A registry that cannot say whether it changed always tells.
    *runtime.revision.lock().unwrap() = None;
    dispatcher
        .dispatch(CHANNELS.list_sessions, Value::Null)
        .await
        .unwrap();
    assert_eq!(told(), 2);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert!(
        frames.try_recv().is_err(),
        "a listing pushes no registry frame"
    );
}

#[tokio::test]
async fn collaboration_fence_refuses_mutation_before_provider_contact() {
    let runtime = Arc::new(FakeRuntime::new(confirmed_delivery()));
    let hooks = Arc::new(FakeCollaborationHooks {
        reject_mutations: true,
        ..FakeCollaborationHooks::default()
    });
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_collaboration_hooks(hooks.clone());

    let error = dispatcher
        .dispatch(
            CHANNELS.set_effort,
            json!({"sessionKey": "codex#one", "effort": "high"}),
        )
        .await
        .expect_err("collaboration-owned mutation");

    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert!(runtime.commands().is_empty());
    assert_eq!(
        hooks
            .guards
            .lock()
            .expect("fake collaboration guards")
            .as_slice(),
        &[("codex#one".into(), "effort change".into(), false)]
    );
}

#[tokio::test]
async fn send_projection_starts_before_provider_contact_and_preserves_certainty() {
    for (disposition, expected_outcome, expected_code) in [
        (
            RuntimeDeliveryDisposition::Confirmed,
            ProjectedSendOutcome::AdapterAccepted,
            None,
        ),
        (
            RuntimeDeliveryDisposition::DefinitelyUnsent,
            ProjectedSendOutcome::DefinitelyUnsent,
            Some(DEFINITELY_UNSENT_CODE),
        ),
        (
            RuntimeDeliveryDisposition::OutcomeUnknown,
            ProjectedSendOutcome::OutcomeUnknown,
            Some(OUTCOME_UNKNOWN_CODE),
        ),
    ] {
        let hooks = Arc::new(FakeCollaborationHooks::default());
        let runtime = Arc::new(
            FakeRuntime::new(RuntimeBridgeResponse::Delivery(RuntimeDeliveryReceipt {
                disposition,
                error: None,
                audit: RuntimeDeliveryAudit::NotApplicable,
            }))
            .with_send_start_fence(Arc::clone(&hooks.started)),
        );
        let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime))
            .with_collaboration_hooks(hooks.clone());

        let result = dispatcher
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": "codex#one", "text": "continue"}),
            )
            .await;

        match expected_code {
            None => assert_eq!(result.expect("confirmed send"), Value::Null),
            Some(expected_code) => {
                assert_eq!(result.expect_err("failed send").code, expected_code);
            }
        }
        assert_eq!(
            hooks
                .starts
                .lock()
                .expect("fake collaboration starts")
                .as_slice(),
            &["codex#one"]
        );
        assert_eq!(
            hooks
                .settlements
                .lock()
                .expect("fake collaboration settlements")
                .as_slice(),
            &[("codex#one".into(), "continue".into(), expected_outcome)]
        );
    }
}

/// Records what the send hook saw and how each send settled.
#[derive(Default)]
struct RecordingSendHooks {
    refuse: bool,
    /// Tickets say the host itself sent the prompt.
    echo: bool,
    prepared: Mutex<Vec<(String, String)>>,
    settled: Arc<Mutex<Vec<SendSettlement>>>,
    interrupts: Mutex<Vec<String>>,
}

struct RecordingTicket(Arc<Mutex<Vec<SendSettlement>>>, bool);

impl SendTicket for RecordingTicket {
    fn settle(self: Box<Self>, settlement: SendSettlement) {
        self.0.lock().expect("settled lock").push(settlement);
    }

    fn echoes_prompt(&self) -> bool {
        self.1
    }
}

#[async_trait]
impl SendHooks for RecordingSendHooks {
    async fn before_send(
        &self,
        session_key: &str,
        text: &str,
    ) -> Result<Option<Box<dyn SendTicket>>, DispatchError> {
        self.prepared
            .lock()
            .expect("prepared lock")
            .push((session_key.to_owned(), text.to_owned()));
        if self.refuse {
            return Err(DispatchError::new(
                "threads-worktree-unavailable",
                "The worktree is missing. Nothing was sent.",
                false,
                true,
            ));
        }
        Ok(Some(Box::new(RecordingTicket(
            Arc::clone(&self.settled),
            self.echo,
        ))))
    }

    fn interrupt_requested(&self, session_key: &str) {
        self.interrupts
            .lock()
            .expect("interrupts lock")
            .push(session_key.to_owned());
    }
}

/// Review B3 (14 Oct 2026): an Away loop's own prompt reaches its thread's
/// pane as a live `user.message` before the send, so the round stands above
/// its answer; the window's own sends are echoed by the window, never twice.
#[tokio::test]
async fn a_host_prompt_is_shown_in_its_pane_before_the_send() {
    for echo in [true, false] {
        let hooks = Arc::new(RecordingSendHooks {
            echo,
            ..RecordingSendHooks::default()
        });
        let runtime = Arc::new(FakeRuntime::new(confirmed_delivery()));
        let (pushes, mut frames) = mpsc::channel(8);
        let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), pushes)
            .with_send_hooks(hooks.clone());
        dispatcher
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": "claude#away", "text": "Away work round 1"}),
            )
            .await
            .expect("delivered");
        let mut shown = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            if frame.operation == CHANNELS.event {
                shown.push(frame.payload.decode::<Value>().unwrap());
            }
        }
        if echo {
            assert_eq!(shown.len(), 1, "{shown:?}");
            assert_eq!(shown[0]["key"], "claude#away");
            assert_eq!(shown[0]["event"]["kind"], "user.message");
            assert_eq!(shown[0]["event"]["text"], "Away work round 1");
        } else {
            assert!(shown.is_empty(), "{shown:?}");
        }
        assert_eq!(
            hooks.settled.lock().expect("settled").as_slice(),
            &[SendSettlement::Delivered]
        );
    }
}

#[tokio::test]
async fn send_hooks_run_before_provider_contact_and_settle_by_delivery_certainty() {
    // A rejection is told apart from a send that never left: an ACP prompt
    // is answered only when its turn ends, so a rejected one may have run.
    for (disposition, delivered) in [
        (
            RuntimeDeliveryDisposition::Confirmed,
            SendSettlement::Delivered,
        ),
        (
            RuntimeDeliveryDisposition::DefinitelyUnsent,
            SendSettlement::Unsent,
        ),
        (
            RuntimeDeliveryDisposition::DefinitelyRejected,
            SendSettlement::Rejected,
        ),
        (
            RuntimeDeliveryDisposition::OutcomeUnknown,
            SendSettlement::Delivered,
        ),
    ] {
        let hooks = Arc::new(RecordingSendHooks::default());
        let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::Delivery(
            RuntimeDeliveryReceipt {
                disposition,
                error: None,
                audit: RuntimeDeliveryAudit::NotApplicable,
            },
        )));
        let dispatcher =
            RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_send_hooks(hooks.clone());
        let _ = dispatcher
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": "codex#one", "text": "continue"}),
            )
            .await;
        assert_eq!(
            hooks.prepared.lock().expect("prepared").as_slice(),
            &[("codex#one".to_owned(), "continue".to_owned())]
        );
        assert_eq!(
            hooks.settled.lock().expect("settled").as_slice(),
            &[delivered],
            "{disposition:?}"
        );
    }

    // A refusal stops the send before anything reaches the runtime.
    let hooks = Arc::new(RecordingSendHooks {
        refuse: true,
        ..RecordingSendHooks::default()
    });
    let runtime = Arc::new(FakeRuntime::scripted([]));
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_send_hooks(hooks.clone());
    let error = dispatcher
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey": "codex#one", "text": "continue"}),
        )
        .await
        .expect_err("refused before the provider");
    assert_eq!(error.code, "threads-worktree-unavailable");
    assert!(error.replay_safe);
    assert!(runtime.commands().is_empty());

    // An interrupt is announced before it reaches the provider.
    let hooks = Arc::new(RecordingSendHooks::default());
    let runtime = Arc::new(
        FakeRuntime::new(confirmed_delivery())
            .with_summary("codex#one", summary("codex", "one", "/repo")),
    );
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_send_hooks(hooks.clone());
    dispatcher
        .dispatch(CHANNELS.interrupt, json!({"sessionKey": "codex#one"}))
        .await
        .expect("interrupt delivered");
    assert_eq!(
        hooks.interrupts.lock().expect("interrupts").as_slice(),
        &["codex#one".to_owned()]
    );
    assert!(matches!(
        runtime.commands().as_slice(),
        [RuntimeBridgeCommand::CancelSession { .. }]
    ));
}

#[tokio::test]
async fn resume_uses_only_session_key_and_restores_canonical_registry_options() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let mut recorded = summary("claude", "provider-session", &cwd);
    recorded.model = Some(" sonnet ".into());
    recorded.effort = Some(Some(" high ".into()));
    recorded.setting_sources = Some(vec!["user".into()]);
    recorded.disallowed_tools = Some(vec!["dangerous".into()]);
    recorded.kind = Some(SessionKind::Chat);
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: "claude#provider-session".into(),
        engine: engine("claude"),
        session_id: "provider-session".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime =
        Arc::new(FakeRuntime::new(response).with_summary("claude#provider-session", recorded));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let result = dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": "claude#provider-session"}),
        )
        .await
        .expect("resume result");

    assert_eq!(result, json!({"key": "claude#provider-session"}));
    assert_eq!(
        runtime.commands(),
        vec![RuntimeBridgeCommand::ResumeSession {
            engine: engine("claude"),
            provider_session_id: "provider-session".into(),
            cwd,
            options: ResumeOptions {
                setting_sources: Some(vec!["user".into()]),
                disallowed_tools: Some(vec!["dangerous".into()]),
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                collaboration_depth: None,
                kind: Some(SessionKind::Chat),
                ..ResumeOptions::default()
            },
        }]
    );
}

#[tokio::test]
async fn cold_send_resumes_and_restores_concrete_configuration_before_prompt_bytes() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let key = "claude#provider-session";
    let mut recorded = summary("claude", "provider-session", &cwd);
    recorded.model = Some(" sonnet ".into());
    recorded.effort = Some(Some(" high ".into()));
    let runtime = Arc::new(
        FakeRuntime::scripted([
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
                key: key.into(),
                engine: engine("claude"),
                session_id: "provider-session".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            })),
            confirmed_delivery(),
            confirmed_delivery(),
            confirmed_delivery(),
        ])
        .with_summary(key, recorded)
        .with_inactive(key),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let result = dispatcher
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey": key, "text": "continue"}),
        )
        .await
        .expect("cold send");

    assert_eq!(result, Value::Null);
    assert_eq!(
        runtime.commands(),
        vec![
            RuntimeBridgeCommand::ResumeSession {
                engine: engine("claude"),
                provider_session_id: "provider-session".into(),
                cwd,
                options: ResumeOptions {
                    setting_sources: None,
                    disallowed_tools: None,
                    model: Some("sonnet".into()),
                    effort: Some("high".into()),
                    collaboration_depth: None,
                    kind: None,
                    ..ResumeOptions::default()
                },
            },
            RuntimeBridgeCommand::SetModel {
                session_key: key.into(),
                model: Some("sonnet".into()),
            },
            RuntimeBridgeCommand::SetEffort {
                session_key: key.into(),
                effort: Some("high".into()),
            },
            RuntimeBridgeCommand::Send {
                session_key: key.into(),
                text: "continue".into(),
            },
        ]
    );
}

fn resumed_ready(key: &str, session_id: &str) -> RuntimeBridgeResponse {
    RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: key.into(),
        engine: engine("claude"),
        session_id: session_id.into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }))
}

fn resumed_options(runtime: &FakeRuntime) -> Vec<ResumeOptions> {
    runtime
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            RuntimeBridgeCommand::ResumeSession { options, .. } => Some(options),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn resume_payload_gate_fields_reach_the_resume_options() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let key = "claude#provider-session";
    let runtime = Arc::new(
        FakeRuntime::new(resumed_ready(key, "provider-session"))
            .with_summary(key, summary("claude", "provider-session", &cwd)),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));
    dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": key, "approvalGate": true, "additionalDirectories": ["/w/p"]}),
        )
        .await
        .expect("gated resume");
    let options = resumed_options(&runtime);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].approval_gate, Some(true));
    assert_eq!(
        options[0].additional_directories,
        Some(vec!["/w/p".to_owned()])
    );
    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.resume_session,
                json!({"sessionKey": key, "approvalGate": true, "sandbox": "off"}),
            )
            .await
            .expect_err("unknown resume field")
            .code,
        INVALID_PAYLOAD_CODE
    );
}

struct Requirements(Vec<(String, ResumeRequirement)>);

impl ResumeRequirements for Requirements {
    fn resume_requirement(&self, session_key: &str, _registry_cwd: &str) -> ResumeRequirement {
        self.0
            .iter()
            .find(|(key, _)| key == session_key)
            .map_or(ResumeRequirement::Unrestricted, |(_, requirement)| {
                requirement.clone()
            })
    }
}

#[tokio::test]
async fn resume_requirements_gate_explicit_and_implicit_resumes_over_the_payload() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let thread = "claude#thread";
    let bare = "claude#bare";
    let runtime = Arc::new(
        FakeRuntime::scripted([
            resumed_ready(thread, "thread"),
            resumed_ready(bare, "bare"),
            resumed_ready(thread, "thread"),
            confirmed_delivery(),
        ])
        .with_summary(thread, summary("claude", "thread", &cwd))
        .with_summary(bare, summary("claude", "bare", &cwd))
        .with_inactive(thread),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_resume_requirements(
        Arc::new(Requirements(vec![(
            thread.into(),
            ResumeRequirement::Gated(vec!["/w/p".into()]),
        )])),
    );
    // A caller asking for an ungated resume, or other folders, still gets the stored gate.
    dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": thread, "approvalGate": false, "additionalDirectories": ["/"]}),
        )
        .await
        .expect("explicit resume");
    dispatcher
        .dispatch(CHANNELS.resume_session, json!({"sessionKey": bare}))
        .await
        .expect("ungated resume");
    // A send to a detached gated session resumes it behind the gate first.
    dispatcher
        .dispatch(CHANNELS.send, json!({"sessionKey": thread, "text": "go"}))
        .await
        .expect("cold send");
    let options = resumed_options(&runtime);
    assert_eq!(options.len(), 3);
    for gated in [&options[0], &options[2]] {
        assert_eq!(gated.approval_gate, Some(true));
        assert_eq!(gated.additional_directories, Some(vec!["/w/p".to_owned()]));
    }
    assert_eq!(options[1].approval_gate, None);
    assert_eq!(options[1].additional_directories, None);
}

#[tokio::test]
async fn an_access_requirement_resumes_in_the_agents_mode_whatever_the_caller_asked() {
    use interface_contracts::engine::AgentAccess;
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let thread = "claude#thread";
    let runtime = Arc::new(
        FakeRuntime::scripted([resumed_ready(thread, "thread")])
            .with_summary(thread, summary("claude", "thread", &cwd))
            .with_inactive(thread),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_resume_requirements(
        Arc::new(Requirements(vec![(
            thread.into(),
            ResumeRequirement::Access(AgentAccess::Bypass, vec!["/w/p".into()]),
        )])),
    );
    dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": thread, "approvalGate": true, "additionalDirectories": ["/"]}),
        )
        .await
        .expect("explicit resume");
    let options = resumed_options(&runtime);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].approval_gate, None);
    assert_eq!(options[0].agent_access, Some(AgentAccess::Bypass));
    assert_eq!(
        options[0].additional_directories,
        Some(vec!["/w/p".to_owned()])
    );
}

#[tokio::test]
async fn refused_resume_requirement_stops_every_resume_before_the_provider() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let past = "claude#past";
    let runtime = Arc::new(
        FakeRuntime::scripted([])
            .with_summary(past, summary("claude", "past", &cwd))
            .with_inactive(past),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_resume_requirements(
        Arc::new(Requirements(vec![(
            past.into(),
            ResumeRequirement::Refused("Past threads are read-only.".into()),
        )])),
    );
    for (operation, payload) in [
        (CHANNELS.resume_session, json!({"sessionKey": past})),
        (CHANNELS.send, json!({"sessionKey": past, "text": "go"})),
        (
            CHANNELS.set_model,
            json!({"sessionKey": past, "model": "opus"}),
        ),
        (
            CHANNELS.set_effort,
            json!({"sessionKey": past, "effort": "high"}),
        ),
        (
            CHANNELS.set_fast_mode,
            json!({"sessionKey": past, "enabled": true}),
        ),
        (
            CHANNELS.set_permission_mode,
            json!({"sessionKey": past, "mode": "bypassPermissions"}),
        ),
    ] {
        let error = dispatcher
            .dispatch(operation, payload)
            .await
            .expect_err("refused resume");
        assert_eq!(error.code, DEFINITELY_UNSENT_CODE, "{operation}");
        assert_eq!(error.message, "Past threads are read-only.");
        assert!(error.replay_safe);
    }
    assert!(runtime.commands().is_empty(), "{:?}", runtime.commands());
}

/// Review C1: one writer per provider session. A session a running Code pane
/// is bound to, or one held for a pane that is starting, is never resumed,
/// sent to or reconfigured from the structured runtime, live or not; each
/// refusal is definitely unsent and reaches no provider.
#[tokio::test]
async fn a_session_a_code_pane_drives_is_never_resumed_or_sent_to_from_here() {
    struct Panes(Vec<String>);
    impl interface_runtime::TerminalSessionBindings for Panes {
        fn bound_provider_sessions(&self) -> Vec<String> {
            self.0.clone()
        }
    }
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let (paned, live, handed) = ("claude#paned", "codex#live", "claude#handed");
    let runtime = Arc::new(
        FakeRuntime::scripted([])
            .with_summary(paned, summary("claude", "paned", &cwd))
            .with_inactive(paned)
            .with_summary(live, summary("codex", "live", &cwd))
            .with_summary(handed, summary("claude", "handed", &cwd))
            .with_inactive(handed),
    );
    let claims = Arc::new(interface_runtime::SessionClaims::new(Arc::new(Panes(
        vec!["paned".into(), "live".into()],
    ))));
    claims.hand_to_terminal(handed);
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_session_claims(claims);
    for key in [paned, live, handed] {
        for (operation, payload) in [
            (CHANNELS.resume_session, json!({"sessionKey": key})),
            (CHANNELS.send, json!({"sessionKey": key, "text": "go"})),
            (
                CHANNELS.set_model,
                json!({"sessionKey": key, "model": "opus"}),
            ),
            (
                CHANNELS.set_permission_mode,
                json!({"sessionKey": key, "mode": "plan"}),
            ),
        ] {
            if key == live && operation == CHANNELS.resume_session {
                continue; // Already live: resume is a no-op, not an effect.
            }
            let error = dispatcher
                .dispatch(operation, payload)
                .await
                .expect_err("held by a pane");
            assert_eq!(error.code, DEFINITELY_UNSENT_CODE, "{key} {operation}");
            assert!(error.replay_safe, "{key} {operation}");
            assert!(error.message.contains("Code pane"), "{}", error.message);
        }
    }
    assert!(runtime.commands().is_empty(), "{:?}", runtime.commands());
}

struct NoFacts;

#[async_trait::async_trait]
impl crate::agents::ApprovalFactsSource for NoFacts {
    fn approval_facts(
        &self,
        _session_key: &str,
        tool: &str,
        _input: &Value,
    ) -> interface_contracts::agents::ApprovalFacts {
        interface_contracts::agents::ApprovalFacts::Unknown { tool: tool.into() }
    }
    fn session_cwd(&self, _session_key: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn ready_executable(&self, _engine: &str) -> Option<String> {
        None
    }
}

/// The production hook: a real agent host over a real desktop agent store.
#[tokio::test]
async fn agent_host_gates_current_threads_and_refuses_other_agent_home_sessions() {
    use interface_desktop_services::{
        DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts,
    };
    // Under /tmp, outside the refused system roots every /var/folders temp dir falls under.
    let temporary = tempfile::tempdir_in("/tmp").expect("temporary root");
    let home = temporary.path().join("home");
    let state = temporary.path().join("state");
    let work = temporary.path().join("work");
    for dir in [&home, &state, &work] {
        std::fs::create_dir_all(dir).expect("fixture folder");
    }
    let desktop = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp.json"),
                terminal_workspaces_file: state.join("terminals.json"),
                home_directory: home.clone(),
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("desktop service"),
    );
    desktop.set_agents_root(home.join("Interface Agents"));
    desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveAgent","expectedRevision":0,"profile":{
                "id":"research","name":"Research","engine":"claude","model":"","effort":"",
                "instructions":"Cite primary sources.","color":"sky","shape":"star"
            }}),
        )
        .expect("agent");
    let agent_home = desktop
        .prepare_agent_thread("research")
        .expect("agent home")
        .home;
    let revision = desktop.agent_control_response().expect("read")["revision"].clone();
    let granted = desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveAgentFolders","expectedRevision":revision,"agentId":"research","folders":[work.to_str().unwrap()]}),
        )
        .expect("grant")["agents"][0]["folders"][0]
        .as_str()
        .expect("granted folder")
        .to_owned();
    let (thread, fork, plain) = ("claude#thread", "claude#fork", "claude#plain");
    desktop
        .bind_agent_thread("research", thread)
        .expect("bind thread");
    let runtime = Arc::new(
        FakeRuntime::scripted([
            resumed_ready(thread, "thread"),
            confirmed_delivery(),
            resumed_ready(plain, "plain"),
        ])
        .with_summary(thread, summary("claude", "thread", &agent_home))
        .with_summary(fork, summary("claude", "fork", &agent_home))
        .with_summary(plain, summary("claude", "plain", work.to_str().unwrap()))
        .with_inactive(thread)
        .with_inactive(fork)
        .with_inactive(plain),
    );
    let host = crate::agents::AgentHost::new(
        desktop,
        Arc::new(crate::agents::LateDispatcher::default()),
        Arc::new(NoFacts),
    );
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_resume_requirements(host);

    dispatcher
        .dispatch(CHANNELS.send, json!({"sessionKey": thread, "text": "go"}))
        .await
        .expect("current thread send");
    // An agent-home session that is not the current thread never resumes, even unrecorded.
    let refused = dispatcher
        .dispatch(
            CHANNELS.set_model,
            json!({"sessionKey": fork, "model": "opus"}),
        )
        .await
        .expect_err("read-only fork");
    assert_eq!(
        refused.message,
        "Past conversations are read-only. Start a new conversation to continue."
    );
    dispatcher
        .dispatch(CHANNELS.resume_session, json!({"sessionKey": plain}))
        .await
        .expect("session outside the agents root");

    let options = resumed_options(&runtime);
    assert_eq!(options.len(), 2, "{:?}", runtime.commands());
    assert_eq!(options[0].approval_gate, Some(true));
    assert_eq!(options[0].additional_directories, Some(vec![granted]));
    assert_eq!(options[1].approval_gate, None);
    assert!(!runtime.commands().iter().any(|command| matches!(
        command,
        RuntimeBridgeCommand::SetModel { session_key, .. } if session_key == fork
    )));
}

#[tokio::test]
async fn isolated_create_strips_the_flag_persists_identity_and_emits_the_legacy_notice() {
    let key = "codex#created";
    let worktree = isolated_worktree();
    let worktrees = Arc::new(FakeWorktrees {
        prepared: worktree.clone(),
        persisted: Mutex::new(Vec::new()),
        discarded: Mutex::new(Vec::new()),
        persist_failure: false,
    });
    let (event_sink, event_stream) = bounded_event_stream(4);
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::SessionOpen(
            SessionOpenOutcome::Ready(RuntimeSession {
                key: key.into(),
                engine: engine("codex"),
                session_id: "created".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            }),
        ))
        .with_summary(
            key,
            summary(
                "codex",
                "created",
                "/repo/.interface-worktrees/build-a1/src",
            ),
        )
        .with_event_stream(key, event_stream),
    );
    let (push_sender, mut pushes) = mpsc::channel(8);
    let dispatcher = RuntimeHostDispatcher::with_pushes_and_worktrees(
        Arc::clone(&runtime),
        push_sender,
        worktrees.clone(),
    );

    let result = dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {
                    "cwd": "/repo/src",
                    "prompt": "build it",
                    "name": "Build",
                    "worktree": true
                }
            }),
        )
        .await
        .expect("isolated create");

    assert_eq!(result, json!({"key": key}));
    assert_eq!(
        runtime.commands(),
        vec![RuntimeBridgeCommand::StartSession {
            engine: engine("codex"),
            options: LaunchOptions {
                cwd: "/repo/.interface-worktrees/build-a1/src".into(),
                prompt: "build it".into(),
                name: Some("Build".into()),
                worktree: None,
                ..LaunchOptions::default()
            },
        }]
    );
    assert_eq!(
        worktrees
            .persisted
            .lock()
            .expect("persisted worktrees")
            .as_slice(),
        &[(key.into(), worktree.clone())]
    );
    assert!(
        worktrees
            .discarded
            .lock()
            .expect("discarded worktrees")
            .is_empty()
    );
    // The registry frame joins the coalescer window, so the notice leaves first.
    let notice = pushes.recv().await.expect("worktree notice");
    assert_eq!(notice.operation, WORKTREE_PUSH);
    assert_eq!(notice.payload["key"], key);
    assert_eq!(notice.payload["state"], "created");
    assert_eq!(
        notice.payload["path"],
        worktree.path.to_string_lossy().as_ref()
    );
    let registry = pushes.recv().await.expect("registry push");
    assert_eq!(registry.operation, CHANNELS.registry);
    assert_eq!(registry.payload[0]["worktreeHome"], "/repo/src");
    assert_eq!(
        registry.payload[0]["worktreePath"],
        worktree.session_cwd.to_string_lossy().as_ref()
    );
    assert!(notice.payload["detail"].as_str().is_some_and(|detail| {
        detail.contains("uncommitted changes") && detail.contains("last commit")
    }));
    event_sink.close();
}

#[tokio::test]
async fn provider_ready_create_survives_best_effort_projection_failures() {
    let key = "codex#created";
    let worktree = isolated_worktree();
    let worktrees = Arc::new(FakeWorktrees {
        prepared: worktree,
        persisted: Mutex::new(Vec::new()),
        discarded: Mutex::new(Vec::new()),
        persist_failure: true,
    });
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::SessionOpen(
            SessionOpenOutcome::Ready(RuntimeSession {
                key: key.into(),
                engine: engine("codex"),
                session_id: "created".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            }),
        ))
        .with_summary(
            key,
            summary(
                "codex",
                "created",
                "/repo/.interface-worktrees/build-a1/src",
            ),
        )
        .with_projection_failures(),
    );
    let (push_sender, push_receiver) = mpsc::channel(1);
    drop(push_receiver);
    let dispatcher = RuntimeHostDispatcher::with_pushes_and_worktrees(
        Arc::clone(&runtime),
        push_sender,
        Arc::clone(&worktrees) as Arc<dyn WorktreeService>,
    );

    let result = dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {
                    "cwd": "/repo/src",
                    "prompt": "build it",
                    "worktree": true
                }
            }),
        )
        .await
        .expect("provider-ready create result");

    assert_eq!(result, json!({"key": key}));
    assert!(
        worktrees
            .discarded
            .lock()
            .expect("discarded worktrees")
            .is_empty(),
        "a worktree that already ran a provider is never treated as an unused launch"
    );
}

#[tokio::test]
async fn isolated_create_without_a_worktree_service_fails_before_provider_contact() {
    let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(
        Vec::new(),
    )));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let error = dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/repo", "prompt": "build", "worktree": true}
            }),
        )
        .await
        .expect_err("missing worktree service");

    assert_eq!(error.code, "worktree-isolation-unavailable");
    assert!(error.replay_safe);
    assert!(runtime.commands().is_empty());
}

fn chat_unavailable() -> String {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/session-kind-contract.json"
    ))
    .expect("session-kind contract fixture");
    fixture["chatUnavailable"]
        .as_str()
        .expect("chatUnavailable")
        .to_owned()
}

#[tokio::test]
async fn chat_create_without_chat_home_is_refused_before_worktree_isolation() {
    let worktrees = Arc::new(FakeWorktrees {
        prepared: isolated_worktree(),
        persisted: Mutex::new(Vec::new()),
        discarded: Mutex::new(Vec::new()),
        persist_failure: false,
    });
    let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(
        Vec::new(),
    )));
    let (push_sender, _pushes) = mpsc::channel(8);
    let dispatcher = RuntimeHostDispatcher::with_pushes_and_worktrees(
        Arc::clone(&runtime),
        push_sender,
        worktrees.clone(),
    );

    let error = dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "claude",
                "options": {
                    "cwd": "/repo/src",
                    "prompt": "hello",
                    "kind": "chat",
                    "worktree": true
                }
            }),
        )
        .await
        .expect_err("chat without chat home");

    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert_eq!(error.message, chat_unavailable());
    assert!(!error.retryable);
    assert!(error.replay_safe);
    assert!(runtime.commands().is_empty());
    assert!(
        worktrees
            .persisted
            .lock()
            .expect("persisted worktrees")
            .is_empty()
    );
}

#[tokio::test]
async fn chat_create_pins_cwd_and_never_creates_a_worktree() {
    let chat = tempfile::tempdir().expect("chat home");
    let chat_home = chat.path().to_path_buf();
    let expected_cwd = chat_home.to_string_lossy().into_owned();
    let worktrees = Arc::new(FakeWorktrees {
        prepared: isolated_worktree(),
        persisted: Mutex::new(Vec::new()),
        discarded: Mutex::new(Vec::new()),
        persist_failure: false,
    });
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::SessionOpen(
            SessionOpenOutcome::Ready(RuntimeSession {
                key: "claude#created".into(),
                engine: engine("claude"),
                session_id: "created".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            }),
        ))
        .with_chat_home(chat_home)
        .with_summary(
            "claude#created",
            summary("claude", "created", &expected_cwd),
        ),
    );
    let dispatcher = RuntimeHostDispatcher::with_pushes_and_worktrees(
        Arc::clone(&runtime),
        mpsc::channel(8).0,
        worktrees.clone(),
    );

    let result = dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "claude",
                "options": {
                    "cwd": "/repo/src",
                    "prompt": "hello",
                    "kind": "chat",
                    "worktree": true
                }
            }),
        )
        .await
        .expect("pinned chat create");

    assert_eq!(result, json!({"key": "claude#created"}));
    assert_eq!(
        runtime.commands(),
        vec![RuntimeBridgeCommand::StartSession {
            engine: engine("claude"),
            options: LaunchOptions {
                cwd: expected_cwd,
                prompt: "hello".into(),
                worktree: None,
                kind: Some(SessionKind::Chat),
                ..LaunchOptions::default()
            },
        }]
    );
    assert!(
        worktrees
            .persisted
            .lock()
            .expect("persisted worktrees")
            .is_empty(),
        "a chat must never enter worktree isolation"
    );
}

#[tokio::test]
async fn cold_set_model_returns_the_exact_legacy_activation_result() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let key = "claude#provider-session";
    let runtime = Arc::new(
        FakeRuntime::scripted([
            RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
                key: key.into(),
                engine: engine("claude"),
                session_id: "provider-session".into(),
                ready_at: "2026-08-27T00:00:00Z".into(),
            })),
            confirmed_delivery(),
        ])
        .with_summary(key, summary("claude", "provider-session", &cwd))
        .with_inactive(key),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let result = dispatcher
        .dispatch(
            CHANNELS.set_model,
            json!({"sessionKey": key, "model": "opus"}),
        )
        .await
        .expect("cold set model");

    assert_eq!(result, json!({"key": key, "resumed": true}));
    assert_eq!(
        runtime.commands(),
        vec![
            RuntimeBridgeCommand::ResumeSession {
                engine: engine("claude"),
                provider_session_id: "provider-session".into(),
                cwd,
                options: ResumeOptions::default(),
            },
            RuntimeBridgeCommand::SetModel {
                session_key: key.into(),
                model: Some("opus".into()),
            },
        ]
    );
}

#[tokio::test]
async fn cold_effort_fast_and_permission_changes_resume_before_delivery() {
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let key = "claude#provider-session";
    let cases = [
        (
            CHANNELS.set_effort,
            json!({"sessionKey": key, "effort": "high"}),
            RuntimeBridgeCommand::SetEffort {
                session_key: key.into(),
                effort: Some("high".into()),
            },
        ),
        (
            CHANNELS.set_fast_mode,
            json!({"sessionKey": key, "enabled": true}),
            RuntimeBridgeCommand::SetFastMode {
                session_key: key.into(),
                enabled: true,
            },
        ),
        (
            CHANNELS.set_permission_mode,
            json!({"sessionKey": key, "mode": "auto"}),
            RuntimeBridgeCommand::SetPermissionMode {
                session_key: key.into(),
                mode: "auto".into(),
            },
        ),
    ];

    for (operation, payload, expected_change) in cases {
        let runtime = Arc::new(
            FakeRuntime::scripted([
                RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
                    key: key.into(),
                    engine: engine("claude"),
                    session_id: "provider-session".into(),
                    ready_at: "2026-08-27T00:00:00Z".into(),
                })),
                confirmed_delivery(),
            ])
            .with_summary(key, summary("claude", "provider-session", &cwd))
            .with_inactive(key),
        );
        let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

        assert_eq!(
            dispatcher
                .dispatch(operation, payload)
                .await
                .expect("cold configuration change"),
            Value::Null
        );
        assert_eq!(
            runtime.commands(),
            vec![
                RuntimeBridgeCommand::ResumeSession {
                    engine: engine("claude"),
                    provider_session_id: "provider-session".into(),
                    cwd: cwd.clone(),
                    options: ResumeOptions::default(),
                },
                expected_change,
            ],
            "{operation}"
        );
    }
}

#[tokio::test]
async fn cold_grok_effort_persists_locally_without_starting_the_provider() {
    let key = "acp:grok#provider-session";
    let recorded = summary("acp:grok", "provider-session", "/missing-but-unused");
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(Vec::new()))
            .with_summary(key, recorded.clone())
            .with_inactive(key),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.set_effort,
                json!({"sessionKey": key, "effort": "high"}),
            )
            .await
            .expect("cold Grok effort"),
        Value::Null
    );
    assert!(runtime.commands().is_empty());
    assert_eq!(
        runtime.projected(),
        vec![(
            key.into(),
            EngineEvent {
                at: None,
                event: EngineEventKind::ConfigChanged {
                    model: recorded.model,
                    effort: Some(Some("high".into())),
                },
            },
        )]
    );
}

#[tokio::test]
async fn closing_a_cold_or_unknown_row_is_an_explicit_no_op() {
    let key = "claude#not-live";
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(Vec::new())).with_inactive(key),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.close_session, json!({"sessionKey": key}))
            .await
            .expect("cold close"),
        Value::Null
    );
    assert!(runtime.commands().is_empty());
}

#[tokio::test]
async fn interrupting_an_inactive_busy_row_is_an_idle_projecting_no_op() {
    let key = "claude#provider-session";
    let mut recorded = summary("claude", "provider-session", "/missing-but-unused");
    recorded.status = SessionStatus::Working;
    let runtime = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(Vec::new()))
            .with_summary(key, recorded)
            .with_inactive(key),
    );
    let (push_sender, mut pushes) = mpsc::channel(4);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender);

    let result = dispatcher
        .dispatch(CHANNELS.interrupt, json!({"sessionKey": key}))
        .await
        .expect("inactive interrupt");

    let expected = EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    };
    assert_eq!(result, Value::Null);
    assert!(runtime.commands().is_empty());
    assert_eq!(runtime.projected(), vec![(key.into(), expected.clone())]);
    assert_eq!(
        pushes.recv().await.expect("status push").payload,
        json!({"key": key, "event": expected})
    );
    let registry = pushes.recv().await.expect("registry push");
    assert_eq!(registry.operation, CHANNELS.registry);
    assert_eq!(registry.payload[0]["status"], "idle");
}

#[tokio::test]
async fn resume_refuses_missing_records_and_working_directories_before_provider_contact() {
    let missing = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(
        Vec::new(),
    )));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&missing));
    let error = dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": "claude#missing"}),
        )
        .await
        .expect_err("missing record");
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert!(missing.commands().is_empty());

    let no_cwd = Arc::new(
        FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(Vec::new()))
            .with_summary("claude#no-cwd", summary("claude", "no-cwd", "")),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&no_cwd));
    let error = dispatcher
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey": "claude#no-cwd"}),
        )
        .await
        .expect_err("missing cwd");
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert!(no_cwd.commands().is_empty());
}

#[tokio::test]
async fn live_runtime_events_use_the_exact_two_argument_electron_push_payload() {
    let (event_sink, event_stream) = bounded_event_stream(4);
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: "codex#created".into(),
        engine: engine("codex"),
        session_id: "created".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime = Arc::new(
        FakeRuntime::new(response)
            .with_summary("codex#created", summary("codex", "created", "/project"))
            .with_event_stream("codex#created", event_stream),
    );
    let (push_sender, mut pushes) = mpsc::channel(8);
    let dispatcher = RuntimeHostDispatcher::with_pushes(runtime, push_sender);

    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.create_session,
                json!({
                    "engine": "codex",
                    "options": {"cwd": "/project", "prompt": "build it"}
                }),
            )
            .await
            .expect("session open"),
        json!({"key": "codex#created"})
    );
    let initial_registry = pushes.recv().await.expect("initial registry push");
    assert_eq!(initial_registry.operation, CHANNELS.registry);
    assert!(initial_registry.payload.is_array());

    let event = EngineEvent {
        at: Some("2026-08-27T00:00:01Z".into()),
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    };
    event_sink
        .emit(RuntimeEventFrame::Event {
            value: RuntimeEvent {
                session_key: "codex#created".into(),
                sequence: 1,
                received_at: "2026-08-27T00:00:01Z".into(),
                event: event.clone(),
            },
        })
        .expect("fake runtime event");

    let event_push = pushes.recv().await.expect("event push");
    assert_eq!(event_push.operation, CHANNELS.event);
    assert_eq!(
        event_push.payload,
        json!({"key": "codex#created", "event": event})
    );
    assert_eq!(
        pushes.recv().await.expect("event registry push").operation,
        CHANNELS.registry
    );
    event_sink.close();
}

#[tokio::test]
async fn live_event_registry_pushes_coalesce_and_use_runtime_observation_time() {
    let (event_sink, event_stream) = bounded_event_stream(8);
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: "codex#burst".into(),
        engine: engine("codex"),
        session_id: "burst".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime = Arc::new(
        FakeRuntime::new(response)
            .with_summary("codex#burst", summary("codex", "burst", "/project"))
            .with_event_stream("codex#burst", event_stream),
    );
    let (push_sender, mut pushes) = mpsc::channel(16);
    let dispatcher = RuntimeHostDispatcher::with_pushes(runtime, push_sender);
    dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/project", "prompt": "build it"}
            }),
        )
        .await
        .expect("session open");
    assert_eq!(
        pushes.recv().await.expect("initial registry").operation,
        CHANNELS.registry
    );

    for sequence in 1..=3 {
        event_sink
            .emit(RuntimeEventFrame::Event {
                value: RuntimeEvent {
                    session_key: "codex#burst".into(),
                    sequence,
                    received_at: format!("2026-08-27T00:00:0{sequence}Z"),
                    event: EngineEvent {
                        at: Some("provider-time-is-not-the-live-stamp".into()),
                        event: EngineEventKind::StatusChanged {
                            status: SessionStatus::Working,
                        },
                    },
                },
            })
            .expect("burst event");
    }

    for sequence in 1..=3 {
        let push = pushes.recv().await.expect("event push");
        assert_eq!(push.operation, CHANNELS.event);
        assert_eq!(
            push.payload["event"]["at"],
            format!("2026-08-27T00:00:0{sequence}Z")
        );
    }
    let registry = tokio::time::timeout(Duration::from_millis(150), pushes.recv())
        .await
        .expect("coalesced registry deadline")
        .expect("coalesced registry push");
    assert_eq!(registry.operation, CHANNELS.registry);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pushes.recv())
            .await
            .is_err(),
        "one token burst must not produce one whole-registry frame per token"
    );
    event_sink.close();
}

#[tokio::test]
async fn terminal_stream_emits_one_closed_event_flushes_then_schedules_worktree_release() {
    let (event_sink, event_stream) = bounded_event_stream(8);
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: "codex#terminal".into(),
        engine: engine("codex"),
        session_id: "terminal".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime = Arc::new(
        FakeRuntime::new(response)
            .with_summary("codex#terminal", summary("codex", "terminal", "/project"))
            .with_event_stream("codex#terminal", event_stream)
            .with_inactive("codex#terminal"),
    );
    let releases = Arc::new(FakeAutomaticWorktreeRelease::default());
    let (push_sender, mut pushes) = mpsc::channel(16);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender)
        .with_worktree_release(releases.clone());
    dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/project", "prompt": "build it"}
            }),
        )
        .await
        .expect("session open");
    assert_eq!(
        pushes.recv().await.expect("initial registry").operation,
        CHANNELS.registry
    );

    // Provider-owned close markers are internal. The desktop receives the
    // one host-owned closure only after the runtime terminal frame.
    event_sink
        .emit(RuntimeEventFrame::Event {
            value: RuntimeEvent {
                session_key: "codex#terminal".into(),
                sequence: 1,
                received_at: "2026-08-27T00:00:01Z".into(),
                event: EngineEvent {
                    at: None,
                    event: EngineEventKind::SessionClosed,
                },
            },
        })
        .expect("provider close marker");
    event_sink
        .emit(RuntimeEventFrame::Terminal {
            session_key: "codex#terminal".into(),
            reason: interface_runtime::RuntimeEventTermination::Closed,
            message: None,
        })
        .expect("runtime terminal");
    event_sink.close();

    let closed = pushes.recv().await.expect("closed event");
    assert_eq!(closed.operation, CHANNELS.event);
    assert_eq!(closed.payload["key"], "codex#terminal");
    assert_eq!(closed.payload["event"]["kind"], "session.closed");
    assert!(closed.payload["event"]["at"].is_string());
    assert_eq!(
        pushes
            .recv()
            .await
            .expect("terminal registry flush")
            .operation,
        CHANNELS.registry
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pushes.recv())
            .await
            .is_err(),
        "provider and host closure markers must not produce duplicates"
    );
    assert!(runtime.projected().iter().any(|(key, event)| {
        key == "codex#terminal" && matches!(event.event, EngineEventKind::SessionClosed)
    }));
    assert_eq!(
        releases
            .scheduled
            .lock()
            .expect("automatic release calls")
            .as_slice(),
        &["codex#terminal"]
    );
}

/// The quit path: Electron ends stdin, `HostServer::run` returns and drops the
/// push receiver, and only then does the runtime close its sessions.
async fn close_after_the_desktop_receiver_is_gone(ordinary_event_first: bool) {
    let (event_sink, event_stream) = bounded_event_stream(8);
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: "codex#terminal".into(),
        engine: engine("codex"),
        session_id: "terminal".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime = Arc::new(
        FakeRuntime::new(response)
            .with_summary("codex#terminal", summary("codex", "terminal", "/project"))
            .with_event_stream("codex#terminal", event_stream)
            .with_inactive("codex#terminal"),
    );
    let releases = Arc::new(FakeAutomaticWorktreeRelease::default());
    let (push_sender, mut pushes) = mpsc::channel(16);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender)
        .with_worktree_release(releases.clone());
    dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/project", "prompt": "build it"}
            }),
        )
        .await
        .expect("session open");
    assert_eq!(
        pushes.recv().await.expect("initial registry").operation,
        CHANNELS.registry
    );

    drop(pushes);
    if ordinary_event_first {
        event_sink
            .emit(RuntimeEventFrame::Event {
                value: RuntimeEvent {
                    session_key: "codex#terminal".into(),
                    sequence: 1,
                    received_at: "2026-08-27T00:00:01Z".into(),
                    event: EngineEvent {
                        at: None,
                        event: EngineEventKind::StatusChanged {
                            status: SessionStatus::Idle,
                        },
                    },
                },
            })
            .expect("provider event");
    }
    event_sink
        .emit(RuntimeEventFrame::Terminal {
            session_key: "codex#terminal".into(),
            reason: interface_runtime::RuntimeEventTermination::Closed,
            message: None,
        })
        .expect("runtime terminal");
    event_sink.close();
    dispatcher.close_event_pumps().await;

    assert!(runtime.projected().iter().any(|(key, event)| {
        key == "codex#terminal" && matches!(event.event, EngineEventKind::SessionClosed)
    }));
    assert_eq!(
        releases
            .scheduled
            .lock()
            .expect("automatic release calls")
            .as_slice(),
        &["codex#terminal"]
    );
}

#[tokio::test]
async fn quit_after_the_desktop_receiver_is_gone_still_schedules_worktree_release() {
    close_after_the_desktop_receiver_is_gone(false).await;
}

#[tokio::test]
async fn quit_after_an_unforwarded_provider_event_still_projects_close_and_release() {
    close_after_the_desktop_receiver_is_gone(true).await;
}

#[tokio::test]
async fn an_overflowed_desktop_queue_waits_for_the_runtime_eviction_then_projects_close() {
    let key = "codex#overflowed";
    let status = |sequence| RuntimeEventFrame::Event {
        value: RuntimeEvent {
            session_key: key.into(),
            sequence,
            received_at: "2026-08-27T00:00:01Z".into(),
            event: EngineEvent {
                at: None,
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Working,
                },
            },
        },
    };
    // Two control frames against a one-event queue: the second closes it
    // with an overflow before the desktop pump reads anything.
    let (event_sink, event_stream) =
        interface_engine::bounded_event_stream_with_budget(1, 64 * 1024);
    event_sink.emit(status(1)).expect("first control frame");
    assert!(event_sink.emit(status(2)).is_err());
    let response = RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
        key: key.into(),
        engine: engine("codex"),
        session_id: "overflowed".into(),
        ready_at: "2026-08-27T00:00:00Z".into(),
    }));
    let runtime = Arc::new(
        FakeRuntime::new(response)
            .with_summary(key, summary("codex", "overflowed", "/project"))
            .with_event_stream(key, event_stream),
    );
    let (push_sender, mut pushes) = mpsc::channel(16);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender);
    dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({
                "engine": "codex",
                "options": {"cwd": "/project", "prompt": "build it"}
            }),
        )
        .await
        .expect("session open");
    assert_eq!(
        pushes.recv().await.expect("initial registry").operation,
        CHANNELS.registry
    );

    // The runtime has not evicted the session yet: nothing may claim it closed.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), pushes.recv())
            .await
            .is_err(),
        "a close must not be projected while the runtime still owns the session"
    );
    runtime
        .inactive
        .lock()
        .expect("fake inactive lock")
        .insert(key.to_owned());

    let closed = tokio::time::timeout(Duration::from_secs(2), pushes.recv())
        .await
        .expect("closed event after the eviction")
        .expect("push channel");
    assert_eq!(closed.operation, CHANNELS.event);
    assert_eq!(closed.payload["key"], key);
    assert_eq!(closed.payload["event"]["kind"], "session.closed");
}

#[tokio::test]
async fn history_options_accept_load_older_shape_and_clamp_to_legacy_minimum() {
    let session_summary = summary("codex", "one", "/project");
    let events = (0_u64..501)
        .map(|sequence| HistoryEvent {
            sequence,
            received_at: "2026-08-27T00:00:00Z".into(),
            event: EngineEvent {
                at: None,
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            },
        })
        .collect();
    let response = RuntimeBridgeResponse::HistorySession(Box::new(Some(HistorySession {
        key: "codex#one".into(),
        summary: session_summary,
        attachment_count: 1,
        last_attached_at: "2026-08-27T00:00:00Z".into(),
        next_event_sequence: 501,
        next_effect_sequence: 0,
        omitted_events_through: None,
        omitted_effects_through: None,
        stream_state: RuntimeStreamState::Closed,
        delivery_state: RuntimeDeliveryState::Closed,
        terminal_message: None,
        events,
        effects: VecDeque::new(),
    })));
    let runtime = Arc::new(FakeRuntime::new(response));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let result = dispatcher
        .dispatch(
            CHANNELS.read_history,
            json!({"sessionKey": "codex#one", "options": {"limit": 1}}),
        )
        .await
        .expect("history page");

    assert_eq!(result["events"].as_array().map(Vec::len), Some(500));
    assert_eq!(result["olderMessages"], 1);
    assert_eq!(
        runtime.commands(),
        vec![RuntimeBridgeCommand::HistorySession {
            session_key: "codex#one".into()
        }]
    );
}

#[tokio::test]
async fn payloads_are_strict_named_objects() {
    let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(
        Vec::new(),
    )));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    for (operation, payload) in [
        (CHANNELS.send, json!(["codex#one", "hello"])),
        (
            CHANNELS.send,
            json!({"sessionKey": "codex#one", "text": "hello", "extra": true}),
        ),
        (CHANNELS.list_sessions, json!({"unexpected": true})),
    ] {
        let result = dispatcher.dispatch(operation, payload).await;
        assert!(
            result.is_err(),
            "{operation} accepted an invalid payload: {result:?}"
        );
        let error = result.expect_err("checked above");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
        assert!(!error.retryable);
    }
    assert!(runtime.commands().is_empty());
}

#[tokio::test]
async fn every_valid_but_unmigrated_operation_fails_closed() {
    let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::RegisteredEngines(
        Vec::new(),
    )));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));

    let mut tested = 0;
    for channel in ALL_CHANNELS.iter().chain(EXTRA_CHANNELS) {
        let operation = channel.value;
        let request = crate::protocol::BridgeRequest {
            version: crate::protocol::PROTOCOL_VERSION,
            id: "test-request".into(),
            operation: operation.into(),
            payload: Value::Null,
        };
        if request.validate().is_err() {
            continue;
        }
        if RUNTIME_BRIDGE_OPERATIONS.contains(&operation) {
            continue;
        }
        let error = dispatcher
            .dispatch(operation, Value::Null)
            .await
            .expect_err("unmigrated operation must not fall back");

        assert_eq!(error.code, OPERATION_NOT_MIGRATED_CODE, "{operation}");
        assert_eq!(
            error.message, "This Interface operation has not migrated to the Rust runtime.",
            "{operation}"
        );
        assert!(!error.retryable, "{operation}");
        tested += 1;
    }
    assert_eq!(tested, 114);
    assert!(runtime.commands().is_empty());
}

#[tokio::test]
async fn unknown_delivery_outcomes_become_non_replayable_bridge_failures() {
    let response = RuntimeBridgeResponse::Delivery(RuntimeDeliveryReceipt {
        disposition: RuntimeDeliveryDisposition::OutcomeUnknown,
        error: None,
        audit: RuntimeDeliveryAudit::NotApplicable,
    });
    let runtime = Arc::new(FakeRuntime::new(response));
    let dispatcher = RuntimeHostDispatcher::new(runtime);

    let error = dispatcher
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey": "codex#one", "text": "continue"}),
        )
        .await
        .expect_err("unknown provider outcomes are bridge failures");

    assert_eq!(error.code, OUTCOME_UNKNOWN_CODE);
    assert!(!error.replay_safe);
}

#[tokio::test]
async fn approval_answers_join_the_registry_coalescer() {
    let runtime = Arc::new(
        FakeRuntime::scripted([
            confirmed_delivery(),
            confirmed_delivery(),
            confirmed_delivery(),
        ])
        .with_summary("codex#one", summary("codex", "one", "/project")),
    );
    let (push_sender, mut pushes) = mpsc::channel(16);
    let dispatcher = RuntimeHostDispatcher::with_pushes(runtime, push_sender);
    for approval in ["approval-1", "approval-2", "approval-3"] {
        dispatcher
            .dispatch(
                CHANNELS.respond,
                json!({"sessionKey": "codex#one", "approvalId": approval, "verdict": "allow"}),
            )
            .await
            .expect("approval answer");
    }
    let registry = tokio::time::timeout(Duration::from_millis(150), pushes.recv())
        .await
        .expect("coalesced registry deadline")
        .expect("coalesced registry push");
    assert_eq!(registry.operation, CHANNELS.registry);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pushes.recv())
            .await
            .is_err(),
        "each approval answer must not push its own whole-registry frame"
    );
}

/// 7 Oct 2026: a thread started in Auto asked before every tool once the app
/// had restarted, because every resume began in the provider's default. The
/// host now keeps the mode the owner chose (at launch, then each change) in
/// `session-access.v1.json` and resumes in it; agent sessions keep their own.
#[tokio::test]
async fn a_session_resumes_after_a_restart_in_the_mode_its_owner_chose() {
    use crate::session_access::HostSessionAccess;

    let data = tempfile::tempdir().expect("data root");
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let thread = "claude#thread";
    let agent = "claude#agent";

    // Launch in Auto.
    let runtime = Arc::new(FakeRuntime::new(resumed_ready(thread, "thread")));
    let (push_sender, _pushes) = mpsc::channel(8);
    let dispatcher = RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), push_sender)
        .with_session_access(HostSessionAccess::open(data.path()).expect("store"));
    dispatcher
        .dispatch(
            CHANNELS.create_session,
            json!({"engine": "claude", "options": {
                "cwd": cwd, "prompt": "plan it", "permissionMode": "auto"}}),
        )
        .await
        .expect("launch");
    drop(dispatcher);

    // After a restart: the resume starts in Auto; a change to Plan is kept.
    let mut agent_summary = summary("claude", "agent", &cwd);
    agent_summary.kind = Some(SessionKind::Agent);
    let runtime = Arc::new(
        FakeRuntime::scripted([
            resumed_ready(thread, "thread"),
            confirmed_delivery(),
            resumed_ready(agent, "agent"),
            confirmed_delivery(),
        ])
        .with_summary(thread, summary("claude", "thread", &cwd))
        .with_summary(agent, agent_summary.clone())
        .with_inactive(thread)
        .with_inactive(agent),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime))
        .with_session_access(HostSessionAccess::open(data.path()).expect("reopened store"));
    dispatcher
        .dispatch(
            CHANNELS.set_permission_mode,
            json!({"sessionKey": thread, "mode": "plan"}),
        )
        .await
        .expect("cold change");
    dispatcher
        .dispatch(
            CHANNELS.set_permission_mode,
            json!({"sessionKey": agent, "mode": "auto"}),
        )
        .await
        .expect("agent change");
    let options = resumed_options(&runtime);
    assert_eq!(options[0].permission_mode.as_deref(), Some("auto"));
    assert_eq!(
        options[1].permission_mode, None,
        "agents keep their own mode"
    );
    drop(dispatcher);

    let runtime = Arc::new(
        FakeRuntime::scripted([
            resumed_ready(thread, "thread"),
            resumed_ready(agent, "agent"),
        ])
        .with_summary(thread, summary("claude", "thread", &cwd))
        .with_summary(agent, agent_summary)
        .with_inactive(thread)
        .with_inactive(agent),
    );
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime))
        .with_session_access(HostSessionAccess::open(data.path()).expect("reopened store"));
    for key in [thread, agent] {
        dispatcher
            .dispatch(CHANNELS.resume_session, json!({"sessionKey": key}))
            .await
            .expect("resume");
    }
    let options = resumed_options(&runtime);
    assert_eq!(options[0].permission_mode.as_deref(), Some("plan"));
    assert_eq!(options[1].permission_mode, None);
}

/// Team tools (protocol 57): a thread team tools started resumes one hop
/// down (collaboration depth 1), so it never gets team tools or the
/// collaboration MCP; every other session resumes as before.
#[tokio::test]
async fn a_thread_team_tools_started_resumes_one_hop_down() {
    struct Children;
    impl crate::dispatcher::TeamChildren for Children {
        fn is_team_child(&self, session_key: &str) -> bool {
            session_key == "claude#child"
        }
    }
    let temporary = tempfile::tempdir().expect("temporary cwd");
    let cwd = temporary.path().to_string_lossy().into_owned();
    let runtime = Arc::new(
        FakeRuntime::scripted([
            resumed_ready("claude#child", "child"),
            resumed_ready("claude#lead", "lead"),
        ])
        .with_summary("claude#child", summary("claude", "child", &cwd))
        .with_summary("claude#lead", summary("claude", "lead", &cwd))
        .with_inactive("claude#child")
        .with_inactive("claude#lead"),
    );
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_team_children(Arc::new(Children));
    for key in ["claude#child", "claude#lead"] {
        dispatcher
            .dispatch(CHANNELS.resume_session, json!({"sessionKey": key}))
            .await
            .expect("resume");
    }
    let options = resumed_options(&runtime);
    assert_eq!(options[0].collaboration_depth, Some(1));
    assert_eq!(options[1].collaboration_depth, None);
}

#[tokio::test]
async fn an_explicit_provider_refresh_finds_the_clis_again_before_it_probes() {
    struct Counting(std::sync::atomic::AtomicUsize);
    impl crate::provider_refresh::ProviderRebuild for Counting {
        fn rebuild(&self) -> Result<interface_terminal::TerminalLaunchCatalog, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(interface_terminal::TerminalLaunchCatalog::shell_only())
        }
    }
    let source = Arc::new(Counting(std::sync::atomic::AtomicUsize::new(0)));
    let runtime = Arc::new(FakeRuntime::scripted(
        (0..3).map(|_| RuntimeBridgeResponse::ProvidersReadiness(Vec::new())),
    ));
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_provider_refresh(Arc::new(
            crate::provider_refresh::ProviderRefresh::new(source.clone()),
        ));
    // The startup probe and a window focus never look again.
    dispatcher
        .dispatch(CHANNELS.probe_engines, json!({}))
        .await
        .unwrap();
    dispatcher
        .dispatch(CHANNELS.probe_engines, Value::Null)
        .await
        .unwrap();
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    // Models & providers → Refresh does, then probes.
    dispatcher
        .dispatch(CHANNELS.probe_engines, json!({"refresh": true}))
        .await
        .unwrap();
    assert_eq!(source.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.commands(),
        vec![RuntimeBridgeCommand::ProbeAllProviders; 3]
    );
    let refused = dispatcher
        .dispatch(CHANNELS.probe_engines, json!({"refresh": "soon"}))
        .await
        .expect_err("a malformed refresh is refused");
    assert!(refused.replay_safe);
}

#[tokio::test]
async fn recovery_receipts_follow_host_delivery_and_never_promote_rejected_work_to_unsent() {
    use interface_contracts::recovery::PromptDelivery;
    use interface_desktop_services::{
        DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts,
    };
    for (disposition, expected) in [
        (
            RuntimeDeliveryDisposition::Confirmed,
            PromptDelivery::Accepted,
        ),
        (
            RuntimeDeliveryDisposition::DefinitelyUnsent,
            PromptDelivery::Unsent,
        ),
        (
            RuntimeDeliveryDisposition::DefinitelyRejected,
            PromptDelivery::Unknown,
        ),
        (
            RuntimeDeliveryDisposition::OutcomeUnknown,
            PromptDelivery::Unknown,
        ),
    ] {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let home = root.path().join("home");
        let state = root.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let desktop = Arc::new(
            DesktopService::open(
                DesktopServiceConfig {
                    appearance_file: state.join("appearance.json"),
                    acp_profiles_file: state.join("acp.json"),
                    terminal_workspaces_file: state.join("terminals.json"),
                    home_directory: home,
                    customization_user_root: None,
                    review_checkpoint_store_path: None,
                },
                DesktopServiceReviewPorts::default(),
            )
            .unwrap(),
        );
        let runtime = Arc::new(FakeRuntime::new(RuntimeBridgeResponse::Delivery(
            RuntimeDeliveryReceipt {
                disposition,
                error: None,
                audit: RuntimeDeliveryAudit::NotApplicable,
            },
        )));
        let dispatcher =
            RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_prompt_recovery(desktop.clone());
        let _=dispatcher.dispatch(CHANNELS.send,json!({"sessionKey":"codex#one","text":"serialized report with attachment","recovery":{"text":"Make the report","context":[{"id":"file","kind":"file","label":"Outline","value":"/project/outline.md"}]}})).await;
        let receipt = desktop
            .prompt_recovery("codex#one")
            .expect("durable receipt");
        assert_eq!(receipt.delivery, expected, "{disposition:?}");
        assert_eq!(receipt.input.text, "Make the report");
        assert_eq!(receipt.input.context[0].value, "/project/outline.md");
    }
    let runtime = Arc::new(FakeRuntime::new(confirmed_delivery()));
    let dispatcher = RuntimeHostDispatcher::new(Arc::clone(&runtime));
    let error=dispatcher.dispatch(CHANNELS.send,json!({"sessionKey":"codex#one","text":"hello","recovery":{"text":"hello","context":[]}})).await.unwrap_err();
    assert!(error.replay_safe);
    assert!(
        runtime.commands().is_empty(),
        "missing receipt storage must refuse before provider I/O"
    );
}

fn launch_recovery_desktop(
    root: &std::path::Path,
) -> Arc<interface_desktop_services::DesktopService> {
    use interface_desktop_services::{
        DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts,
    };
    let home = root.join("home");
    let state = root.join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp.json"),
                terminal_workspaces_file: state.join("terminals.json"),
                home_directory: home,
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .unwrap(),
    )
}

fn launch_recovery_worktrees(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    // A new-worktree retry gets a fresh cwd, but names the same project intent.
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "seed",
        "--quiet",
    ]);
    let retry_worktree = root.join("retry-worktree");
    git(&[
        "worktree",
        "add",
        "--detach",
        retry_worktree.to_str().unwrap(),
    ]);
    (project, retry_worktree)
}

#[tokio::test]
async fn initial_launch_recovery_binds_original_context_and_blocks_ambiguous_recreation() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let (project, retry_worktree) = launch_recovery_worktrees(root.path());
    let desktop = launch_recovery_desktop(root.path());
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/recovery-v49.json"
    ))
    .unwrap();
    let runtime = Arc::new(FakeRuntime::scripted([
        RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::Ready(RuntimeSession {
            key: "claude#created".into(),
            engine: engine("claude"),
            session_id: "created".into(),
            ready_at: "2026-10-14T00:00:00Z".into(),
        })),
        RuntimeBridgeResponse::SessionOpen(SessionOpenOutcome::OutcomeUnknown(RuntimeError::new(
            RuntimeErrorKind::Provider,
            "writer ended after acceptance",
            false,
            EffectCertainty::OutcomeUnknown,
        ))),
    ]));
    let dispatcher =
        RuntimeHostDispatcher::new(runtime.clone()).with_prompt_recovery(desktop.clone());
    let mut request = fixture["launch"].clone();
    request["options"]["cwd"] = json!(project.to_str().unwrap());
    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.create_session, request.clone())
            .await
            .unwrap(),
        json!({"key":"claude#created"})
    );
    let receipt = desktop.prompt_recovery("claude#created").unwrap();
    assert_eq!(receipt.input.text, "Make the report");
    assert_eq!(receipt.input.context[0].value, "/project/sketch.png");
    assert_eq!(
        receipt.delivery,
        interface_contracts::recovery::PromptDelivery::Accepted
    );
    request["options"]["cwd"] = json!(retry_worktree.to_str().unwrap());
    assert!(
        !dispatcher
            .dispatch(CHANNELS.create_session, request.clone())
            .await
            .unwrap_err()
            .replay_safe
    );
    assert_eq!(
        runtime.commands().len(),
        1,
        "accepted launch whose response was lost must not recreate"
    );
    desktop
        .dispatch_blocking(
            CHANNELS.recovery_control,
            json!({"version":1,"kind":"acknowledgeLaunch","key":"claude#created","id":receipt.id}),
        )
        .unwrap();
    // Acknowledgement is issued only after the renderer receives the key. A later
    // intentional launch is admitted, but its unknown outcome remains protected.
    assert!(
        !dispatcher
            .dispatch(CHANNELS.create_session, request.clone())
            .await
            .unwrap_err()
            .replay_safe
    );
    let count = runtime.commands().len();
    let refused = dispatcher
        .dispatch(CHANNELS.create_session, request)
        .await
        .unwrap_err();
    assert!(!refused.replay_safe);
    assert!(refused.message.contains("may already have started"));
    assert_eq!(
        runtime.commands().len(),
        count,
        "ambiguous creation must not reach the provider again"
    );
}

/// Protocol 56: the thread journal's page stands in for a provider history
/// that is unavailable, and only then.
#[tokio::test]
async fn read_history_falls_back_to_the_thread_journal_only_when_unavailable() {
    struct Journal(Mutex<Vec<(String, u64)>>);

    #[async_trait]
    impl HistoryFallback for Journal {
        async fn history(&self, session_key: &str, limit: u64) -> Option<HistoryPage> {
            self.0
                .lock()
                .expect("calls")
                .push((session_key.to_owned(), limit));
            (session_key == "codex#journalled").then(|| {
                as_history_page(vec![EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: "from the journal".into(),
                        agent_key: None,
                    },
                }])
            })
        }
    }

    let journal = Arc::new(Journal(Mutex::new(Vec::new())));
    let runtime = Arc::new(FakeRuntime::scripted([
        RuntimeBridgeResponse::HistorySession(Box::new(None)),
        RuntimeBridgeResponse::HistorySession(Box::new(None)),
    ]));
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_history_fallback(journal.clone());
    let page = dispatcher
        .dispatch(
            CHANNELS.read_history,
            json!({"sessionKey": "codex#journalled"}),
        )
        .await
        .expect("history page");
    assert_eq!(page["events"][0]["text"], "from the journal");
    assert!(page.get("availability").is_none(), "{page}");
    // Nothing journalled: the provider's own answer stays.
    let page = dispatcher
        .dispatch(CHANNELS.read_history, json!({"sessionKey": "codex#other"}))
        .await
        .expect("history page");
    assert_eq!(page["availability"], "unavailable");
    assert_eq!(
        journal.0.lock().expect("calls").clone(),
        [
            ("codex#journalled".to_owned(), 500),
            ("codex#other".to_owned(), 500)
        ]
    );
}

/// T3 section 8: a provider's own history page gains the visuals Interface
/// showed in it; an unavailable one is answered by the journal instead (its
/// replay already holds them), never both.
#[tokio::test]
async fn read_history_adds_the_threads_visuals_to_a_providers_own_page() {
    struct Visuals(Mutex<Vec<String>>);

    #[async_trait]
    impl HistoryFallback for Visuals {
        async fn history(&self, _session_key: &str, _limit: u64) -> Option<HistoryPage> {
            None
        }

        async fn with_visuals(&self, session_key: &str, mut page: HistoryPage) -> HistoryPage {
            self.0.lock().expect("calls").push(session_key.to_owned());
            page.events.push(EngineEvent {
                at: None,
                event: EngineEventKind::VisualShown {
                    id: "v0000000000001-00000000".into(),
                    title: "Chart".into(),
                    bytes: 10,
                },
            });
            page
        }
    }

    let event = HistoryEvent {
        sequence: 0,
        received_at: "2026-08-27T00:00:00Z".into(),
        event: EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "the provider's answer".into(),
                agent_key: None,
            },
        },
    };
    let session = HistorySession {
        key: "codex#one".into(),
        summary: summary("codex", "one", "/project"),
        attachment_count: 1,
        last_attached_at: "2026-08-27T00:00:00Z".into(),
        next_event_sequence: 1,
        next_effect_sequence: 0,
        omitted_events_through: None,
        omitted_effects_through: None,
        stream_state: RuntimeStreamState::Closed,
        delivery_state: RuntimeDeliveryState::Closed,
        terminal_message: None,
        events: [event].into(),
        effects: VecDeque::new(),
    };
    let visuals = Arc::new(Visuals(Mutex::new(Vec::new())));
    let runtime = Arc::new(FakeRuntime::scripted([
        RuntimeBridgeResponse::HistorySession(Box::new(Some(session))),
        RuntimeBridgeResponse::HistorySession(Box::new(None)),
    ]));
    let dispatcher =
        RuntimeHostDispatcher::new(Arc::clone(&runtime)).with_history_fallback(visuals.clone());
    let page = dispatcher
        .dispatch(CHANNELS.read_history, json!({"sessionKey": "codex#one"}))
        .await
        .expect("history page");
    let kinds = page["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["kind"].as_str().expect("kind").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["message.delta", "visual.shown"]);
    // Unavailable: the journal's answer (none here), not a merge.
    let page = dispatcher
        .dispatch(CHANNELS.read_history, json!({"sessionKey": "codex#two"}))
        .await
        .expect("history page");
    assert_eq!(page["availability"], "unavailable");
    assert_eq!(visuals.0.lock().expect("calls").clone(), ["codex#one"]);
}
