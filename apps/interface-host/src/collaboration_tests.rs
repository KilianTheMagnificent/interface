#![allow(clippy::too_many_lines)]

use std::{
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use interface_collaboration::{
    CollaborationCoordinator, CollaborationJournal, CollaborationModelIngress, CoordinatorError,
    EffectSuccess, PreparedTarget,
    state::{
        CollaborationEffectKind, CollaborationLiveStatus, CollaborationPolicyOverrides,
        CollaborationWorktreeV1, UnmanagedWorktreeV1,
    },
};
use interface_contracts::{
    engine::{EngineEventKind, EngineId, SessionStatus, SessionSummary},
    ipc::{CollaborationEventDto, HISTORY_MAX_LIMIT},
};
use interface_runtime::SessionRuntime;
use serde_json::json;
use tempfile::TempDir;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::port::{SourceRegistryFingerprint, push_bounded_utf8};
use super::service::{DriveState, expire_pending_approvals_and_publish};
use super::*;

const SOURCE_KEY: &str = "claude#source-1";
const RUNTIME_EPOCH: &str = "runtime-epoch-1";

fn sample_status() -> CollaborationStatusDto {
    CollaborationStatusDto {
        receipt: CollaborationReceiptDto {
            version: 1,
            collaboration_id: "collaboration-1".to_owned(),
            revision: 7,
            state: CollaborationState::AwaitingApproval,
            mode: CollaborationMode::Consult,
            source_engine: CollaborationEngine::Claude,
            target_engine: CollaborationEngine::Codex,
            approval_required: true,
            cancellation_safe: true,
            created_at: "2026-08-27T12:00:00.000Z".to_owned(),
            updated_at: "2026-08-27T12:00:01.000Z".to_owned(),
            error: None,
        },
        source_session_key: SOURCE_KEY.to_owned(),
        target_session_key: None,
        approval: None,
        peer: None,
    }
}

struct FakeControl {
    calls: StdMutex<Vec<String>>,
    status: CollaborationStatusDto,
}

impl FakeControl {
    fn new() -> Self {
        Self {
            calls: StdMutex::new(Vec::new()),
            status: sample_status(),
        }
    }

    fn record(&self, call: String) {
        self.calls.lock().expect("fake control calls").push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("fake control calls").clone()
    }
}

#[async_trait]
impl CollaborationDesktopControl for FakeControl {
    async fn list(&self) -> Result<Vec<CollaborationStatusDto>, CollaborationServiceError> {
        self.record("list".to_owned());
        Ok(vec![self.status.clone()])
    }

    async fn submit(
        &self,
        input: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!(
            "submit:{}:{:?}:{:?}:{}:{}",
            input.source_session_key,
            input.mode,
            input.target_engine,
            input.objective,
            input.client_request_id
        ));
        Ok(self.status.clone())
    }

    async fn status(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!("status:{collaboration_id}"));
        Ok(self.status.clone())
    }

    async fn approve(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!(
            "approve:{}:{}:{}",
            action.collaboration_id, action.approval_id, action.expected_revision
        ));
        Ok(self.status.clone())
    }

    async fn deny(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!(
            "deny:{}:{}:{}",
            action.collaboration_id, action.approval_id, action.expected_revision
        ));
        Ok(self.status.clone())
    }

    async fn accept_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!(
            "accept:{}:{}:{}:{}",
            action.collaboration_id,
            action.result_id,
            action.result_digest,
            action.expected_revision
        ));
        Ok(self.status.clone())
    }

    async fn discard_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!(
            "discard:{}:{}:{}:{}",
            action.collaboration_id,
            action.result_id,
            action.result_digest,
            action.expected_revision
        ));
        Ok(self.status.clone())
    }

    async fn cancel(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.record(format!("cancel:{collaboration_id}"));
        Ok(self.status.clone())
    }

    fn set_draft_state(
        &self,
        update: CollaborationDraftStateUpdate,
    ) -> Result<(), CollaborationServiceError> {
        self.record(format!(
            "draft:{}:{}:{}",
            update.source_session_key, update.revision, update.empty
        ));
        Ok(())
    }

    async fn close(&self) -> Result<(), CollaborationServiceError> {
        self.record("close".to_owned());
        Ok(())
    }
}

fn valid_payload(operation: &str) -> Value {
    match operation {
        "iface:listCollaborations" => Value::Null,
        "iface:submitCollaboration" => json!({
            "sourceSessionKey": SOURCE_KEY,
            "mode": "consult",
            "targetEngine": "codex",
            "objective": "Review this change",
            "clientRequestId": "request-1"
        }),
        "iface:getCollaborationStatus" | "iface:cancelCollaboration" => {
            json!({"collaborationId": "collaboration-1"})
        }
        "iface:approveCollaboration" | "iface:denyCollaboration" => json!({
            "collaborationId": "collaboration-1",
            "approvalId": "approval-1",
            "expectedRevision": 7
        }),
        "iface:acceptCollaborationPeer" | "iface:discardCollaborationPeer" => json!({
            "collaborationId": "collaboration-1",
            "resultId": "result-1",
            "resultDigest": "digest-1",
            "expectedRevision": 7
        }),
        "iface:setCollaborationDraftState" => json!({
            "sourceSessionKey": SOURCE_KEY,
            "revision": 7,
            "empty": true
        }),
        _ => panic!("unknown collaboration test operation"),
    }
}

#[tokio::test]
async fn dispatcher_maps_all_nine_frozen_operations_exactly() {
    let control = Arc::new(FakeControl::new());
    let dispatcher = CollaborationHostDispatcher::new(control.clone());
    let status = serde_json::to_value(sample_status()).expect("status JSON");

    for operation in COLLABORATION_OPERATIONS {
        let actual = dispatcher
            .dispatch(operation, valid_payload(operation))
            .await
            .expect("dispatch succeeds");
        let expected = if operation == CHANNELS.list_collaborations {
            Value::Array(vec![status.clone()])
        } else if operation == CHANNELS.set_collaboration_draft_state {
            Value::Null
        } else {
            status.clone()
        };
        assert_eq!(actual, expected, "operation {operation}");
    }
    dispatcher.close().await.expect("close");
    assert_eq!(
        control.calls(),
        [
            "list",
            "submit:claude#source-1:Consult:Codex:Review this change:request-1",
            "status:collaboration-1",
            "approve:collaboration-1:approval-1:7",
            "deny:collaboration-1:approval-1:7",
            "accept:collaboration-1:result-1:digest-1:7",
            "discard:collaboration-1:result-1:digest-1:7",
            "cancel:collaboration-1",
            "draft:claude#source-1:7:true",
            "close",
        ]
    );
}

#[tokio::test]
async fn unavailable_control_preserves_exact_error_for_all_nine_operations() {
    let dispatcher = CollaborationHostDispatcher::unavailable();
    for operation in COLLABORATION_OPERATIONS {
        let error = dispatcher
            .dispatch(operation, valid_payload(operation))
            .await
            .expect_err("unavailable operation");
        assert_eq!(error.code, "collaboration-unavailable");
        assert_eq!(error.message, UNAVAILABLE_MESSAGE);
        assert!(error.retryable);
        assert!(error.replay_safe);
    }
}

