use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use interface_contracts::{
    engine::{
        EngineEvent, EngineId, EngineUsage, ProviderCommandCatalogue, ProviderCommandScope,
        SessionOrigin, SessionStatus, SessionSummary, UsageWindow,
    },
    ipc::{CHANNELS, SessionListingSnapshot, SessionSearchResponse},
};
use interface_engine::{EffectCertainty, EngineError, EngineErrorKind};
use interface_runtime::{
    ImportProgress, ImportProgressSink, ImportReport, RuntimeBridgeCommand, RuntimeBridgeResponse,
    RuntimeDeliveryAudit, RuntimeDeliveryDisposition, RuntimeDeliveryReceipt, RuntimeError,
    RuntimeErrorKind, RuntimeSendLiveResponse,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use interface_host::{
    collaboration,
    dispatcher::{
        self, DEFINITELY_REJECTED_CODE, DEFINITELY_UNSENT_CODE, HostDispatcher,
        INVALID_PAYLOAD_CODE, OUTCOME_UNKNOWN_CODE,
    },
    session_management::{
        ADOPT_ALL_OPERATION, IMPORT_PROGRESS_OPERATION, RenameHooks, RenameTicket,
        SEND_LIVE_OPERATION, SESSION_MANAGEMENT_OPERATIONS, SessionManagementHostDispatcher,
        SessionManagementRuntimePort, SupplementalUsageReader,
    },
};

struct FakeSupplementalUsage;

#[async_trait]
impl SupplementalUsageReader for FakeSupplementalUsage {
    fn engine(&self) -> EngineId {
        engine("acp:gemini")
    }

    async fn read_usage(&self) -> EngineUsage {
        EngineUsage {
            engine: engine("acp:gemini"),
            available: true,
            plan: None,
            windows: vec![UsageWindow {
                label: "5-hour".to_owned(),
                utilization: Some(25.0),
                resets_at: Some("2026-08-29T08:15:47Z".to_owned()),
            }],
            cost_usd: None,
            note: Some("Quota reported by Antigravity.".to_owned()),
            account: None,
            account_label: None,
        }
    }
}

struct RejectingCollaborationHooks;

#[derive(Default)]
struct RecordingWorktreeRelease {
    sessions: Mutex<Vec<String>>,
}

impl dispatcher::AutomaticWorktreeRelease for RecordingWorktreeRelease {
    fn schedule_session_release(&self, session_key: String) {
        self.sessions
            .lock()
            .expect("worktree release lock")
            .push(session_key);
    }
}

#[async_trait]
impl collaboration::CollaborationSessionHooks for RejectingCollaborationHooks {
    async fn guard_session_mutation(
        &self,
        _session_key: &str,
        _action: &str,
        _allow_safe_stop: bool,
    ) -> Result<(), dispatcher::DispatchError> {
        Err(dispatcher::DispatchError::new(
            "effect-definitely-unsent",
            "collaboration locked",
            false,
            true,
        ))
    }

    async fn guard_bulk_mutation(&self, _action: &str) -> Result<(), dispatcher::DispatchError> {
        Err(dispatcher::DispatchError::new(
            "effect-definitely-unsent",
            "collaboration locked",
            false,
            true,
        ))
    }

    fn user_send_started(&self, _session_key: &str) -> Result<(), dispatcher::DispatchError> {
        Ok(())
    }

    fn user_send_settled(
        &self,
        _session_key: &str,
        _text: &str,
        _outcome: collaboration::ProjectedSendOutcome,
    ) -> Result<(), dispatcher::DispatchError> {
        Ok(())
    }

    fn observe_event(
        &self,
        _session_key: &str,
        _event: &EngineEvent,
    ) -> Result<(), dispatcher::DispatchError> {
        Ok(())
    }

    fn observe_registry_change(&self, _session_key: &str) -> Result<(), dispatcher::DispatchError> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeRuntime {
    commands: Mutex<Vec<RuntimeBridgeCommand>>,
    next_response: Mutex<Option<RuntimeBridgeResponse>>,
    visible_error: Mutex<Option<RuntimeError>>,
}

impl FakeRuntime {
    fn respond_once(&self, response: RuntimeBridgeResponse) {
        *self.next_response.lock().expect("response lock") = Some(response);
    }

    fn fail_visible_once(&self, error: RuntimeError) {
        *self.visible_error.lock().expect("visible error lock") = Some(error);
    }

    fn recorded_commands(&self) -> Vec<RuntimeBridgeCommand> {
        self.commands.lock().expect("commands lock").clone()
    }
}

#[async_trait]
impl SessionManagementRuntimePort for FakeRuntime {
    async fn handle_management_command(
        &self,
        command: RuntimeBridgeCommand,
        progress: Option<Arc<dyn ImportProgressSink>>,
    ) -> RuntimeBridgeResponse {
        self.commands
            .lock()
            .expect("commands lock")
            .push(command.clone());
        if let Some(response) = self.next_response.lock().expect("response lock").take() {
            return response;
        }
        match command {
            RuntimeBridgeCommand::ListSessionSnapshot => {
                RuntimeBridgeResponse::SessionSnapshot(SessionListingSnapshot {
                    sessions: Vec::new(),
                    providers: Vec::new(),
                })
            }
            RuntimeBridgeCommand::ListArchivedSessions => {
                RuntimeBridgeResponse::ArchivedSessions(Vec::new())
            }
            RuntimeBridgeCommand::ListProviderCommands { scope } => {
                RuntimeBridgeResponse::ProviderCommands(ProviderCommandCatalogue {
                    scope,
                    available: true,
                    commands: Vec::new(),
                    reason: None,
                })
            }
            RuntimeBridgeCommand::ForgetSession { .. }
            | RuntimeBridgeCommand::ArchiveSession { .. }
            | RuntimeBridgeCommand::RenameSession { .. }
            | RuntimeBridgeCommand::DeleteSession { .. }
            | RuntimeBridgeCommand::AdoptSession { .. }
            | RuntimeBridgeCommand::SetEngineArchived { .. } => RuntimeBridgeResponse::Acknowledged,
            RuntimeBridgeCommand::ForkSession { .. } => RuntimeBridgeResponse::ForkedSession {
                session_key: "claude#fork".to_owned(),
            },
            RuntimeBridgeCommand::ReadUsage => RuntimeBridgeResponse::Usage(Vec::new()),
            RuntimeBridgeCommand::EngineOps => RuntimeBridgeResponse::EngineOps(Vec::new()),
            RuntimeBridgeCommand::SearchSessions { .. } => {
                RuntimeBridgeResponse::SessionSearch(SessionSearchResponse {
                    hits: Vec::new(),
                    providers: Vec::new(),
                    sessions_scanned: 0,
                    truncated: false,
                })
            }
            RuntimeBridgeCommand::AdoptAll { adopt } => {
                if let Some(progress) = progress {
                    progress.emit(ImportProgress {
                        adopt,
                        done: 0,
                        total: 1,
                        ok: 0,
                        failed: 0,
                        key: None,
                    });
                    progress.emit(ImportProgress {
                        adopt,
                        done: 1,
                        total: 1,
                        ok: 1,
                        failed: 0,
                        key: Some("claude#one".to_owned()),
                    });
                }
                RuntimeBridgeResponse::ImportReport(ImportReport {
                    adopt,
                    total: 1,
                    ok: 1,
                    local: 0,
                    failed: 0,
                    errors: Vec::new(),
                })
            }
            RuntimeBridgeCommand::SendLive { .. } => {
                RuntimeBridgeResponse::SendLive(RuntimeSendLiveResponse::NotLive)
            }
            command => panic!("unexpected session-management command: {command:?}"),
        }
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        if let Some(error) = self
            .visible_error
            .lock()
            .expect("visible error lock")
            .take()
        {
            return Err(error);
        }
        Ok(vec![sample_summary()])
    }
}

fn engine(value: &str) -> EngineId {
    EngineId::new(value).expect("valid test engine")
}

fn sample_summary() -> SessionSummary {
    SessionSummary {
        engine: engine("claude"),
        session_id: "one".to_owned(),
        title: "First session".to_owned(),
        cwd: "/workspace".to_owned(),
        status: SessionStatus::Idle,
        cloud: false,
        model: Some("sonnet".to_owned()),
        effort: None,
        last_activity: "2026-08-27T12:00:00.000Z".to_owned(),
        first_seen: "2026-08-27T11:00:00.000Z".to_owned(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: Some(SessionOrigin::App),
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn exact_frozen_operation_family_routes_and_preserves_legacy_results() {
    let fake = Arc::new(FakeRuntime::default());
    let dispatcher = SessionManagementHostDispatcher::new(Arc::clone(&fake));
    let scope = ProviderCommandScope {
        engine: engine("claude"),
        cwd: "/workspace".to_owned(),
        model: Some("sonnet".to_owned()),
        setting_sources: Some(vec!["project".to_owned()]),
    };

    assert_eq!(SESSION_MANAGEMENT_OPERATIONS.len(), 15);
    let mut unique = SESSION_MANAGEMENT_OPERATIONS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), SESSION_MANAGEMENT_OPERATIONS.len());

    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.list_session_snapshot, Value::Null)
            .await
            .expect("snapshot"),
        json!({"sessions": [], "providers": []})
    );
    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.list_archived_sessions, json!({}))
            .await
            .expect("archived"),
        json!([])
    );
    assert_eq!(
        dispatcher
            .dispatch(
                CHANNELS.list_provider_commands,
                serde_json::to_value(&scope).expect("scope value"),
            )
            .await
            .expect("commands"),
        json!({
            "scope": {
                "engine": "claude",
                "cwd": "/workspace",
                "model": "sonnet",
                "settingSources": ["project"]
            },
            "available": true,
            "commands": []
        })
    );

    let unit_requests = [
        (CHANNELS.forget_session, json!({"sessionKey": "claude#one"})),
        (
            CHANNELS.archive,
            json!({"sessionKey": "claude#one", "archived": true}),
        ),
        (
            CHANNELS.rename_session,
            json!({"sessionKey": "claude#one", "title": "Renamed"}),
        ),
        (CHANNELS.delete_session, json!({"sessionKey": "claude#one"})),
        (
            CHANNELS.adopt_session,
            json!({"sessionKey": "claude#one", "adopt": true}),
        ),
        (
            CHANNELS.set_engine_archived,
            json!({"sessionKey": "claude#one", "archived": true}),
        ),
    ];
    for (operation, payload) in unit_requests {
        assert_eq!(
            dispatcher
                .dispatch(operation, payload)
                .await
                .expect("unit result"),
            Value::Null
        );
    }

    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.fork_session, json!({"sessionKey": "claude#one"}),)
            .await
            .expect("fork"),
        json!({"key": "claude#fork"})
    );
    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.read_usage, json!({}))
            .await
            .expect("usage"),
        json!([])
    );
    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.engine_ops, Value::Null)
            .await
            .expect("engine ops"),
        json!([])
    );
    assert_eq!(
        dispatcher
            .dispatch(CHANNELS.search_sessions, json!({"query": "parser"}))
            .await
            .expect("search"),
        json!({
            "hits": [],
            "providers": [],
            "sessionsScanned": 0,
            "truncated": false
        })
    );
    assert_eq!(
        dispatcher
            .dispatch(ADOPT_ALL_OPERATION, json!({"adopt": true}))
            .await
            .expect("adopt all"),
        json!({
            "adopt": true,
            "total": 1,
            "ok": 1,
            "local": 0,
            "failed": 0,
            "errors": []
        })
    );
    assert_eq!(
        dispatcher
            .dispatch(
                SEND_LIVE_OPERATION,
                json!({"sessionKey": "claude#one", "text": "hello"}),
            )
            .await
            .expect("send live"),
        Value::Bool(false)
    );

    assert_eq!(
        fake.recorded_commands(),
        vec![
            RuntimeBridgeCommand::ListSessionSnapshot,
            RuntimeBridgeCommand::ListArchivedSessions,
            RuntimeBridgeCommand::ListProviderCommands { scope },
            RuntimeBridgeCommand::ForgetSession {
                session_key: "claude#one".to_owned(),
            },
            RuntimeBridgeCommand::ArchiveSession {
                session_key: "claude#one".to_owned(),
                archived: true,
            },
            RuntimeBridgeCommand::RenameSession {
                session_key: "claude#one".to_owned(),
                title: "Renamed".to_owned(),
            },
            RuntimeBridgeCommand::DeleteSession {
                session_key: "claude#one".to_owned(),
            },
            RuntimeBridgeCommand::AdoptSession {
                session_key: "claude#one".to_owned(),
                adopt: true,
            },
            RuntimeBridgeCommand::SetEngineArchived {
                session_key: "claude#one".to_owned(),
                archived: true,
            },
            RuntimeBridgeCommand::ForkSession {
                session_key: "claude#one".to_owned(),
            },
            RuntimeBridgeCommand::ReadUsage,
            RuntimeBridgeCommand::EngineOps,
            RuntimeBridgeCommand::SearchSessions {
                query: "parser".to_owned(),
            },
            RuntimeBridgeCommand::AdoptAll { adopt: true },
            RuntimeBridgeCommand::SendLive {
                session_key: "claude#one".to_owned(),
                text: "hello".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn supplemental_usage_is_appended_to_successful_runtime_usage() {
    let fake = Arc::new(FakeRuntime::default());
    fake.respond_once(RuntimeBridgeResponse::Usage(vec![EngineUsage {
        engine: engine("acp:grok"),
        available: true,
        plan: Some("supergrok".to_owned()),
        windows: Vec::new(),
        cost_usd: None,
        note: None,
        account: None,
        account_label: None,
    }]));
    let dispatcher = SessionManagementHostDispatcher::new(fake)
        .with_supplemental_usage(vec![Arc::new(FakeSupplementalUsage)]);

    let rows = dispatcher
        .dispatch(CHANNELS.read_usage, json!({}))
        .await
        .expect("usage response");
    let rows = rows.as_array().expect("usage rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["engine"], "acp:gemini");
    assert_eq!(rows[0]["windows"][0]["utilization"], 25.0);
    assert_eq!(rows[1]["engine"], "acp:grok");
}

#[tokio::test]
async fn supplemental_usage_preserves_a_runtime_engine_collision() {
    let fake = Arc::new(FakeRuntime::default());
    fake.respond_once(RuntimeBridgeResponse::Usage(vec![
        EngineUsage {
            engine: engine("acp:grok"),
            available: true,
            plan: Some("supergrok".to_owned()),
            windows: Vec::new(),
            cost_usd: None,
            note: None,
            account: None,
            account_label: None,
        },
        EngineUsage {
            engine: engine("acp:gemini"),
            available: false,
            plan: None,
            windows: Vec::new(),
            cost_usd: None,
            note: Some("stale duplicate".to_owned()),
            account: None,
            account_label: None,
        },
    ]));
    let dispatcher = SessionManagementHostDispatcher::new(fake)
        .with_supplemental_usage(vec![Arc::new(FakeSupplementalUsage)]);

    let rows = dispatcher
        .dispatch(CHANNELS.read_usage, json!({}))
        .await
        .expect("usage response");
    assert_eq!(
        rows,
        json!([
            {
                "engine": "acp:gemini",
                "available": false,
                "windows": [],
                "note": "stale duplicate"
            },
            {
                "engine": "acp:grok",
                "available": true,
                "plan": "supergrok",
                "windows": []
            }
        ])
    );
}

#[tokio::test]
async fn adopt_all_pushes_bounded_progress_then_the_canonical_registry() {
    let fake = Arc::new(FakeRuntime::default());
    let (pushes, mut received) = mpsc::channel(8);
    let dispatcher = SessionManagementHostDispatcher::with_pushes(fake, pushes);

    dispatcher
        .dispatch(ADOPT_ALL_OPERATION, json!({"adopt": true}))
        .await
        .expect("adopt all");

    let started = received.recv().await.expect("start progress");
    assert_eq!(started.operation, IMPORT_PROGRESS_OPERATION);
    assert_eq!(
        started.payload,
        json!({"adopt": true, "done": 0, "total": 1, "ok": 0, "failed": 0})
    );
    let settled = received.recv().await.expect("settled progress");
    assert_eq!(settled.operation, IMPORT_PROGRESS_OPERATION);
    assert_eq!(
        settled.payload,
        json!({
            "adopt": true,
            "done": 1,
            "total": 1,
            "ok": 1,
            "failed": 0,
            "key": "claude#one"
        })
    );
    let registry = received.recv().await.expect("registry push");
    assert_eq!(registry.operation, CHANNELS.registry);
    assert_eq!(
        registry.payload,
        serde_json::to_value(vec![sample_summary()]).expect("registry payload")
    );
    assert!(matches!(
        received.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn first_listing_paints_the_cached_registry_before_answering_once() {
    let fake = Arc::new(FakeRuntime::default());
    let (pushes, mut received) = mpsc::channel(8);
    let dispatcher = SessionManagementHostDispatcher::with_pushes(Arc::clone(&fake), pushes);

    dispatcher
        .dispatch(CHANNELS.list_session_snapshot, Value::Null)
        .await
        .expect("first snapshot");
    // Queued before the response completed, so it reaches the desktop first.
    let cached = received.try_recv().expect("cached registry push");
    assert_eq!(cached.operation, CHANNELS.registry);
    assert_eq!(
        cached.payload,
        serde_json::to_value(vec![sample_summary()]).expect("registry payload")
    );

    // Focus refreshes answer from the listing alone.
    dispatcher
        .dispatch(CHANNELS.list_session_snapshot, Value::Null)
        .await
        .expect("second snapshot");
    assert!(matches!(
        received.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(fake.recorded_commands().len(), 2);
}

#[tokio::test]
async fn successful_forget_schedules_the_frozen_managed_worktree_release() {
    let fake = Arc::new(FakeRuntime::default());
    let releases = Arc::new(RecordingWorktreeRelease::default());
    let (pushes, mut received) = mpsc::channel(4);
    let dispatcher = SessionManagementHostDispatcher::with_pushes(fake, pushes)
        .with_worktree_release(releases.clone());

    dispatcher
        .dispatch(CHANNELS.forget_session, json!({"sessionKey": "claude#one"}))
        .await
        .expect("forget");

    assert_eq!(
        received.recv().await.expect("registry push").operation,
        CHANNELS.registry
    );
    assert_eq!(
        releases
            .sessions
            .lock()
            .expect("worktree release calls")
            .as_slice(),
        &["claude#one"]
    );
}

#[tokio::test]
async fn runtime_errors_retain_effect_certainty_and_safe_replay_contract() {
    let cases = [
        (
            EffectCertainty::DefinitelyUnsent,
            DEFINITELY_UNSENT_CODE,
            true,
        ),
        (
            EffectCertainty::DefinitelyRejected,
            DEFINITELY_REJECTED_CODE,
            false,
        ),
        (EffectCertainty::OutcomeUnknown, OUTCOME_UNKNOWN_CODE, false),
    ];
    for (certainty, expected_code, replay_safe) in cases {
        let fake = Arc::new(FakeRuntime::default());
        fake.respond_once(RuntimeBridgeResponse::Error(RuntimeError::new(
            RuntimeErrorKind::Provider,
            "sanitized provider failure",
            true,
            certainty,
        )));
        let dispatcher = SessionManagementHostDispatcher::new(fake);
        let error = dispatcher
            .dispatch(
                CHANNELS.rename_session,
                json!({"sessionKey": "claude#one", "title": "Renamed"}),
            )
            .await
            .expect_err("runtime failure");
        assert_eq!(error.code, expected_code);
        assert_eq!(error.message, "sanitized provider failure");
        assert!(error.retryable);
        assert_eq!(error.replay_safe, replay_safe);
    }
}

#[tokio::test]
async fn send_live_maps_confirmed_and_uncertain_delivery_without_blind_replay() {
    let fake = Arc::new(FakeRuntime::default());
    fake.respond_once(RuntimeBridgeResponse::SendLive(
        RuntimeSendLiveResponse::Delivery(RuntimeDeliveryReceipt {
            disposition: RuntimeDeliveryDisposition::Confirmed,
            error: None,
            audit: RuntimeDeliveryAudit::NotApplicable,
        }),
    ));
    let dispatcher = SessionManagementHostDispatcher::new(Arc::clone(&fake));
    assert_eq!(
        dispatcher
            .dispatch(
                SEND_LIVE_OPERATION,
                json!({"sessionKey": "claude#one", "text": "hello"}),
            )
            .await
            .expect("confirmed delivery"),
        Value::Bool(true)
    );

    fake.respond_once(RuntimeBridgeResponse::SendLive(
        RuntimeSendLiveResponse::Delivery(RuntimeDeliveryReceipt {
            disposition: RuntimeDeliveryDisposition::OutcomeUnknown,
            error: Some(EngineError::outcome_unknown(
                EngineErrorKind::Transport,
                "delivery acknowledgement was lost",
                true,
            )),
            audit: RuntimeDeliveryAudit::NotApplicable,
        }),
    ));
    let error = dispatcher
        .dispatch(
            SEND_LIVE_OPERATION,
            json!({"sessionKey": "claude#one", "text": "hello again"}),
        )
        .await
        .expect_err("uncertain delivery");
    assert_eq!(error.code, OUTCOME_UNKNOWN_CODE);
    assert_eq!(error.message, "delivery acknowledgement was lost");
    assert!(error.retryable);
    assert!(!error.replay_safe);
}

#[tokio::test]
async fn a_failed_post_mutation_push_is_never_reported_as_safe_to_replay() {
    let fake = Arc::new(FakeRuntime::default());
    fake.fail_visible_once(RuntimeError::definitely_unsent(
        RuntimeErrorKind::Registry,
        "registry read failed",
        true,
    ));
    let (pushes, _received) = mpsc::channel(1);
    let dispatcher = SessionManagementHostDispatcher::with_pushes(fake, pushes);
    let error = dispatcher
        .dispatch(
            CHANNELS.rename_session,
            json!({"sessionKey": "claude#one", "title": "Renamed"}),
        )
        .await
        .expect_err("registry push failure");
    assert_eq!(error.code, OUTCOME_UNKNOWN_CODE);
    assert!(!error.retryable);
    assert!(!error.replay_safe);
    assert_eq!(
        error.message,
        "The session changed, but its registry update could not reach the desktop."
    );
}

/// Records each owner rename the hooks saw: the session, then how it settled.
#[derive(Default)]
struct RecordingRenameHooks {
    seen: Arc<Mutex<Vec<String>>>,
}

struct RecordingRenameTicket {
    key: String,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl RenameHooks for RecordingRenameHooks {
    async fn before_rename(&self, session_key: &str) -> Box<dyn RenameTicket> {
        self.seen
            .lock()
            .expect("rename hook lock")
            .push(format!("before {session_key}"));
        Box::new(RecordingRenameTicket {
            key: session_key.to_owned(),
            seen: Arc::clone(&self.seen),
        })
    }
}

impl RenameTicket for RecordingRenameTicket {
    fn settle(self: Box<Self>, renamed: bool) {
        self.seen
            .lock()
            .expect("rename hook lock")
            .push(format!("{} {renamed}", self.key));
    }
}

#[tokio::test]
async fn rename_hooks_wrap_an_owner_rename_and_learn_whether_it_may_have_happened() {
    let fake = Arc::new(FakeRuntime::default());
    let hooks = Arc::new(RecordingRenameHooks::default());
    let dispatcher =
        SessionManagementHostDispatcher::new(Arc::clone(&fake)).with_rename_hooks(hooks.clone());
    let rename = |key: &str| json!({"sessionKey": key, "title": "Renamed"});
    dispatcher
        .dispatch(CHANNELS.rename_session, rename("claude#done"))
        .await
        .expect("renamed");
    // A definitely-unsent failure did not rename; an unknown outcome may have.
    for (key, certainty) in [
        ("claude#unsent", EffectCertainty::DefinitelyUnsent),
        ("claude#unknown", EffectCertainty::OutcomeUnknown),
    ] {
        fake.respond_once(RuntimeBridgeResponse::Error(RuntimeError::new(
            RuntimeErrorKind::Provider,
            "sanitized provider failure",
            false,
            certainty,
        )));
        dispatcher
            .dispatch(CHANNELS.rename_session, rename(key))
            .await
            .expect_err("rename failure");
    }
    // Other operations never reach the hooks.
    dispatcher
        .dispatch(
            CHANNELS.archive,
            json!({"sessionKey": "claude#done", "archived": true}),
        )
        .await
        .expect("archived");
    assert_eq!(
        hooks.seen.lock().expect("rename hook lock").clone(),
        [
            "before claude#done",
            "claude#done true",
            "before claude#unsent",
            "claude#unsent false",
            "before claude#unknown",
            "claude#unknown true",
        ]
    );
}

#[tokio::test]
async fn collaboration_guard_refuses_mutation_before_runtime_contact() {
    let fake = Arc::new(FakeRuntime::default());
    let dispatcher = SessionManagementHostDispatcher::new(Arc::clone(&fake))
        .with_collaboration_hooks(Arc::new(RejectingCollaborationHooks));
    let error = dispatcher
        .dispatch(
            CHANNELS.archive,
            json!({"sessionKey": "claude#one", "archived": true}),
        )
        .await
        .expect_err("collaboration lock");
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert!(fake.recorded_commands().is_empty());
}

#[tokio::test]
async fn strict_payloads_and_operation_ownership_fail_closed() {
    let dispatcher = SessionManagementHostDispatcher::new(Arc::new(FakeRuntime::default()));
    let invalid = [
        (CHANNELS.list_session_snapshot, json!([])),
        (
            CHANNELS.archive,
            json!({"sessionKey": "claude#one", "archived": true, "extra": 1}),
        ),
        (CHANNELS.search_sessions, Value::Null),
        (
            SEND_LIVE_OPERATION,
            json!({"sessionKey": "claude#one", "text": 7}),
        ),
    ];
    for (operation, payload) in invalid {
        let error = dispatcher
            .dispatch(operation, payload)
            .await
            .expect_err("invalid payload");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
        assert!(!error.retryable);
        assert!(error.replay_safe);
    }

    let error = dispatcher
        .dispatch("iface:createSession", json!({}))
        .await
        .expect_err("unowned operation");
    assert_eq!(error.code, "operation-not-owned");
    assert!(error.replay_safe);
}

/// The permission modes a host keeps, by session key.
#[derive(Default)]
struct FakeAccess(Mutex<std::collections::HashMap<String, String>>);

impl dispatcher::SessionAccessMemory for FakeAccess {
    fn mode(&self, session_key: &str) -> Option<String> {
        self.0
            .lock()
            .expect("access lock")
            .get(session_key)
            .cloned()
    }

    fn remember(&self, session_key: &str, mode: Option<&str>) {
        let mut modes = self.0.lock().expect("access lock");
        match mode {
            Some(mode) => modes.insert(session_key.to_owned(), mode.to_owned()),
            None => modes.remove(session_key),
        };
    }
}

/// Protocol 68: the listing and the cached registry it paints carry each
/// session's permission mode as the host keeps it (`default` when it keeps
/// none), so a reloaded window shows the mode a thread really runs in.
/// Sessions without permission controls (Chat, Voice) and agents' sessions
/// carry none, and nothing is added when the host keeps no modes at all.
#[tokio::test]
async fn listings_and_the_cached_registry_carry_the_mode_the_host_keeps() {
    let listing = || {
        let mut chat = sample_summary();
        chat.session_id = "chat".to_owned();
        chat.kind = Some(interface_contracts::engine::SessionKind::Chat);
        let mut agent = sample_summary();
        agent.session_id = "agent".to_owned();
        agent.kind = Some(interface_contracts::engine::SessionKind::Agent);
        let mut unknown = sample_summary();
        unknown.session_id = "unknown".to_owned();
        RuntimeBridgeResponse::SessionSnapshot(SessionListingSnapshot {
            sessions: vec![sample_summary(), unknown, chat, agent],
            providers: Vec::new(),
        })
    };
    let access = Arc::new(FakeAccess::default());
    for (key, mode) in [
        ("claude#one", "acceptEdits"),
        ("claude#chat", "auto"),
        ("claude#agent", "auto"),
    ] {
        dispatcher::SessionAccessMemory::remember(access.as_ref(), key, Some(mode));
    }
    let fake = Arc::new(FakeRuntime::default());
    fake.respond_once(listing());
    let (pushes, mut received) = mpsc::channel(8);
    let dispatcher = SessionManagementHostDispatcher::with_pushes(Arc::clone(&fake), pushes)
        .with_session_access(access);

    let answered = dispatcher
        .dispatch(CHANNELS.list_session_snapshot, Value::Null)
        .await
        .expect("snapshot");
    let modes = answered["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| session.get("permissionMode").cloned())
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            Some(json!("acceptEdits")),
            Some(json!("default")),
            None,
            None
        ],
        "{answered}"
    );
    let cached = received.try_recv().expect("cached registry push");
    assert_eq!(cached.payload[0]["permissionMode"], "acceptEdits");

    // A host that keeps no modes says nothing.
    let fake = Arc::new(FakeRuntime::default());
    fake.respond_once(listing());
    let dispatcher = SessionManagementHostDispatcher::new(Arc::clone(&fake));
    let answered = dispatcher
        .dispatch(CHANNELS.list_session_snapshot, Value::Null)
        .await
        .expect("snapshot");
    assert!(
        answered["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .all(|session| session.get("permissionMode").is_none()),
        "{answered}"
    );
}