#[tokio::test]
async fn dispatcher_rejects_noncanonical_payloads_before_control_dispatch() {
    let control = Arc::new(FakeControl::new());
    let dispatcher = CollaborationHostDispatcher::new(control.clone());
    let invalid = [
        (CHANNELS.list_collaborations, json!({"extra": true})),
        (
            CHANNELS.submit_collaboration,
            json!({
                "sourceSessionKey": "unknown#source-1",
                "mode": "consult",
                "targetEngine": "codex",
                "objective": "Review",
                "clientRequestId": "request-1"
            }),
        ),
        (
            CHANNELS.submit_collaboration,
            json!({
                "sourceSessionKey": SOURCE_KEY,
                "mode": "consult",
                "targetEngine": "codex",
                "objective": "   ",
                "clientRequestId": "request-1"
            }),
        ),
        (
            CHANNELS.submit_collaboration,
            json!({
                "sourceSessionKey": SOURCE_KEY,
                "mode": "consult",
                "targetEngine": "codex",
                "objective": "Review",
                "clientRequestId": "request-1",
                "extra": true
            }),
        ),
        (
            CHANNELS.get_collaboration_status,
            json!({"collaborationId": " collaboration-1"}),
        ),
        (
            CHANNELS.approve_collaboration,
            json!({
                "collaborationId": "collaboration-1",
                "approvalId": "approval-1",
                "expectedRevision": interface_collaboration::state::JS_MAX_SAFE_INTEGER + 1
            }),
        ),
        (
            CHANNELS.accept_collaboration_peer,
            json!({
                "collaborationId": "collaboration-1",
                "resultId": "result-1",
                "resultDigest": "bad\nvalue",
                "expectedRevision": 7
            }),
        ),
        (
            CHANNELS.cancel_collaboration,
            json!({"collaborationId": ""}),
        ),
        (
            CHANNELS.set_collaboration_draft_state,
            json!({
                "sourceSessionKey": SOURCE_KEY,
                "revision": interface_collaboration::state::JS_MAX_SAFE_INTEGER + 1,
                "empty": true
            }),
        ),
    ];
    for (operation, payload) in invalid {
        let error = dispatcher
            .dispatch(operation, payload)
            .await
            .expect_err("invalid request");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
        assert_eq!(error.message, INVALID_REQUEST_MESSAGE);
        assert!(!error.retryable);
        assert!(error.replay_safe);
    }
    assert!(control.calls().is_empty());
}

#[test]
fn renderer_state_and_error_names_cover_every_domain_variant() {
    let states = [
        (DomainState::Requested, CollaborationState::Requested),
        (
            DomainState::AwaitingApproval,
            CollaborationState::AwaitingApproval,
        ),
        (DomainState::Approved, CollaborationState::Approved),
        (DomainState::Preflight, CollaborationState::Preflight),
        (
            DomainState::PreparingTarget,
            CollaborationState::PreparingTarget,
        ),
        (
            DomainState::RunningTarget,
            CollaborationState::RunningTarget,
        ),
        (DomainState::ResultReady, CollaborationState::ResultReady),
        (
            DomainState::AwaitingPeerReview,
            CollaborationState::AwaitingPeerReview,
        ),
        (
            DomainState::DeliveryPending,
            CollaborationState::DeliveryPending,
        ),
        (
            DomainState::ParkingSource,
            CollaborationState::ParkingSource,
        ),
        (
            DomainState::OwnershipTransfer,
            CollaborationState::OwnershipTransfer,
        ),
        (
            DomainState::ActivatingTarget,
            CollaborationState::ActivatingTarget,
        ),
        (DomainState::Completed, CollaborationState::Completed),
        (DomainState::Denied, CollaborationState::Denied),
        (DomainState::Cancelled, CollaborationState::Cancelled),
        (DomainState::Failed, CollaborationState::Failed),
        (DomainState::Unknown, CollaborationState::Unknown),
    ];
    for (domain, renderer) in states {
        assert_eq!(state_dto(domain), renderer);
    }

    let errors = [
        (CollaborationErrorCode::SourceNotFound, "SOURCE_NOT_FOUND"),
        (CollaborationErrorCode::SourceNotLive, "SOURCE_NOT_LIVE"),
        (CollaborationErrorCode::SourceNotIdle, "SOURCE_NOT_IDLE"),
        (
            CollaborationErrorCode::TargetNotSupported,
            "TARGET_NOT_SUPPORTED",
        ),
        (CollaborationErrorCode::TargetUnhealthy, "TARGET_UNHEALTHY"),
        (
            CollaborationErrorCode::ApprovalRequired,
            "APPROVAL_REQUIRED",
        ),
        (CollaborationErrorCode::ApprovalExpired, "APPROVAL_EXPIRED"),
        (CollaborationErrorCode::ApprovalStale, "APPROVAL_STALE"),
        (CollaborationErrorCode::Denied, "DENIED"),
        (
            CollaborationErrorCode::PendingSourceApproval,
            "PENDING_SOURCE_APPROVAL",
        ),
        (
            CollaborationErrorCode::SourceQueueNotEmpty,
            "SOURCE_QUEUE_NOT_EMPTY",
        ),
        (
            CollaborationErrorCode::SourceDeliveryUnknown,
            "SOURCE_DELIVERY_UNKNOWN",
        ),
        (
            CollaborationErrorCode::ManagedWorktreeTransferUnavailable,
            "MANAGED_WORKTREE_TRANSFER_UNAVAILABLE",
        ),
        (
            CollaborationErrorCode::WorktreeOwnerChanged,
            "WORKTREE_OWNER_CHANGED",
        ),
        (
            CollaborationErrorCode::IdempotencyConflict,
            "IDEMPOTENCY_CONFLICT",
        ),
        (CollaborationErrorCode::LimitExceeded, "LIMIT_EXCEEDED"),
        (
            CollaborationErrorCode::BrokerUnavailable,
            "BROKER_UNAVAILABLE",
        ),
        (
            CollaborationErrorCode::ProviderOutcomeUnknown,
            "PROVIDER_OUTCOME_UNKNOWN",
        ),
        (
            CollaborationErrorCode::SourceParkUnknown,
            "SOURCE_PARK_UNKNOWN",
        ),
        (CollaborationErrorCode::DeliveryUnknown, "DELIVERY_UNKNOWN"),
        (CollaborationErrorCode::StateCorrupt, "STATE_CORRUPT"),
        (
            CollaborationErrorCode::RecoveryRequired,
            "RECOVERY_REQUIRED",
        ),
    ];
    for (code, name) in errors {
        assert_eq!(code.as_str(), name);
    }
}

#[test]
fn projection_is_fail_closed_and_enforces_peer_identity() {
    let projection = DesktopCollaborationProjection::new();
    let initial = projection.source_state(SOURCE_KEY).expect("initial state");
    assert_eq!(initial.queue_depth, 1, "unknown draft is not empty");
    projection.activate_session(SOURCE_KEY).expect("activate");
    projection
        .set_draft_state(&CollaborationDraftStateUpdate {
            source_session_key: SOURCE_KEY.to_owned(),
            revision: 1,
            empty: true,
        })
        .expect("fresh empty draft");
    assert_eq!(
        projection
            .source_state(SOURCE_KEY)
            .expect("empty")
            .queue_depth,
        0
    );
    let stale = projection
        .set_draft_state(&CollaborationDraftStateUpdate {
            source_session_key: SOURCE_KEY.to_owned(),
            revision: 1,
            empty: false,
        })
        .expect_err("stale draft");
    assert_eq!(stale.code, "collaboration-draft-revision-stale");

    projection
        .user_send_started(SOURCE_KEY)
        .expect("send start");
    projection
        .user_send_settled(
            SOURCE_KEY,
            "hello\u{1b}[31m world",
            ProjectedSendOutcome::AdapterAccepted,
        )
        .expect("accepted send");
    assert_eq!(
        projection
            .live_transcript(SOURCE_KEY)
            .expect("events")
            .len(),
        1
    );
    projection
        .set_draft_state(&CollaborationDraftStateUpdate {
            source_session_key: SOURCE_KEY.to_owned(),
            revision: 2,
            empty: true,
        })
        .expect("empty after send");
    projection
        .observe_event(
            SOURCE_KEY,
            &EngineEvent {
                at: None,
                event: EngineEventKind::ApprovalRequested {
                    approval_id: "approval-z".to_owned(),
                    tool: "shell".to_owned(),
                    summary: "run".to_owned(),
                    input: Value::Null,
                },
            },
        )
        .expect("approval projection");
    assert_eq!(
        projection
            .source_state(SOURCE_KEY)
            .expect("approval state")
            .pending_approval_count,
        1
    );
    projection
        .observe_event(
            SOURCE_KEY,
            &EngineEvent {
                at: None,
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            },
        )
        .expect("idle projection");
    assert_eq!(
        projection
            .source_state(SOURCE_KEY)
            .expect("cleared state")
            .pending_approval_count,
        0
    );
    projection
        .observe_event(
            SOURCE_KEY,
            &EngineEvent {
                at: None,
                event: EngineEventKind::MessageDelta {
                    text: "token".to_owned(),
                    agent_key: None,
                },
            },
        )
        .expect("projected delta");
    assert!(
        !projection
            .live_transcript(SOURCE_KEY)
            .expect("live")
            .is_empty()
    );
    projection
        .observe_event(
            SOURCE_KEY,
            &EngineEvent {
                at: None,
                event: EngineEventKind::SessionClosed,
            },
        )
        .expect("session closed");
    assert!(
        projection
            .live_transcript(SOURCE_KEY)
            .expect("cleared events")
            .is_empty()
    );
    let closed = projection
        .source_state(SOURCE_KEY)
        .expect("closed source row");
    assert!(closed.delivery_revision > 0);
    projection.fail_closed(SOURCE_KEY).expect("fail closed");
    let failed = projection.source_state(SOURCE_KEY).expect("failed state");
    assert!(failed.delivery_unknown);
    assert_eq!(failed.queue_depth, 1);

    let peer = HeldPeerProjection {
        collaboration_id: "collaboration-1".to_owned(),
        queue_entry_id: "queue-1".to_owned(),
        result_id: "result-1".to_owned(),
        result_digest: "digest-1".to_owned(),
        source_session_key: SOURCE_KEY.to_owned(),
        source_engine: CollaborationEngineId::Claude,
        target_session_key: "codex#target-1".to_owned(),
        target_engine: CollaborationEngineId::Codex,
        objective: "Review".to_owned(),
        body: "Result".to_owned(),
    };
    projection.hold_peer(peer.clone()).expect("hold peer");
    projection.hold_peer(peer.clone()).expect("idempotent hold");
    let mut conflict = peer.clone();
    conflict.result_digest = "different".to_owned();
    assert_eq!(
        projection
            .hold_peer(conflict)
            .expect_err("identity conflict")
            .code,
        "collaboration-peer-identity-conflict"
    );
    projection
        .remove_peer(SOURCE_KEY, "collaboration-1", "queue-1", "result-1")
        .expect("remove peer");
    assert!(
        projection
            .held_peer(SOURCE_KEY, "queue-1")
            .expect("held lookup")
            .is_none()
    );
}

fn test_source_snapshot(projection: &DesktopCollaborationProjection) -> SourceSnapshot {
    let projected = projection
        .source_state(SOURCE_KEY)
        .expect("source projection");
    SourceSnapshot {
        engine: CollaborationEngineId::Claude,
        session_key: SOURCE_KEY.to_owned(),
        provider_session_id: "source-1".to_owned(),
        title: Some("Source title".to_owned()),
        registry_revision: 7,
        runtime_epoch: RUNTIME_EPOCH.to_owned(),
        live_handle_generation: 1,
        live_status: CollaborationLiveStatus::Idle,
        canonical_cwd: "/tmp/project".to_owned(),
        canonical_project_root: "/tmp/project".to_owned(),
        pending_approval_set_hash: projected.pending_approval_set_hash,
        pending_approval_count: projected.pending_approval_count,
        queue_revision: projected.queue_revision,
        queue_depth: projected.queue_depth,
        delivery_revision: projected.delivery_revision,
        delivery_unknown: projected.delivery_unknown,
        worktree: Some(CollaborationWorktreeV1::Unmanaged(UnmanagedWorktreeV1 {
            version: 1,
            managed: false,
        })),
    }
}

/// A runtime with no engines and no sessions, for port checks that stop before a provider.
fn empty_runtime(root: &Path) -> Arc<SessionRuntime> {
    use std::os::unix::fs::PermissionsExt;
    let private = |path: &Path| {
        std::fs::create_dir(path).expect("runtime directory");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("private runtime directory");
    };
    let runtime_root = root.join("runtime");
    private(&runtime_root);
    let sessions = interface_registry::SessionRegistry::open(&runtime_root).expect("registry");
    let history_root = runtime_root.join("history");
    private(&history_root);
    let history = interface_runtime::EngineHistoryStore::open(
        &history_root,
        interface_runtime::HistoryLimits::default(),
    )
    .expect("history");
    Arc::new(
        SessionRuntime::builder(interface_runtime::RuntimeServices::new(sessions, history))
            .build()
            .expect("runtime"),
    )
}

struct TestAgentSessions {
    bound_key: &'static str,
    agents_root: &'static str,
}

impl CollaborationAgentSessions for TestAgentSessions {
    fn is_agent_session(&self, session_key: &str, canonical_cwd: &Path) -> bool {
        session_key == self.bound_key || canonical_cwd.starts_with(self.agents_root)
    }
}

/// Agents run behind the approval gate, so the broker refuses them as a consult or handoff
/// source before any preview, target launch or advisory send.
#[tokio::test]
async fn agent_sessions_cannot_start_a_consult_or_handoff() {
    let temp = TempDir::new().expect("temp");
    let port =
        SessionRuntimeCollaborationPort::new(empty_runtime(temp.path()), prepare_projection());
    port.set_agent_sessions(Arc::new(TestAgentSessions {
        bound_key: "claude#agent-run",
        agents_root: "/Users/me/Interface Agents",
    }))
    .expect("agent sessions");
    let projection = prepare_projection();
    let ordinary = test_source_snapshot(&projection);
    let bound = SourceSnapshot {
        session_key: "claude#agent-run".to_owned(),
        canonical_cwd: "/tmp/project".to_owned(),
        ..ordinary.clone()
    };
    let in_agents_root = SourceSnapshot {
        canonical_cwd: "/Users/me/Interface Agents/Research".to_owned(),
        ..ordinary.clone()
    };
    for source in [&bound, &in_agents_root] {
        for mode in [DomainMode::Consult, DomainMode::Handoff] {
            let error = port
                .check_target(source, CollaborationEngineId::Codex, mode)
                .await
                .expect_err("agent source");
            assert_eq!(error.code, CollaborationErrorCode::TargetNotSupported);
            assert_eq!(error.message, AGENT_SOURCE_REFUSED);
            assert!(!error.retryable);
        }
        assert_eq!(
            port.refuse_agent_source(&source.session_key, &source.canonical_cwd)
                .await
                .expect_err("effect check")
                .code,
            CollaborationErrorCode::TargetNotSupported
        );
    }
    // An ordinary source goes on to the ordinary target checks (no engine is registered here).
    let error = port
        .check_target(&ordinary, CollaborationEngineId::Codex, DomainMode::Consult)
        .await
        .expect_err("unregistered target");
    assert_eq!(error.message, "target engine is not registered");
    port.refuse_agent_source(&ordinary.session_key, &ordinary.canonical_cwd)
        .await
        .expect("ordinary source");
    assert!(
        port.set_agent_sessions(Arc::new(TestAgentSessions {
            bound_key: "",
            agents_root: "/nowhere",
        }))
        .is_err(),
        "the agent check is installed once"
    );
}

/// The provider sessions running Code panes are bound to.
struct BoundPanes(Vec<String>);

impl interface_runtime::TerminalSessionBindings for BoundPanes {
    fn bound_provider_sessions(&self) -> Vec<String> {
        self.0.clone()
    }
}

/// One writer per provider session (review B4): a consult or handoff whose
/// source a Code pane holds, or is opening for, is refused at its preview
/// and before every effect that would write to that session — the target
/// launch, the advisory a consult delivers, the park a handoff makes — all
/// definitely unsent, before the source is even read.
#[tokio::test]
async fn a_consult_never_writes_to_a_session_a_pane_holds() {
    use interface_collaboration::{EffectCommand, EffectOutcome, state::CollaborationTargetV1};

    let temp = TempDir::new().expect("temp");
    let port =
        SessionRuntimeCollaborationPort::new(empty_runtime(temp.path()), prepare_projection());
    let claims = Arc::new(interface_runtime::SessionClaims::new(Arc::new(BoundPanes(
        vec!["source-1".to_owned()],
    ))));
    port.set_session_claims(claims.clone()).expect("claims");
    let projection = prepare_projection();
    let snapshot = test_source_snapshot(&projection);
    for mode in [DomainMode::Consult, DomainMode::Handoff] {
        let error = port
            .check_target(&snapshot, CollaborationEngineId::Codex, mode)
            .await
            .expect_err("held source");
        assert_eq!(error.code, CollaborationErrorCode::SourceNotLive);
        assert_eq!(
            error.message,
            interface_runtime::SessionClaim::Terminal.message()
        );
    }

    let source = snapshot.to_source().expect("source");
    let target = CollaborationTargetV1 {
        engine: CollaborationEngineId::Codex,
        session_key: Some("codex#target-1".to_owned()),
        provider_session_id: Some("target-1".to_owned()),
        reconciliation_marker: None,
    };
    let effects = |source: &interface_collaboration::state::CollaborationSourceV1| {
        [
            EffectCommand::PrepareTarget {
                collaboration_id: "collaboration-1".to_owned(),
                effect_id: "effect-prepare".to_owned(),
                mode: DomainMode::Consult,
                target_collaboration_depth: 1,
                source: source.clone(),
                target_engine: CollaborationEngineId::Codex,
            },
            EffectCommand::DeliverResult {
                collaboration_id: "collaboration-1".to_owned(),
                effect_id: "effect-deliver".to_owned(),
                source: source.clone(),
                target: target.clone(),
                queue_entry_id: "queue-1".to_owned(),
                result_id: "result-1".to_owned(),
                result_digest: "digest".to_owned(),
                body: "Looks wrong.".to_owned(),
                expected_source_guard_hash: "guard".to_owned(),
            },
            EffectCommand::ParkSource {
                collaboration_id: "collaboration-1".to_owned(),
                effect_id: "effect-park".to_owned(),
                source: source.clone(),
                target: target.clone(),
                expected_source_guard_hash: "guard".to_owned(),
            },
        ]
    };
    for effect in effects(&source) {
        let kind = effect.kind();
        assert_eq!(
            port.execute(effect, CancellationToken::new()).await,
            EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::SourceNotLive
            },
            "{kind:?} on a held source"
        );
    }

    // A session handed to a pane that has not started yet is held too.
    let handed = interface_collaboration::state::CollaborationSourceV1 {
        session_key: "claude#source-2".to_owned(),
        provider_session_id: Some("source-2".to_owned()),
        ..source.clone()
    };
    claims.hand_to_terminal("claude#source-2");
    let error = port
        .check_target(
            &SourceSnapshot {
                session_key: "claude#source-2".to_owned(),
                provider_session_id: "source-2".to_owned(),
                ..snapshot.clone()
            },
            CollaborationEngineId::Codex,
            DomainMode::Consult,
        )
        .await
        .expect_err("handed source");
    assert_eq!(
        error.message,
        interface_runtime::SessionClaim::HandingOff.message()
    );
    for effect in effects(&handed) {
        assert_eq!(
            port.execute(effect, CancellationToken::new()).await,
            EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::SourceNotLive
            }
        );
    }

    // Once the pane lets go, the effects go on to their ordinary checks
    // (here: this empty runtime has no such source).
    claims.release_handoff("claude#source-2");
    for effect in effects(&handed) {
        assert_ne!(
            port.execute(effect, CancellationToken::new()).await,
            EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::SourceNotLive
            }
        );
    }
    assert!(
        port.set_session_claims(claims).is_err(),
        "the claims are attached once"
    );
}

fn sample_summary(last_activity: &str) -> SessionSummary {
    SessionSummary {
        engine: EngineId::new("claude").expect("claude engine"),
        session_id: "source-1".to_owned(),
        title: "Source title".to_owned(),
        cwd: "/tmp/project".to_owned(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: last_activity.to_owned(),
        first_seen: "2026-08-27T12:00:00.000Z".to_owned(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: None,
        project_root: Some("/tmp/project".to_owned()),
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

#[test]
fn registry_fingerprint_ignores_last_activity() {
    let first =
        SourceRegistryFingerprint::from_summary(&sample_summary("2026-08-27T12:00:00.000Z"));
    let second =
        SourceRegistryFingerprint::from_summary(&sample_summary("2026-08-27T12:00:01.000Z"));
    assert_eq!(first, second);
    let mut retitled = sample_summary("2026-08-27T12:00:00.000Z");
    retitled.title = "Changed".to_owned();
    assert_ne!(first, SourceRegistryFingerprint::from_summary(&retitled));
    assert!(first.matches(&sample_summary("2026-08-27T12:00:01.000Z")));
    assert!(!first.matches(&retitled));
}

#[test]
fn push_bounded_utf8_truncates_in_place_on_char_boundary() {
    let mut current = String::from("hello");
    push_bounded_utf8(&mut current, " world", 8);
    assert_eq!(current, "hello wo");
    push_bounded_utf8(&mut current, "😀", 8);
    assert_eq!(current, "hello wo");
    let mut empty = String::new();
    push_bounded_utf8(&mut empty, "abc", 0);
    assert_eq!(empty, "");
}

#[test]
fn source_index_tracks_live_collaborations_and_forgets_terminal() {
    let mut drives = DriveState::default();
    assert!(!drives.source_has(SOURCE_KEY));
    drives.remember_source(SOURCE_KEY, "collaboration-1");
    drives.remember_source(SOURCE_KEY, "collaboration-2");
    assert!(drives.source_has(SOURCE_KEY));
    let mut ids = drives.source_collaborations(SOURCE_KEY);
    ids.sort();
    assert_eq!(ids, vec!["collaboration-1", "collaboration-2"]);
    drives.forget_source(SOURCE_KEY, "collaboration-1");
    assert_eq!(
        drives.source_collaborations(SOURCE_KEY),
        vec!["collaboration-2".to_owned()]
    );
    drives.forget_source(SOURCE_KEY, "collaboration-2");
    assert!(!drives.source_has(SOURCE_KEY));
    assert!(drives.source_collaborations(SOURCE_KEY).is_empty());
}

#[test]
fn provider_and_live_transcript_preserves_duplicates_and_keeps_the_newest_bound() {
    let event = |text: String| EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text,
            agent_key: None,
        },
    };
    let duplicate = event("duplicate".into());
    let combined = combine_provider_and_live_transcript(
        vec![event("stored".into()), duplicate.clone()],
        vec![duplicate.clone(), event("live".into())],
    );
    assert_eq!(
        combined,
        vec![
            event("stored".into()),
            duplicate.clone(),
            duplicate,
            event("live".into())
        ]
    );

    let limit = usize::try_from(HISTORY_MAX_LIMIT).expect("history limit");
    let bounded = combine_provider_and_live_transcript(
        (0..limit)
            .map(|index| event(format!("stored-{index}")))
            .collect(),
        vec![event("live-one".into()), event("live-two".into())],
    );
    assert_eq!(bounded.len(), limit);
    assert_eq!(bounded.first(), Some(&event("stored-2".into())));
    assert_eq!(bounded.last(), Some(&event("live-two".into())));
}

#[derive(Default)]
struct FakePort {
    commands: StdMutex<Vec<CollaborationEffectKind>>,
    shutdowns: AtomicUsize,
    ambiguous_delivery: AtomicBool,
    /// `None`: the source is always live. `Some(n)`: n more live answers,
    /// then the source reads as not live.
    live_budget: StdMutex<Option<usize>>,
}

impl FakePort {
    fn commands(&self) -> Vec<CollaborationEffectKind> {
        self.commands.lock().expect("fake port commands").clone()
    }
}

#[async_trait]
impl CollaborationRuntimePort for FakePort {
    async fn resolve_source(
        &self,
        session_key: &str,
        runtime_epoch: &str,
        projection: &DesktopCollaborationProjection,
    ) -> Result<Option<SourceSnapshot>, CollaborationPortError> {
        if session_key != SOURCE_KEY || runtime_epoch != RUNTIME_EPOCH {
            return Ok(None);
        }
        if let Some(remaining) = self.live_budget.lock().expect("live budget").as_mut() {
            if *remaining == 0 {
                return Ok(None);
            }
            *remaining -= 1;
        }
        Ok(Some(test_source_snapshot(projection)))
    }

    async fn check_target(
        &self,
        _source: &SourceSnapshot,
        target: CollaborationEngineId,
        _mode: DomainMode,
    ) -> Result<(), CollaborationPortError> {
        if target == CollaborationEngineId::Codex {
            Ok(())
        } else {
            Err(CollaborationPortError::new(
                CollaborationErrorCode::TargetNotSupported,
                "fake only supports codex",
                false,
            ))
        }
    }

    async fn build_packet(
        &self,
        material: &HumanPreviewMaterial,
        _source: &SourceSnapshot,
        _projection: &DesktopCollaborationProjection,
    ) -> Result<PacketBuildResult, CollaborationPortError> {
        Ok(PacketBuildResult {
            body: format!(
                "packet:{}:{}",
                material.collaboration_id, material.objective
            ),
            context_bytes: 0,
            omissions: 0,
        })
    }

    async fn execute(
        &self,
        command: EffectCommand,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        let kind = command.kind();
        self.commands.lock().expect("fake port commands").push(kind);
        if cancellation.is_cancelled() {
            return EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::BrokerUnavailable,
            };
        }
        if kind == CollaborationEffectKind::DeliverResult
            && self.ambiguous_delivery.load(Ordering::SeqCst)
        {
            return EffectOutcome::Ambiguous {
                code: None,
                provider_effect_id: None,
            };
        }
        let value = match command {
            EffectCommand::PrepareTarget { .. } => EffectSuccess::PreparedTarget(PreparedTarget {
                session_key: "codex#target-1".to_owned(),
                provider_session_id: "target-1".to_owned(),
                reconciliation_marker: None,
            }),
            EffectCommand::RunTarget { .. } => {
                EffectSuccess::ConsultResult("independent review result".to_owned())
            }
            EffectCommand::ParkSource { .. }
            | EffectCommand::TransferOwnership { .. }
            | EffectCommand::ActivateTarget { .. }
            | EffectCommand::DeliverResult { .. } => EffectSuccess::Unit,
        };
        EffectOutcome::Succeeded {
            value,
            provider_effect_id: Some(format!("provider-{kind:?}")),
        }
    }

    async fn shutdown(&self) -> Result<(), CollaborationPortError> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn deferred_model_ingress_is_fail_closed_and_attaches_exactly_once() {
    let temporary = TempDir::new().expect("temporary collaboration root");
    let journal = CollaborationJournal::open(temporary.path().join("collaboration"))
        .expect("collaboration journal");
    let projection = Arc::new(DesktopCollaborationProjection::new());
    let service = Arc::new(
        DurableCollaborationService::start(
            CollaborationCoordinator::new(journal),
            Arc::new(FakePort::default()),
            projection,
            RUNTIME_EPOCH.to_owned(),
            None,
        )
        .await
        .expect("durable collaboration service"),
    );
    let deferred = DeferredCollaborationModelIngress::new();

    let Err(unavailable) = deferred.target() else {
        panic!("unattached ingress must fail");
    };
    assert_eq!(unavailable.code, CollaborationErrorCode::BrokerUnavailable);

    let target: Arc<dyn CollaborationModelIngress> = service.clone();
    deferred
        .attach(target.clone())
        .expect("first attachment succeeds");
    let resolved = deferred.target().expect("attached ingress");
    assert!(Arc::ptr_eq(&resolved, &target));
    let duplicate = deferred
        .attach(target)
        .expect_err("second attachment must fail closed");
    assert_eq!(duplicate.code, "collaboration-unavailable");

    service.close().await.expect("service shutdown");
}

#[tokio::test]
async fn approval_sweep_expires_and_pushes_every_changed_receipt() {
    let temporary = TempDir::new().expect("temporary collaboration root");
    let now = Arc::new(StdMutex::new("2026-08-27T12:00:00.000Z".to_owned()));
    let clock = Arc::clone(&now);
    let identifiers = Arc::new(AtomicUsize::new(0));
    let next_identifier = Arc::clone(&identifiers);
    let coordinator = CollaborationCoordinator::with_generators(
        CollaborationJournal::open(temporary.path().join("collaboration"))
            .expect("collaboration journal"),
        None,
        CollaborationPolicyOverrides {
            approval_timeout_ms: Some(1),
            ..CollaborationPolicyOverrides::default()
        },
        move || clock.lock().expect("test clock").clone(),
        move |kind| format!("{kind}-{}", next_identifier.fetch_add(1, Ordering::SeqCst)),
    );
    let projection = prepare_projection();
    let (push_tx, mut push_rx) = mpsc::channel(16);
    let service = DurableCollaborationService::start(
        coordinator,
        Arc::new(FakePort::default()),
        projection,
        RUNTIME_EPOCH.to_owned(),
        Some(push_tx),
    )
    .await
    .expect("durable collaboration service");
    let submitted = CollaborationDesktopControl::submit(
        &service,
        CollaborationSubmitRequest {
            source_session_key: SOURCE_KEY.to_owned(),
            mode: CollaborationMode::Consult,
            target_engine: CollaborationEngine::Codex,
            objective: "Review the Rust port".to_owned(),
            client_request_id: "approval-sweep-request".to_owned(),
        },
    )
    .await
    .expect("submit collaboration");
    assert_eq!(
        submitted.receipt.state,
        CollaborationState::AwaitingApproval
    );
    while push_rx.try_recv().is_ok() {}
    assert!(
        service
            .inner
            .drives
            .lock()
            .expect("drive state")
            .source_has(SOURCE_KEY)
    );

    *now.lock().expect("test clock") = "2026-08-27T12:00:01.000Z".to_owned();
    assert_eq!(
        expire_pending_approvals_and_publish(&service.inner)
            .await
            .expect("approval sweep"),
        1
    );
    let changed = timeout(Duration::from_secs(1), push_rx.recv())
        .await
        .expect("expiry push timeout")
        .expect("expiry push");
    let event: CollaborationEventDto = changed.payload.decode().expect("expiry event");
    assert_eq!(event.status.receipt.state, CollaborationState::Cancelled);
    assert_eq!(
        event.status.receipt.error.expect("expiry error").code,
        "APPROVAL_EXPIRED"
    );
    assert!(push_rx.try_recv().is_err());
    assert!(
        !service
            .inner
            .drives
            .lock()
            .expect("drive state")
            .source_has(SOURCE_KEY)
    );
    service.close().await.expect("service shutdown");
}

fn prepare_projection() -> Arc<DesktopCollaborationProjection> {
    let projection = Arc::new(DesktopCollaborationProjection::new());
    projection.activate_session(SOURCE_KEY).expect("activate");
    projection
        .set_draft_state(&CollaborationDraftStateUpdate {
            source_session_key: SOURCE_KEY.to_owned(),
            revision: 1,
            empty: true,
        })
        .expect("empty draft");
    projection
}

fn held_peer_for(
    projection: &DesktopCollaborationProjection,
    collaboration_id: &str,
) -> Option<HeldPeerProjection> {
    projection.held_collaboration_for_test(SOURCE_KEY, collaboration_id)
}

/// The projection is updated after the durable state commit that makes the
/// peer visible, so observers poll for it rather than assume it is already
/// there.
async fn wait_for_held_peer(
    projection: &DesktopCollaborationProjection,
    collaboration_id: &str,
    held: bool,
) {
    timeout(Duration::from_secs(5), async {
        while held_peer_for(projection, collaboration_id).is_some() != held {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("held peer projection timeout");
}

async fn wait_for_state(
    service: &DurableCollaborationService<FakePort>,
    collaboration_id: &str,
    expected: CollaborationState,
) -> CollaborationStatusDto {
    timeout(Duration::from_secs(5), async {
        loop {
            let status = CollaborationDesktopControl::status(service, collaboration_id.to_owned())
                .await
                .expect("collaboration status");
            if status.receipt.state == expected {
                return status;
            }
            assert!(
                !matches!(
                    status.receipt.state,
                    CollaborationState::Failed
                        | CollaborationState::Unknown
                        | CollaborationState::Denied
                        | CollaborationState::Cancelled
                ),
                "collaboration entered {:?}",
                status.receipt.state
            );
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("collaboration drive timeout")
}

async fn wait_for_settled_state(
    service: &DurableCollaborationService<FakePort>,
    collaboration_id: &str,
    expected: CollaborationState,
) -> CollaborationStatusDto {
    timeout(Duration::from_secs(5), async {
        loop {
            let status = CollaborationDesktopControl::status(service, collaboration_id.to_owned())
                .await
                .expect("collaboration status");
            if status.receipt.state == expected {
                return status;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("collaboration settle timeout")
}

fn consult_request(client_request_id: &str) -> CollaborationSubmitRequest {
    CollaborationSubmitRequest {
        source_session_key: SOURCE_KEY.to_owned(),
        mode: CollaborationMode::Consult,
        target_engine: CollaborationEngine::Codex,
        objective: "Review the Rust port".to_owned(),
        client_request_id: client_request_id.to_owned(),
    }
}

#[tokio::test]
async fn restart_reconciles_an_unknown_record_and_unlocks_its_source() {
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("collaboration");
    let first_port = Arc::new(FakePort::default());
    first_port.ambiguous_delivery.store(true, Ordering::SeqCst);
    let first = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(&root).expect("collaboration journal"),
        ),
        first_port.clone(),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("start first service");
    let submitted =
        CollaborationDesktopControl::submit(&first, consult_request("unknown-request-1"))
            .await
            .expect("submit collaboration");
    let approval = submitted.approval.expect("approval preview");
    first
        .approve(CollaborationApprovalAction {
            collaboration_id: approval.collaboration_id.clone(),
            approval_id: approval.approval_id,
            expected_revision: approval.expected_revision,
        })
        .await
        .expect("approve collaboration");
    let awaiting = wait_for_state(
        &first,
        &approval.collaboration_id,
        CollaborationState::AwaitingPeerReview,
    )
    .await;
    let peer = awaiting.peer.expect("held peer");
    first
        .accept_peer(CollaborationPeerAction {
            collaboration_id: peer.collaboration_id.clone(),
            result_id: peer.result_id,
            result_digest: peer.result_digest,
            expected_revision: peer.expected_revision,
        })
        .await
        .expect("accept peer");
    wait_for_settled_state(
        &first,
        &approval.collaboration_id,
        CollaborationState::Unknown,
    )
    .await;

    // The ambiguous delivery pins the source for as long as the record lives.
    let refused = CollaborationDesktopControl::submit(&first, consult_request("unknown-request-2"))
        .await
        .expect_err("a locked source must refuse new work");
    assert_eq!(refused.code, "COLLABORATION_SOURCE_DELIVERY_UNKNOWN");
    first.close().await.expect("close first service");
    drop(first);

    let second_port = Arc::new(FakePort::default());
    let second = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(&root).expect("reopen collaboration journal"),
        ),
        second_port.clone(),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("recover service");
    let reconciled =
        CollaborationDesktopControl::status(&second, approval.collaboration_id.clone())
            .await
            .expect("reconciled status");
    assert_eq!(reconciled.receipt.state, CollaborationState::Failed);
    assert_eq!(
        reconciled.receipt.error.expect("reconciled failure").code,
        "SOURCE_DELIVERY_UNKNOWN"
    );
    let unlocked =
        CollaborationDesktopControl::submit(&second, consult_request("unknown-request-3"))
            .await
            .expect("recovery must unlock the source session");
    assert_eq!(unlocked.receipt.state, CollaborationState::AwaitingApproval);
    assert!(
        second_port.commands().is_empty(),
        "the ambiguous delivery must never be dispatched again"
    );
    second.close().await.expect("close recovered service");
}

#[tokio::test]
async fn durable_service_drives_consult_recovers_peer_and_shuts_down_cleanly() {
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("collaboration");
    let first_projection = prepare_projection();
    let first_port = Arc::new(FakePort::default());
    let (push_tx, mut push_rx) = mpsc::channel(64);
    let first = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(&root).expect("collaboration journal"),
        ),
        first_port.clone(),
        first_projection.clone(),
        RUNTIME_EPOCH.to_owned(),
        Some(push_tx),
    )
    .await
    .expect("start first service");
    let submitted = CollaborationDesktopControl::submit(
        &first,
        CollaborationSubmitRequest {
            source_session_key: SOURCE_KEY.to_owned(),
            mode: CollaborationMode::Consult,
            target_engine: CollaborationEngine::Codex,
            objective: "Review the Rust port".to_owned(),
            client_request_id: "request-1".to_owned(),
        },
    )
    .await
    .expect("submit collaboration");
    assert_eq!(
        submitted.receipt.state,
        CollaborationState::AwaitingApproval
    );
    let approval = submitted.approval.expect("approval preview");
    let approved = first
        .approve(CollaborationApprovalAction {
            collaboration_id: approval.collaboration_id.clone(),
            approval_id: approval.approval_id,
            expected_revision: approval.expected_revision,
        })
        .await
        .expect("approve collaboration");
    assert_eq!(approved.receipt.state, CollaborationState::Approved);
    let awaiting = wait_for_state(
        &first,
        &approval.collaboration_id,
        CollaborationState::AwaitingPeerReview,
    )
    .await;
    let peer = awaiting.peer.expect("held peer");
    wait_for_held_peer(&first_projection, &approval.collaboration_id, true).await;
    assert_eq!(
        first_port.commands(),
        [
            CollaborationEffectKind::PrepareTarget,
            CollaborationEffectKind::RunTarget,
        ]
    );
    let mut changed_pushes = 0;
    while let Ok(frame) = push_rx.try_recv() {
        assert_eq!(frame.operation, CHANNELS.collaboration_event);
        let event: CollaborationEventDto = frame.payload.decode().expect("collaboration event");
        assert_eq!(event.kind, "changed");
        changed_pushes += 1;
    }
    assert!(changed_pushes >= 2);
    first.close().await.expect("close first service");
    assert_eq!(first_port.shutdowns.load(Ordering::SeqCst), 1);
    drop(first);

    let second_projection = prepare_projection();
    let second_port = Arc::new(FakePort::default());
    let second = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(&root).expect("reopen collaboration journal"),
        ),
        second_port.clone(),
        second_projection.clone(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("recover service");
    assert!(held_peer_for(&second_projection, &approval.collaboration_id).is_some());
    let accepted = second
        .accept_peer(CollaborationPeerAction {
            collaboration_id: peer.collaboration_id.clone(),
            result_id: peer.result_id,
            result_digest: peer.result_digest,
            expected_revision: peer.expected_revision,
        })
        .await
        .expect("accept peer");
    assert!(matches!(
        accepted.receipt.state,
        CollaborationState::DeliveryPending | CollaborationState::Completed
    ));
    let completed = wait_for_state(
        &second,
        &peer.collaboration_id,
        CollaborationState::Completed,
    )
    .await;
    assert!(completed.peer.is_none());
    wait_for_held_peer(&second_projection, &peer.collaboration_id, false).await;
    assert_eq!(
        second_port.commands(),
        [CollaborationEffectKind::DeliverResult]
    );
    second.close().await.expect("close recovered service");
    assert_eq!(second_port.shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_interrupted_transition_stops_admission_and_is_never_replay_safe() {
    let temp = TempDir::new().expect("temp directory");
    let service = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(temp.path().join("collaboration"))
                .expect("collaboration journal"),
        ),
        Arc::new(FakePort::default()),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("start service");
    let interrupted = service
        .inner
        .transition(
            |_: &mut CollaborationCoordinator| -> Result<(), CoordinatorError> {
                panic!("injected transition failure")
            },
        )
        .await
        .expect_err("a panicking transition fails");
    assert!(matches!(
        interrupted,
        CoordinatorError::Ingress {
            code: CollaborationErrorCode::RecoveryRequired,
            retryable: false,
            ..
        }
    ));
    assert!(!CollaborationServiceError::domain(interrupted).replay_safe);
    CollaborationDesktopControl::submit(&service, consult_request("after-interruption"))
        .await
        .expect_err("the coordinator stops admitting work after an interruption");
    service.close().await.expect("close service");
}

/// A source event that changes nothing the dispatch guard reads.
fn source_token() -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::ThinkingSignal {
            active: true,
            agent_key: None,
        },
    }
}

async fn wait_for_drives(
    service: &DurableCollaborationService<FakePort>,
    what: &str,
    ready: impl Fn(&DriveState) -> bool,
) {
    timeout(Duration::from_secs(5), async {
        loop {
            if ready(&service.inner.drives.lock().expect("drive state")) {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn parked_peer_review_skips_source_events_but_delivery_pending_redrives() {
    let temp = TempDir::new().expect("temp directory");
    let port = Arc::new(FakePort::default());
    let service = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(temp.path().join("collaboration"))
                .expect("collaboration journal"),
        ),
        port.clone(),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("durable collaboration service");
    let submitted = CollaborationDesktopControl::submit(&service, consult_request("parked-1"))
        .await
        .expect("submit collaboration");
    let approval = submitted.approval.expect("approval preview");
    let id = approval.collaboration_id.clone();
    service
        .approve(CollaborationApprovalAction {
            collaboration_id: id.clone(),
            approval_id: approval.approval_id,
            expected_revision: approval.expected_revision,
        })
        .await
        .expect("approve collaboration");
    let peer = wait_for_state(&service, &id, CollaborationState::AwaitingPeerReview)
        .await
        .peer
        .expect("held peer");
    // Waiting on the person: parked, so source tokens no longer drive it.
    wait_for_drives(&service, "the peer review to park", |drives| {
        drives.parked.contains(&id) && !drives.active.contains(&id)
    })
    .await;
    service
        .observe_event(SOURCE_KEY, &source_token())
        .expect("source token");
    assert!(
        service
            .inner
            .drives
            .lock()
            .expect("drive state")
            .parked
            .contains(&id)
    );

    // Accepted while the source is live, but the delivery drive finds it gone.
    *port.live_budget.lock().expect("live budget") = Some(1);
    service
        .accept_peer(CollaborationPeerAction {
            collaboration_id: id.clone(),
            result_id: peer.result_id,
            result_digest: peer.result_digest,
            expected_revision: peer.expected_revision,
        })
        .await
        .expect("accept peer");
    wait_for_drives(&service, "the delivery drive to wait", |drives| {
        !drives.active.contains(&id)
    })
    .await;
    assert_eq!(
        CollaborationDesktopControl::status(&service, id.clone())
            .await
            .expect("status")
            .receipt
            .state,
        CollaborationState::DeliveryPending
    );
    assert!(
        !service
            .inner
            .drives
            .lock()
            .expect("drive state")
            .parked
            .contains(&id)
    );
    assert!(
        !port
            .commands()
            .contains(&CollaborationEffectKind::DeliverResult)
    );

    // DeliveryPending waits on its source, so the next source event delivers.
    *port.live_budget.lock().expect("live budget") = None;
    service
        .observe_event(SOURCE_KEY, &source_token())
        .expect("source token");
    wait_for_state(&service, &id, CollaborationState::Completed).await;
    assert!(
        port.commands()
            .contains(&CollaborationEffectKind::DeliverResult)
    );
    service.close().await.expect("service shutdown");
}

#[tokio::test]
async fn a_failed_approval_preview_is_released_even_while_parked() {
    let temp = TempDir::new().expect("temp directory");
    let port = Arc::new(FakePort::default());
    let service = DurableCollaborationService::start(
        CollaborationCoordinator::new(
            CollaborationJournal::open(temp.path().join("collaboration"))
                .expect("collaboration journal"),
        ),
        port.clone(),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        None,
    )
    .await
    .expect("durable collaboration service");
    let submitted = CollaborationDesktopControl::submit(&service, consult_request("parked-2"))
        .await
        .expect("submit collaboration");
    let approval = submitted.approval.expect("approval preview");
    let id = approval.collaboration_id.clone();
    service
        .observe_event(SOURCE_KEY, &source_token())
        .expect("source token");
    wait_for_drives(&service, "the approval to park", |drives| {
        drives.parked.contains(&id) && !drives.active.contains(&id)
    })
    .await;
    // The preview cannot resolve the source, so approval fails the record.
    *port.live_budget.lock().expect("live budget") = Some(0);
    let failed = service
        .approve(CollaborationApprovalAction {
            collaboration_id: id.clone(),
            approval_id: approval.approval_id,
            expected_revision: approval.expected_revision,
        })
        .await
        .expect("approve collaboration");
    assert_eq!(failed.receipt.state, CollaborationState::Failed);
    wait_for_drives(&service, "the failed record to be released", |drives| {
        !drives.source_has(SOURCE_KEY) && !drives.parked.contains(&id)
    })
    .await;
    service.close().await.expect("service shutdown");
}

#[tokio::test]
async fn approving_one_request_releases_the_others_it_expired() {
    let temporary = TempDir::new().expect("temporary collaboration root");
    let now = Arc::new(StdMutex::new("2026-08-27T12:00:00.000Z".to_owned()));
    let clock = Arc::clone(&now);
    let identifiers = Arc::new(AtomicUsize::new(0));
    let next_identifier = Arc::clone(&identifiers);
    let coordinator = CollaborationCoordinator::with_generators(
        CollaborationJournal::open(temporary.path().join("collaboration"))
            .expect("collaboration journal"),
        None,
        CollaborationPolicyOverrides {
            approval_timeout_ms: Some(1_000),
            ..CollaborationPolicyOverrides::default()
        },
        move || clock.lock().expect("test clock").clone(),
        move |kind| format!("{kind}-{}", next_identifier.fetch_add(1, Ordering::SeqCst)),
    );
    let (push_tx, mut push_rx) = mpsc::channel(64);
    let service = DurableCollaborationService::start(
        coordinator,
        Arc::new(FakePort::default()),
        prepare_projection(),
        RUNTIME_EPOCH.to_owned(),
        Some(push_tx),
    )
    .await
    .expect("durable collaboration service");
    let stale = CollaborationDesktopControl::submit(&service, consult_request("stale"))
        .await
        .expect("submit stale request")
        .approval
        .expect("stale preview")
        .collaboration_id;
    *now.lock().expect("test clock") = "2026-08-27T12:00:00.500Z".to_owned();
    let fresh = CollaborationDesktopControl::submit(&service, consult_request("fresh"))
        .await
        .expect("submit fresh request")
        .approval
        .expect("fresh preview");
    // Both wait on the person, so a source event parks them.
    service
        .observe_event(SOURCE_KEY, &source_token())
        .expect("source token");
    wait_for_drives(&service, "both approvals to park", |drives| {
        drives.parked.contains(&stale) && drives.parked.contains(&fresh.collaboration_id)
    })
    .await;
    while push_rx.try_recv().is_ok() {}
    // The stale request is past its expiry, the fresh one is not.
    *now.lock().expect("test clock") = "2026-08-27T12:00:01.200Z".to_owned();
    service
        .approve(CollaborationApprovalAction {
            collaboration_id: fresh.collaboration_id.clone(),
            approval_id: fresh.approval_id,
            expected_revision: fresh.expected_revision,
        })
        .await
        .expect("approve fresh request");
    // The stale request expired inside that approval: it is published and
    // released at once instead of staying pinned to its source.
    assert_eq!(
        CollaborationDesktopControl::status(&service, stale.clone())
            .await
            .expect("stale status")
            .receipt
            .state,
        CollaborationState::Cancelled
    );
    {
        let drives = service.inner.drives.lock().expect("drive state");
        assert!(
            !drives
                .source_index
                .get(SOURCE_KEY)
                .is_some_and(|ids| ids.contains(&stale))
        );
        assert!(!drives.parked.contains(&stale));
    }
    let mut published_stale = false;
    while let Ok(frame) = push_rx.try_recv() {
        let event: CollaborationEventDto = frame.payload.decode().expect("collaboration event");
        published_stale |= event.status.receipt.collaboration_id == stale;
    }
    assert!(published_stale, "the expired request was never published");
    service.close().await.expect("service shutdown");
}
