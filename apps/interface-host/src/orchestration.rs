use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use interface_contracts::{
    ipc::{
        CHANNELS, OrchestrationAgentSteerRequest, OrchestrationAgentStopRequest,
        OrchestrationEventDto, OrchestrationInboxRequest, OrchestrationRoleClaimRequest,
        OrchestrationRoleRelinquishRequest, OrchestrationTimelineRequest,
        OrchestrationTranscriptRequest, OrchestrationWorktreeEscrowRequest,
    },
    orchestration::{
        AgentCapabilityV1, AgentNodeState, AgentNodeV1, AgentScopeV1, EffectReservationV1,
        MailboxMessageV1, MessageDeliveryReceiptV1, MessageDeliveryState, MessageOrigin,
        ORCHESTRATOR_POLICY_BOUNDS, ORCHESTRATOR_POLICY_DEFAULTS, OrchestrationActorKind,
        OrchestrationActorV1, OrchestrationCapabilityFacet, OrchestrationCapabilityState,
        OrchestrationEffectKind, OrchestrationEffectState, OrchestrationEffectTargetKind,
        OrchestrationMutation, OrchestrationMutationMetaV1, OrchestrationSnapshotV1,
        OrchestratorAutonomy, OrchestratorPolicyV1, RoleClaimState, RoleClaimV1,
        SettledEffectState, SettledEscrowState, WorktreeEscrowState, WorktreeEscrowV1,
        WorktreeLeaseState, WorktreeLeaseV1,
    },
    orchestration_control::{
        AdversarialReviewStartRequestV1, AdversarialReviewStartV1, CoordinationStatusRequestV1,
        CoordinationStatusV1, OrchestrationAgentBindParentRequestV1,
        OrchestrationAgentCreateRequestV1, OrchestrationAgentRegisterSessionRequestV1,
        OrchestrationAgentSendRequestV1, OrchestratorCatalogueState, OrchestratorEngineModelsV1,
        OrchestratorModelRowV1, OrchestratorModelsV1, OrchestratorPolicyClearRequestV1,
        OrchestratorPolicySetRequestV1, OrchestratorSubmitResultV1, OrchestratorSyntheticModelsV1,
        WorkerAbandonRequestV1, WorkerAckRequestV1, WorkerIntegrateRequestV1,
        WorkerSubmitRequestV1,
    },
};
use interface_engine::EffectCertainty;
use interface_orchestration::{
    EventProjectionInput, InboxProjectionInput, OrchestrationError, OrchestrationErrorCode,
    OrchestrationStore, TranscriptProjectionInput, agent_state_is_terminal,
    project_orchestration_agent_for_control, project_orchestration_events_for_control,
    project_orchestration_inbox_for_control, project_orchestration_message_for_control,
    project_orchestration_snapshot_for_control, project_orchestration_transcript_for_control,
    worker_session_keys,
};
use interface_runtime::{RuntimeClock, RuntimeError, SystemRuntimeClock};
use interface_storage::sha256_hex;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    dispatcher::{DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE},
    protocol::PushFrame,
    registry_push::{
        CoalescedPush, RegistryPushCoordinator, SharedRegistryPushes, registry_push_round,
    },
    worktree::WorktreeService,
};

mod adversarial;
mod effects;
mod observer;
mod runtime_port;

use adversarial::adversarial_pair_token;

pub use runtime_port::{
    ExternalOutcome, OrchestrationRuntimePort, RuntimeAgentDescription, RuntimeEngineProbe,
    RuntimeLaunchRequest, RuntimeLaunchSuccess,
};

const MAX_TIMELINE_EVENTS: usize = 64;

/// The store revision the memo is keyed on, beside the snapshot it answered.
type SnapshotMemo = Arc<Mutex<Option<(u64, Arc<OrchestrationSnapshotV1>)>>>;
/// No commit has seeded the memo since the last failed write or close.
const NO_COMMITTED_REVISION: u64 = u64::MAX;
const MAX_INBOX_MESSAGES: usize = 4;
const MAX_TRANSCRIPT_LINES: u64 = 2_000;
const ULTRACODE_NOTE: &str =
    "Synthetic routing hint only; Interface never sends `ultracode` to a provider.";
const ADVERSARIAL_BUILDER_ROLE_PREFIX: &str = "adversarial-review-builder:";
const ADVERSARIAL_REVIEWER_ROLE_PREFIX: &str = "adversarial-review-reviewer:";

pub const ORCHESTRATION_OPERATIONS: [&str; 23] = [
    CHANNELS.get_orchestration_snapshot,
    CHANNELS.read_orchestration_timeline,
    CHANNELS.start_adversarial_review,
    CHANNELS.create_orchestration_agent,
    CHANNELS.send_orchestration_message,
    CHANNELS.reply_orchestration_message,
    CHANNELS.read_orchestration_transcript,
    CHANNELS.read_orchestration_inbox,
    CHANNELS.stop_orchestration_agent,
    CHANNELS.steer_orchestration_agent,
    CHANNELS.claim_orchestration_role,
    CHANNELS.relinquish_orchestration_role,
    CHANNELS.escrow_orchestration_worktree,
    CHANNELS.set_orchestrator_policy,
    CHANNELS.clear_orchestrator_policy,
    CHANNELS.bind_orchestration_parent,
    CHANNELS.register_orchestration_session,
    CHANNELS.submit_coordinated_worker,
    CHANNELS.integrate_coordinated_worker,
    CHANNELS.abandon_coordinated_task,
    CHANNELS.ack_coordinated_obligation,
    CHANNELS.get_coordination_status,
    CHANNELS.get_orchestrator_models,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinationSubmission {
    pub submission_id: String,
    pub changes: u64,
    pub state: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinationIntegration {
    pub task_id: String,
}

#[async_trait]
pub trait OrchestrationCoordinationPort: Send + Sync + 'static {
    async fn submit(&self, agent: &AgentNodeV1) -> Result<CoordinationSubmission, String>;
    async fn integrate(
        &self,
        agent: &AgentNodeV1,
        dry_run: bool,
    ) -> Result<CoordinationIntegration, String>;
    async fn abandon(&self, agent: &AgentNodeV1) -> Result<(), String>;
    async fn acknowledge(&self, agent: &AgentNodeV1, keep: &str) -> Result<Vec<String>, String>;
    async fn status(&self, canonical_project_root: &str) -> CoordinationStatusV1;
    async fn transfer_worktree(
        &self,
        lease_id: &str,
        source_agent_id: &str,
        target_agent_id: &str,
        source_owner_revision: u64,
    ) -> ExternalOutcome<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableCoordination;

#[async_trait]
impl OrchestrationCoordinationPort for UnavailableCoordination {
    async fn submit(&self, _agent: &AgentNodeV1) -> Result<CoordinationSubmission, String> {
        Err("Coordination is unavailable for this checkout.".into())
    }

    async fn integrate(
        &self,
        _agent: &AgentNodeV1,
        _dry_run: bool,
    ) -> Result<CoordinationIntegration, String> {
        Err("Coordination is unavailable for this checkout.".into())
    }

    async fn abandon(&self, _agent: &AgentNodeV1) -> Result<(), String> {
        Err("Coordination is unavailable for this checkout.".into())
    }

    async fn acknowledge(&self, _agent: &AgentNodeV1, _keep: &str) -> Result<Vec<String>, String> {
        Err("Coordination is unavailable for this checkout.".into())
    }

    async fn status(&self, _canonical_project_root: &str) -> CoordinationStatusV1 {
        CoordinationStatusV1 {
            registered: false,
            active_tasks: 0,
            published_uncommitted_paths: vec![],
            required_checks: 0,
        }
    }

    async fn transfer_worktree(
        &self,
        _lease_id: &str,
        _source_agent_id: &str,
        _target_agent_id: &str,
        _source_owner_revision: u64,
    ) -> ExternalOutcome<()> {
        ExternalOutcome::DefinitelyUnsent {
            code: "COORDINATION_UNAVAILABLE".into(),
            message: "Managed worktree transfer is unavailable in this host.".into(),
        }
    }
}

/// The newest `orchestration_event` a burst produced, handed to the shared
/// coalescer one round at a time. Revisions are monotonic and the renderer
/// ignores the revision it is given, so collapsing a burst onto its newest
/// event loses nothing.
type OrchestrationEventSlot = Arc<Mutex<Option<OrchestrationEventDto>>>;

fn orchestration_push_round(slot: &Mutex<Option<OrchestrationEventDto>>) -> CoalescedPush {
    let event = match slot.lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => return CoalescedPush::Unavailable,
    };
    let Some(event) = event else {
        return CoalescedPush::Idle;
    };
    let Ok(payload) = serde_json::to_value(event) else {
        tracing::error!("orchestration event push could not be encoded");
        return CoalescedPush::Unavailable;
    };
    let Ok(frame) = PushFrame::new(CHANNELS.orchestration_event, payload) else {
        tracing::error!("orchestration event push could not be framed");
        return CoalescedPush::Unavailable;
    };
    CoalescedPush::Frame(frame)
}

#[derive(Clone)]
pub struct OrchestrationHostDispatcher {
    store: Arc<Mutex<OrchestrationStore>>,
    runtime: Arc<dyn OrchestrationRuntimePort>,
    coordination: Arc<dyn OrchestrationCoordinationPort>,
    worktrees: Option<Arc<dyn WorktreeService>>,
    /// The desktop host's process-live collaboration transcript: every event
    /// the observer forwards feeds it, and agent transcripts include it.
    projection: Option<Arc<crate::collaboration::DesktopCollaborationProjection>>,
    runtime_epoch: String,
    gate: Arc<AsyncMutex<()>>,
    closed: Arc<AtomicBool>,
    observer_cancel: CancellationToken,
    observer_tasks: Arc<AsyncMutex<Vec<JoinHandle<()>>>>,
    processing_messages: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    pushes: Option<mpsc::Sender<PushFrame>>,
    orchestration_events: OrchestrationEventSlot,
    orchestration_pushes: Option<RegistryPushCoordinator>,
    registry_pushes: Option<RegistryPushCoordinator>,
    /// False once the host's shared coalescer replaced this dispatcher's own:
    /// `main` closes the shared one after every owner has drained.
    owns_registry_pushes: bool,
    clock: Arc<dyn RuntimeClock>,
    snapshot_cache: SnapshotMemo,
    /// The revision the last successful commit left in the memo. A memo at
    /// this revision is the latest committed state, so readers take it
    /// without the store lock that a commit holds across its fsync.
    committed: Arc<AtomicU64>,
}

impl OrchestrationHostDispatcher {
    #[must_use]
    pub fn new(
        store: Arc<Mutex<OrchestrationStore>>,
        runtime: Arc<dyn OrchestrationRuntimePort>,
        coordination: Arc<dyn OrchestrationCoordinationPort>,
        runtime_epoch: String,
    ) -> Self {
        Self {
            store,
            runtime,
            coordination,
            worktrees: None,
            projection: None,
            runtime_epoch,
            gate: Arc::new(AsyncMutex::new(())),
            closed: Arc::new(AtomicBool::new(false)),
            observer_cancel: CancellationToken::new(),
            observer_tasks: Arc::new(AsyncMutex::new(Vec::new())),
            processing_messages: Arc::new(Mutex::new(HashMap::new())),
            pushes: None,
            orchestration_events: Arc::new(Mutex::new(None)),
            orchestration_pushes: None,
            registry_pushes: None,
            owns_registry_pushes: false,
            clock: Arc::new(SystemRuntimeClock),
            snapshot_cache: Arc::new(Mutex::new(None)),
            committed: Arc::new(AtomicU64::new(NO_COMMITTED_REVISION)),
        }
    }

    #[must_use]
    pub fn with_pushes(mut self, pushes: mpsc::Sender<PushFrame>) -> Self {
        let events = Arc::clone(&self.orchestration_events);
        self.orchestration_pushes = Some(RegistryPushCoordinator::start(
            move || orchestration_push_round(&events),
            pushes.clone(),
        ));
        let runtime = Arc::clone(&self.runtime);
        let revision_runtime = Arc::clone(&self.runtime);
        self.registry_pushes = Some(RegistryPushCoordinator::start_registry(
            move || registry_push_round(runtime.desktop_session_summaries()),
            move || revision_runtime.desktop_registry_content_revision(),
            pushes.clone(),
        ));
        self.owns_registry_pushes = true;
        self.pushes = Some(pushes);
        self
    }

    /// Uses the host's shared registry coalescer instead of this dispatcher's
    /// own; `close` then leaves it running for the other owners.
    #[must_use]
    pub fn with_shared_registry_pushes(mut self, shared: SharedRegistryPushes) -> Self {
        self.registry_pushes = Some(shared.0);
        self.owns_registry_pushes = false;
        self
    }

    #[must_use]
    pub fn with_worktrees(mut self, worktrees: Arc<dyn WorktreeService>) -> Self {
        self.worktrees = Some(worktrees);
        self
    }

    #[must_use]
    pub fn with_projection(
        mut self,
        projection: Arc<crate::collaboration::DesktopCollaborationProjection>,
    ) -> Self {
        self.projection = Some(projection);
        self
    }

    /// An agent's transcript, with the live collaboration projection when
    /// this host keeps one.
    async fn agent_transcript(
        &self,
        session_key: &str,
        lines: usize,
    ) -> Result<Vec<String>, RuntimeError> {
        self.runtime
            .transcript(session_key, lines, self.projection.as_deref())
            .await
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: Arc<dyn RuntimeClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Recovers a prior runtime epoch without repeating any provider effect.
    /// Reserved effects are definitely unsent; started effects are quarantined
    /// unknown. The new epoch becomes visible only after every recovery write.
    ///
    /// # Errors
    ///
    /// Fails closed if canonical orchestration state cannot be read, mutated,
    /// persisted, or projected onto the bounded desktop push stream.
    #[allow(clippy::too_many_lines)]
    pub async fn recover(&self) -> Result<(), DispatchError> {
        let _guard = self.gate.lock().await;
        self.ensure_open()?;
        let snapshot = self.snapshot()?;
        if snapshot.runtime_epoch == self.runtime_epoch {
            return Ok(());
        }
        for effect in snapshot
            .effects
            .iter()
            .filter(|effect| effect.runtime_epoch != self.runtime_epoch)
            .cloned()
            .collect::<Vec<_>>()
        {
            match effect.state {
                OrchestrationEffectState::Reserved => {
                    self.apply(OrchestrationMutation::SettleEffect {
                        effect_id: effect.effect_id.clone(),
                        state: SettledEffectState::Failed,
                        provider_effect_id: None,
                        error_code: Some("HOST_STOPPED_BEFORE_DISPATCH".into()),
                        reconciliation_note: Some(
                            "The old host stopped before its sole provider attempt.".into(),
                        ),
                        meta: self.meta("Closed unattempted provider reservation"),
                    })
                    .await?;
                    self.recover_message_receipt(&effect, MessageDeliveryState::Failed)
                        .await?;
                    if effect.kind == OrchestrationEffectKind::SpawnAgent {
                        let snapshot = self.snapshot()?;
                        if let Some(agent) = snapshot
                            .agents
                            .iter()
                            .find(|agent| agent.agent_id == effect.target_id)
                            && matches!(
                                agent.state,
                                AgentNodeState::Planned | AgentNodeState::Starting
                            )
                        {
                            self.apply(OrchestrationMutation::SetAgentState {
                                agent_id: agent.agent_id.clone(),
                                state: if agent.state == AgentNodeState::Planned {
                                    AgentNodeState::Cancelled
                                } else {
                                    AgentNodeState::Failed
                                },
                                session_key: None,
                                provider_session_id: None,
                                meta: self.meta("Closed an unattempted old agent launch"),
                            })
                            .await?;
                        }
                    }
                }
                OrchestrationEffectState::Started => {
                    self.apply(OrchestrationMutation::SettleEffect {
                        effect_id: effect.effect_id.clone(),
                        state: SettledEffectState::Unknown,
                        provider_effect_id: None,
                        error_code: Some("HOST_STOPPED_AFTER_DISPATCH".into()),
                        reconciliation_note: Some(
                            "Previous host stopped after provider contact; no retry was made."
                                .into(),
                        ),
                        meta: self.meta("Quarantined interrupted provider effect"),
                    })
                    .await?;
                    self.recover_message_receipt(&effect, MessageDeliveryState::Unknown)
                        .await?;
                }
                _ => {}
            }
        }
        let snapshot = self.snapshot()?;
        for lease in snapshot
            .worktree_leases
            .iter()
            .filter(|lease| {
                lease.state == WorktreeLeaseState::Escrow
                    && lease
                        .escrow
                        .as_ref()
                        .is_some_and(|escrow| escrow.state == WorktreeEscrowState::Prepared)
            })
            .cloned()
            .collect::<Vec<_>>()
        {
            let Some(escrow) = lease.escrow.as_ref() else {
                continue;
            };
            let transfer = snapshot.effects.iter().find(|effect| {
                effect.kind == OrchestrationEffectKind::TransferWorktree
                    && effect.target_kind == OrchestrationEffectTargetKind::WorktreeLease
                    && effect.target_id == lease.lease_id
                    && effect.reserved_at == escrow.prepared_at
            });
            let state = match transfer.map(|effect| effect.state) {
                Some(OrchestrationEffectState::Succeeded) => SettledEscrowState::Completed,
                Some(OrchestrationEffectState::Failed) => SettledEscrowState::Failed,
                _ => SettledEscrowState::Unknown,
            };
            self.apply(OrchestrationMutation::SettleWorktreeEscrow {
                lease_id: lease.lease_id,
                state,
                meta: self.meta(match state {
                    SettledEscrowState::Completed => "Recovered managed worktree escrow",
                    SettledEscrowState::Failed => "Closed failed managed worktree escrow",
                    SettledEscrowState::Unknown => "Quarantined ambiguous managed worktree escrow",
                }),
            })
            .await?;
        }
        let snapshot = self.snapshot()?;
        for agent in snapshot
            .agents
            .iter()
            .filter(|agent| {
                agent.runtime_epoch != self.runtime_epoch
                    && matches!(
                        agent.state,
                        AgentNodeState::Starting
                            | AgentNodeState::Ready
                            | AgentNodeState::Working
                            | AgentNodeState::Waiting
                            | AgentNodeState::Idle
                    )
            })
            .cloned()
            .collect::<Vec<_>>()
        {
            self.apply(OrchestrationMutation::SetAgentState {
                agent_id: agent.agent_id,
                state: AgentNodeState::Unknown,
                session_key: None,
                provider_session_id: None,
                meta: self.meta("Quarantined agent from a previous runtime"),
            })
            .await?;
        }
        self.apply(OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch: self.runtime_epoch.clone(),
            meta: self.meta("Started a new orchestration runtime epoch"),
        })
        .await?;
        Ok(())
    }

    async fn recover_message_receipt(
        &self,
        effect: &EffectReservationV1,
        state: MessageDeliveryState,
    ) -> Result<(), DispatchError> {
        if effect.kind != OrchestrationEffectKind::SendMessage {
            return Ok(());
        }
        let snapshot = self.snapshot()?;
        let Some(message) = snapshot
            .messages
            .iter()
            .find(|message| message.message_id == effect.target_id)
        else {
            return Ok(());
        };
        let current = message.receipts.last().map(|receipt| receipt.state);
        if matches!(
            current,
            Some(MessageDeliveryState::Requested | MessageDeliveryState::Queued)
        ) {
            self.append_receipt(
                &message.message_id,
                state,
                "Recovered an interrupted provider delivery without replay.",
            )
            .await?;
        }
        Ok(())
    }

    /// Quiesces mutations and releases the canonical orchestration writer lease.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable store cannot be closed safely.
    pub async fn close(&self) -> Result<(), DispatchError> {
        self.closed.store(true, Ordering::Release);
        self.observer_cancel.cancel();
        let observers = {
            let mut observers = self.observer_tasks.lock().await;
            std::mem::take(&mut *observers)
        };
        for observer in observers {
            let _ = observer.await;
        }
        if let Some(coordinator) = &self.orchestration_pushes {
            coordinator.close().await;
        }
        if self.owns_registry_pushes
            && let Some(coordinator) = &self.registry_pushes
        {
            coordinator.close().await;
        }
        let _guard = self.gate.lock().await;
        let dispatcher = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = dispatcher.store.lock().map_err(|_| store_lock_error())?;
            dispatcher.forget_snapshot();
            store.close().map_err(orchestration_error)
        })
        .await
        .unwrap_or_else(|_| Err(store_lock_error()))
    }

    fn ensure_open(&self) -> Result<(), DispatchError> {
        if self.closed.load(Ordering::Acquire) {
            Err(DispatchError::new(
                "orchestration-unavailable",
                "The orchestration dispatcher is closed.",
                false,
                true,
            ))
        } else {
            Ok(())
        }
    }

    /// Reads the canonical snapshot, memoized on `store.revision()`.
    ///
    /// `OrchestrationStore::snapshot` deep-clones the whole graph, and one
    /// dispatch reads it many times, so the memo is keyed on the revision that
    /// every write moves. A write that *fails* can poison the store without
    /// moving the revision, so `apply` and `close` drop the memo explicitly —
    /// without that, a cache hit would answer where the store now fails closed.
    /// The current snapshot, shared while the store revision is unchanged.
    ///
    /// A commit holds the store lock across its fsync. The memo a successful
    /// commit left is the latest committed state, so a caller that reads it
    /// does not wait for a commit in flight: it reads as if just before that
    /// commit, which the ungated read-only operations already allow. After a
    /// failed write or close, reads take the store lock again.
    pub(crate) fn snapshot(&self) -> Result<Arc<OrchestrationSnapshotV1>, DispatchError> {
        if let Ok(cache) = self.snapshot_cache.lock()
            && let Some((revision, snapshot)) = cache.as_ref()
            && *revision == self.committed.load(Ordering::Acquire)
        {
            return Ok(Arc::clone(snapshot));
        }
        let store = self.store.lock().map_err(|_| store_lock_error())?;
        let revision = store.revision();
        let mut cache = self.snapshot_cache.lock().map_err(|_| store_lock_error())?;
        if let Some((cached_revision, snapshot)) = cache.as_ref()
            && *cached_revision == revision
        {
            return Ok(Arc::clone(snapshot));
        }
        let snapshot = Arc::new(store.snapshot().map_err(orchestration_error)?);
        *cache = Some((revision, Arc::clone(&snapshot)));
        Ok(snapshot)
    }

    /// Every orchestration worker's session key, over the whole agent list
    /// (the control snapshot keeps only 64 agents).
    pub(crate) fn worker_session_keys(&self) -> Result<Vec<String>, DispatchError> {
        Ok(worker_session_keys(&self.snapshot()?.agents))
    }

    fn forget_snapshot(&self) {
        self.committed
            .store(NO_COMMITTED_REVISION, Ordering::Release);
        if let Ok(mut cache) = self.snapshot_cache.lock() {
            *cache = None;
        }
    }

    fn agent_result(&self, agent_id: &str) -> Result<Value, DispatchError> {
        encode(project_orchestration_agent_for_control(find_agent(
            &*self.snapshot()?,
            agent_id,
        )?))
    }

    fn agent_envelope(&self, agent_id: &str) -> Result<Value, DispatchError> {
        encode(json!({
            "agent": project_orchestration_agent_for_control(find_agent(
                &*self.snapshot()?,
                agent_id,
            )?)
        }))
    }

    fn meta(&self, summary: impl Into<String>) -> OrchestrationMutationMetaV1 {
        host_meta_at(self.clock.now(), summary)
    }

    fn human_meta(&self, summary: impl Into<String>) -> OrchestrationMutationMetaV1 {
        human_meta_at(self.clock.now(), summary)
    }

    /// Commits one mutation. The store re-reads, re-serializes and fsyncs the
    /// whole snapshot file, so the commit runs on the blocking pool; the std
    /// store mutex still orders writers exactly as before. The commit and its
    /// push scheduling run together there, so a caller that stops waiting
    /// cannot separate a committed revision from its desktop event.
    async fn apply(&self, mutation: OrchestrationMutation) -> Result<(), DispatchError> {
        let dispatcher = self.clone();
        tokio::task::spawn_blocking(move || dispatcher.commit(mutation))
            .await
            .unwrap_or_else(|_| {
                // A panicking commit poisoned the store lock; whether it wrote
                // is unknown, so the memo is dropped and nothing is replayed.
                self.forget_snapshot();
                Err(store_lock_error())
            })
    }

    fn commit(&self, mutation: OrchestrationMutation) -> Result<(), DispatchError> {
        let next = {
            let mut store = self.store.lock().map_err(|_| store_lock_error())?;
            let revision = store.revision();
            let applied = store.apply(revision, mutation);
            if applied.is_err() {
                self.forget_snapshot();
            }
            // The store returns the snapshot it just committed; keep it as the
            // read memo so the next read does not deep-clone the graph again.
            let next = Arc::new(applied.map_err(orchestration_error)?);
            if let Ok(mut cache) = self.snapshot_cache.lock() {
                let revision = store.revision();
                *cache = Some((revision, Arc::clone(&next)));
                self.committed.store(revision, Ordering::Release);
            }
            next
        };
        if let Some(coordinator) = &self.orchestration_pushes {
            let stored = self
                .orchestration_events
                .lock()
                .map(|mut slot| {
                    *slot = Some(OrchestrationEventDto {
                        version: 1,
                        kind: "changed".into(),
                        host_id: next.host_id.clone(),
                        runtime_epoch: next.runtime_epoch.clone(),
                        revision: next.revision,
                    });
                })
                .is_ok();
            if !stored || !coordinator.schedule() {
                return Err(DispatchError::new(
                    "orchestration-push-unavailable",
                    "Orchestration state was saved, but its desktop event could not be queued.",
                    true,
                    false,
                ));
            }
        }
        Ok(())
    }

    async fn append_receipt(
        &self,
        message_id: &str,
        state: MessageDeliveryState,
        detail: &str,
    ) -> Result<(), DispatchError> {
        let at = self.clock.now();
        self.apply(OrchestrationMutation::AppendMessageReceipt {
            message_id: message_id.into(),
            receipt: MessageDeliveryReceiptV1 {
                version: 1,
                receipt_id: format!("receipt_{}", Uuid::new_v4()),
                state,
                at: at.clone(),
                provider_receipt_id: None,
                error_code: None,
                detail: Some(detail.into()),
            },
            meta: host_meta_at(at, format!("Message {state:?}")),
        })
        .await?;
        Ok(())
    }

    fn snapshot_result(&self) -> Result<Value, DispatchError> {
        let projected = project_orchestration_snapshot_for_control(&*self.snapshot()?)
            .map_err(orchestration_error)?;
        encode(projected)
    }

    fn timeline(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationTimelineRequest =
            decode(CHANNELS.read_orchestration_timeline, payload)?;
        let limit = request.limit.unwrap_or(MAX_TIMELINE_EVENTS as u64);
        if limit == 0 || limit > MAX_TIMELINE_EVENTS as u64 {
            return Err(invalid_payload(
                CHANNELS.read_orchestration_timeline,
                "limit must be an integer from 1 through 64",
            ));
        }
        encode(
            project_orchestration_events_for_control(
                &*self.snapshot()?,
                EventProjectionInput {
                    sequence: request.sequence,
                    limit: usize::try_from(limit).unwrap_or(MAX_TIMELINE_EVENTS),
                },
            )
            .map_err(orchestration_error)?,
        )
    }

    fn inbox(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationInboxRequest =
            decode(CHANNELS.read_orchestration_inbox, payload)?;
        let limit = request.limit.unwrap_or(MAX_INBOX_MESSAGES as u64);
        if limit == 0 || limit > MAX_INBOX_MESSAGES as u64 {
            return Err(invalid_payload(
                CHANNELS.read_orchestration_inbox,
                "limit must be an integer from 1 through 4",
            ));
        }
        let page = project_orchestration_inbox_for_control(
            &*self.snapshot()?,
            &InboxProjectionInput {
                agent_id: request.agent_id,
                thread_id: request.thread_id,
                before_message_id: request.before_message_id,
                limit: usize::try_from(limit).unwrap_or(MAX_INBOX_MESSAGES),
            },
        )
        .map_err(orchestration_error)?
        .ok_or_else(|| not_found("Inbox page cursor not found."))?;
        encode(page)
    }

    async fn transcript(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationTranscriptRequest =
            decode(CHANNELS.read_orchestration_transcript, payload)?;
        if request.lines == 0 || request.lines > MAX_TRANSCRIPT_LINES {
            return Err(invalid_payload(
                CHANNELS.read_orchestration_transcript,
                "lines must be an integer from 1 through 2000",
            ));
        }
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        let key = agent.session_key.ok_or_else(|| {
            blocked("Agent has no provider session from which to read a transcript.")
        })?;
        let provider_limit = request.lines.min(64);
        let lines = self
            .agent_transcript(&key, usize::try_from(provider_limit).unwrap_or(64))
            .await
            .map_err(runtime_error)?;
        encode(
            project_orchestration_transcript_for_control(&TranscriptProjectionInput {
                agent_id: request.agent_id,
                requested_lines: request.lines,
                lines,
                source_was_truncated: request.lines > provider_limit,
            })
            .map_err(orchestration_error)?,
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn start_adversarial_review(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: AdversarialReviewStartRequestV1 =
            decode(CHANNELS.start_adversarial_review, payload)?;
        if !matches!(
            (
                request.builder_engine_id.as_str(),
                request.reviewer_engine_id.as_str()
            ),
            ("claude", "codex") | ("codex", "claude")
        ) {
            return Err(invalid_payload(
                CHANNELS.start_adversarial_review,
                "builderEngineId and reviewerEngineId must be the cross-model Claude/Codex pair",
            ));
        }
        if request.client_request_id.is_empty()
            || request.client_request_id.len() > 256
            || request.client_request_id.chars().any(char::is_control)
        {
            return Err(invalid_payload(
                CHANNELS.start_adversarial_review,
                "clientRequestId must be a bounded printable identifier",
            ));
        }
        let objective_limit =
            interface_contracts::orchestration::ORCHESTRATION_LIMITS.body_bytes - 1_024;
        if request.objective.trim().is_empty()
            || request.objective.len() > objective_limit
            || request.objective.contains('\0')
        {
            return Err(invalid_payload(
                CHANNELS.start_adversarial_review,
                "objective must be non-empty and fit within the bounded agent prompt",
            ));
        }
        let canonical_cwd = tokio::fs::canonicalize(&request.canonical_cwd)
            .await
            .map_err(|_| blocked("Adversarial review cwd is unavailable."))?;
        if !is_directory(&canonical_cwd).await {
            return Err(blocked(
                "Adversarial review cwd must be an existing directory.",
            ));
        }
        let canonical_cwd = canonical_cwd.to_string_lossy().into_owned();
        let pair_token = adversarial_pair_token(&request.client_request_id);
        let reviewer_id = format!("agent_adversarial_{pair_token}_reviewer");
        let builder_id = format!("agent_adversarial_{pair_token}_builder");
        self.preflight_adversarial_reviewer(
            &request.reviewer_engine_id,
            &canonical_cwd,
            &reviewer_id,
        )
        .await?;
        let reviewer_role = format!("{ADVERSARIAL_REVIEWER_ROLE_PREFIX}{pair_token}");
        let builder_role = format!("{ADVERSARIAL_BUILDER_ROLE_PREFIX}{pair_token}");
        let reviewer_prompt = format!(
            "You are the read-only adversarial reviewer for a cross-model coding pair. Do not modify files or perform implementation work. Wait for the host's /review packet, then critically inspect the builder's work against this objective and return only concrete findings and verification advice.\n\nObjective:\n{}",
            request.objective
        );
        let builder_prompt = format!(
            "You are the builder in a cross-model adversarial-review pair. Complete the objective in the project checkout. When you become idle, Interface will ask the independent reviewer to inspect your result and will deliver its feedback for one final correction pass.\n\nObjective:\n{}",
            request.objective
        );
        let required_capabilities = Some(vec![
            OrchestrationCapabilityFacet::Spawn,
            OrchestrationCapabilityFacet::Message,
            OrchestrationCapabilityFacet::Transcript,
        ]);
        self.create_agent(
            serde_json::to_value(OrchestrationAgentCreateRequestV1 {
                agent_id: Some(reviewer_id.clone()),
                engine_id: request.reviewer_engine_id,
                name: "Adversarial reviewer".into(),
                prompt: reviewer_prompt,
                canonical_cwd: canonical_cwd.clone(),
                access_mode: "read-only".into(),
                model: None,
                effort: None,
                parent_agent_id: None,
                role: Some(reviewer_role),
                required_capabilities: required_capabilities.clone(),
                client_request_id: format!("adversarial-review:{pair_token}:reviewer"),
            })
            .map_err(encode_error)?,
        )
        .await?;
        self.create_agent(
            serde_json::to_value(OrchestrationAgentCreateRequestV1 {
                agent_id: Some(builder_id.clone()),
                engine_id: request.builder_engine_id,
                name: "Adversarial builder".into(),
                prompt: builder_prompt,
                canonical_cwd,
                access_mode: "project".into(),
                model: None,
                effort: None,
                parent_agent_id: None,
                role: Some(builder_role),
                required_capabilities,
                client_request_id: format!("adversarial-review:{pair_token}:builder"),
            })
            .map_err(encode_error)?,
        )
        .await?;
        let snapshot = self.snapshot()?;
        encode(AdversarialReviewStartV1 {
            version: 1,
            builder: project_orchestration_agent_for_control(find_agent(&snapshot, &builder_id)?),
            reviewer: project_orchestration_agent_for_control(find_agent(&snapshot, &reviewer_id)?),
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn create_agent(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentCreateRequestV1 =
            decode(CHANNELS.create_orchestration_agent, payload)?;
        validate_access_mode(CHANNELS.create_orchestration_agent, &request.access_mode)?;
        let canonical_cwd = tokio::fs::canonicalize(&request.canonical_cwd)
            .await
            .map_err(|_| blocked("Agent cwd is unavailable."))?;
        if !is_directory(&canonical_cwd).await {
            return Err(blocked("Agent cwd must be an existing directory."));
        }
        let canonical_cwd = canonical_cwd.to_string_lossy().into_owned();
        let request_hash = sha256_json(&request)?;
        let idempotency_key = format!("manual-spawn:{}", request.client_request_id);
        let snapshot = self.snapshot()?;
        if let Some(effect) = snapshot
            .effects
            .iter()
            .find(|effect| effect.idempotency_key == idempotency_key)
        {
            if effect.request_hash != request_hash
                || effect.kind != OrchestrationEffectKind::SpawnAgent
            {
                return Err(conflict(
                    "clientRequestId already belongs to a different agent launch.",
                ));
            }
            let agent = find_agent(&snapshot, &effect.target_id)?;
            return encode(project_orchestration_agent_for_control(agent));
        }
        let capabilities = agent_capabilities(self.runtime.as_ref(), &request.engine_id).await;
        let mut required_capabilities = request
            .required_capabilities
            .clone()
            .unwrap_or_else(|| vec![OrchestrationCapabilityFacet::Spawn]);
        if matches!(request.access_mode.as_str(), "isolated" | "coordinated")
            && !required_capabilities.contains(&OrchestrationCapabilityFacet::Worktree)
        {
            required_capabilities.push(OrchestrationCapabilityFacet::Worktree);
        }
        if let Some(missing) = required_capabilities.iter().find(|required| {
            !capabilities.iter().any(|capability| {
                capability.facet == **required
                    && capability.state == OrchestrationCapabilityState::Available
            })
        }) {
            return Err(blocked(format!(
                "Provider lacks {} capability.",
                capability_name(*missing)
            )));
        }
        let parent = request
            .parent_agent_id
            .as_deref()
            .map(|agent_id| find_agent(&snapshot, agent_id).cloned())
            .transpose()?;
        if parent.as_ref().is_some_and(|agent| agent.depth != 0) {
            return Err(blocked(
                "Delegation is one level deep; a worker cannot take workers.",
            ));
        }
        if request.access_mode == "coordinated" {
            return Err(blocked(
                "Coordinated agent launch requires an active coordinator claim before provider contact.",
            ));
        }
        let prepared_worktree = if request.access_mode == "isolated" {
            let worktrees = self.worktrees.as_ref().ok_or_else(|| {
                blocked("Isolated agent launch requires the shared managed-worktree authority.")
            })?;
            Some(
                worktrees
                    .create_initialized(canonical_cwd.clone(), Some(request.name.clone()))
                    .await
                    .map_err(|error| {
                        DispatchError::new(
                            "worktree-creation-failed",
                            error.message,
                            false,
                            error.replay_safe,
                        )
                    })?,
            )
        } else {
            None
        };
        let canonical_project_root = prepared_worktree.as_ref().map_or_else(
            || {
                parent
                    .as_ref()
                    .and_then(|agent| agent.scope.canonical_project_root.clone())
                    .unwrap_or_else(|| canonical_cwd.clone())
            },
            |worktree| worktree.repo_root.to_string_lossy().into_owned(),
        );
        if let Some(parent_root) = parent.as_ref().and_then(|agent| {
            agent
                .scope
                .canonical_project_root
                .as_deref()
                .or(Some(agent.scope.canonical_cwd.as_str()))
        }) && parent_root != canonical_project_root
        {
            if let (Some(worktree), Some(worktrees)) =
                (prepared_worktree.clone(), self.worktrees.as_ref())
            {
                let _ = worktrees.discard(worktree).await;
            }
            return Err(blocked(
                "A child agent must remain inside its parent's canonical project root.",
            ));
        }
        let agent_id = request
            .agent_id
            .clone()
            .unwrap_or_else(|| format!("agent_{}", Uuid::new_v4()));
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.agent_id == agent_id)
        {
            if let (Some(worktree), Some(worktrees)) =
                (prepared_worktree.clone(), self.worktrees.as_ref())
            {
                let _ = worktrees.discard(worktree).await;
            }
            return Err(conflict("agentId already exists."));
        }
        let now = self.clock.now();
        let scope = AgentScopeV1 {
            canonical_cwd: canonical_cwd.clone(),
            canonical_project_root: Some(canonical_project_root.clone()),
            access_mode: request.access_mode.clone(),
            // `planRunId`/`taskRunId` were always null for an agent the canvas
            // created, and the digest is persisted on every stored agent. The
            // two null members stay so an existing scope digest still matches.
            scope_digest: sha256_json(&json!({
                "canonicalCwd": canonical_cwd,
                "canonicalProjectRoot": canonical_project_root,
                "accessMode": request.access_mode,
                "planRunId": Value::Null,
                "taskRunId": Value::Null,
            }))?,
            worktree_lease_id: None,
        };
        let agent = AgentNodeV1 {
            version: 1,
            agent_id: agent_id.clone(),
            name: Some(request.name.clone()),
            engine_id: request.engine_id.clone(),
            session_key: None,
            provider_session_id: None,
            runtime_epoch: self.runtime_epoch.clone(),
            parent_agent_id: request.parent_agent_id.clone(),
            role: request.role.clone(),
            depth: parent.as_ref().map_or(0, |agent| agent.depth + 1),
            state: AgentNodeState::Planned,
            capabilities,
            scope,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let effect = self.reserved_effect(
            OrchestrationEffectKind::SpawnAgent,
            OrchestrationEffectTargetKind::Agent,
            agent_id.clone(),
            idempotency_key,
            request_hash,
            now.clone(),
        );
        self.apply(OrchestrationMutation::CreateAgentIntent {
            agent,
            effect: effect.clone(),
            meta: human_meta_at(now, format!("Registered manual agent {}", request.name)),
        })
        .await?;
        self.apply(OrchestrationMutation::SetAgentState {
            agent_id: agent_id.clone(),
            state: AgentNodeState::Starting,
            session_key: None,
            provider_session_id: None,
            meta: self.meta("Starting manual agent"),
        })
        .await?;
        self.apply(OrchestrationMutation::StartEffect {
            effect_id: effect.effect_id.clone(),
            meta: self.meta("Crossed provider boundary for manual agent"),
        })
        .await?;
        let provider_cwd = prepared_worktree.as_ref().map_or_else(
            || canonical_cwd.clone(),
            |worktree| worktree.session_cwd.to_string_lossy().into_owned(),
        );
        let outcome = self
            .runtime
            .launch(RuntimeLaunchRequest {
                engine_id: request.engine_id,
                name: request.name,
                prompt: request.prompt,
                canonical_cwd: provider_cwd,
                access_mode: request.access_mode,
                model: request.model,
                effort: request.effort,
            })
            .await;
        match outcome {
            ExternalOutcome::Succeeded {
                value,
                provider_effect_id,
            } => {
                let settlement = self.meta("Provider made manual agent ready");
                let lease = if let Some(worktree) = prepared_worktree.as_ref() {
                    let Some(worktrees) = self.worktrees.as_ref() else {
                        return Err(blocked("Managed-worktree authority disappeared."));
                    };
                    if let Err(error) = worktrees.persist(&value.session_key, worktree).await {
                        self.apply(OrchestrationMutation::SettleEffect {
                            effect_id: effect.effect_id.clone(),
                            state: SettledEffectState::Unknown,
                            provider_effect_id: provider_effect_id
                                .clone()
                                .or(value.provider_effect_id.clone()),
                            error_code: Some("WORKTREE_LEDGER_WRITE_FAILED".into()),
                            reconciliation_note: Some(error.message),
                            meta: self
                                .meta("Provider is ready but managed-worktree persistence failed"),
                        })
                        .await?;
                        self.apply(OrchestrationMutation::SetAgentState {
                            agent_id: agent_id.clone(),
                            state: AgentNodeState::Unknown,
                            session_key: Some(value.session_key),
                            provider_session_id: Some(value.provider_session_id),
                            meta: self.meta(
                                "Quarantined provider-ready agent after local persistence failure",
                            ),
                        })
                        .await?;
                        self.push_desktop_registry().await;
                        return self.agent_result(&agent_id);
                    }
                    let canonical_path = tokio::fs::canonicalize(&worktree.path)
                        .await
                        .unwrap_or_else(|_| worktree.path.clone());
                    let canonical_session_cwd = tokio::fs::canonicalize(&worktree.session_cwd)
                        .await
                        .unwrap_or_else(|_| worktree.session_cwd.clone());
                    let repository_root = tokio::fs::canonicalize(&worktree.repo_root)
                        .await
                        .unwrap_or_else(|_| worktree.repo_root.clone());
                    Some(WorktreeLeaseV1 {
                        version: 1,
                        lease_id: format!("lease_{}", Uuid::new_v4()),
                        canonical_path: canonical_path.to_string_lossy().into_owned(),
                        canonical_cwd: Some(canonical_session_cwd.to_string_lossy().into_owned()),
                        repository_root: repository_root.to_string_lossy().into_owned(),
                        branch: Some(worktree.branch.clone()),
                        head: None,
                        dirty_state_digest: sha256_json(&json!({
                            "dirtySource": worktree.dirty_source,
                        }))?,
                        owner_agent_id: agent_id.clone(),
                        owner_revision: 0,
                        state: WorktreeLeaseState::Active,
                        escrow: None,
                        coordinator: None,
                        acquired_at: settlement.at.clone(),
                        updated_at: settlement.at.clone(),
                        released_at: None,
                    })
                } else {
                    None
                };
                self.apply(OrchestrationMutation::SettleAgentLaunch {
                    agent_id: agent_id.clone(),
                    effect_id: effect.effect_id,
                    session_key: value.session_key,
                    provider_session_id: value.provider_session_id,
                    provider_effect_id: provider_effect_id.or(value.provider_effect_id),
                    lease,
                    meta: settlement,
                })
                .await?;
                self.apply(OrchestrationMutation::SetAgentState {
                    agent_id: agent_id.clone(),
                    state: AgentNodeState::Working,
                    session_key: None,
                    provider_session_id: None,
                    meta: self.meta("Manual agent began work"),
                })
                .await?;
            }
            ExternalOutcome::DefinitelyUnsent { code, message } => {
                self.apply(OrchestrationMutation::SettleEffect {
                    effect_id: effect.effect_id,
                    state: SettledEffectState::Failed,
                    provider_effect_id: None,
                    error_code: Some(code),
                    reconciliation_note: Some(message),
                    meta: self.meta("Manual agent launch failed"),
                })
                .await?;
                self.apply(OrchestrationMutation::SetAgentState {
                    agent_id: agent_id.clone(),
                    state: AgentNodeState::Failed,
                    session_key: None,
                    provider_session_id: None,
                    meta: self.meta("Manual agent launch failed"),
                })
                .await?;
                if let (Some(worktree), Some(worktrees)) =
                    (prepared_worktree, self.worktrees.as_ref())
                {
                    let _ = worktrees.discard(worktree).await;
                }
            }
            ExternalOutcome::Unknown { code, message } => {
                self.apply(OrchestrationMutation::SettleEffect {
                    effect_id: effect.effect_id,
                    state: SettledEffectState::Unknown,
                    provider_effect_id: None,
                    error_code: Some(code),
                    reconciliation_note: Some(message),
                    meta: self.meta("Manual agent launch outcome is unknown"),
                })
                .await?;
                self.apply(OrchestrationMutation::SetAgentState {
                    agent_id: agent_id.clone(),
                    state: AgentNodeState::Unknown,
                    session_key: None,
                    provider_session_id: None,
                    meta: self.meta("Manual agent launch outcome is unknown"),
                })
                .await?;
            }
        }
        let snapshot = self.snapshot()?;
        let agent = find_agent(&snapshot, &agent_id)?.clone();
        if agent.state == AgentNodeState::Working
            && let Some(session_key) = agent.session_key.clone()
        {
            self.observe_agent(agent.agent_id.clone(), session_key)
                .await;
        }
        self.push_desktop_registry().await;
        encode(project_orchestration_agent_for_control(&agent))
    }

    #[allow(clippy::too_many_lines)]
    async fn send_message(&self, payload: Value, reply: bool) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentSendRequestV1 = decode(
            if reply {
                CHANNELS.reply_orchestration_message
            } else {
                CHANNELS.send_orchestration_message
            },
            payload,
        )?;
        let snapshot = self.snapshot()?;
        let recipient = find_agent(&snapshot, &request.recipient_agent_id)?.clone();
        let key = recipient.session_key.clone().ok_or_else(|| {
            blocked("Recipient has no provider session and cannot receive host messages.")
        })?;
        if request
            .sender_agent_id
            .as_deref()
            .is_some_and(|sender| !snapshot.agents.iter().any(|agent| agent.agent_id == sender))
        {
            return Err(not_found("Sender agent does not exist."));
        }
        let parent = if reply {
            let parent_id = request
                .parent_message_id
                .as_deref()
                .ok_or_else(|| invalid_payload("agent.reply", "parentMessageId is required"))?;
            Some(
                snapshot
                    .messages
                    .iter()
                    .find(|message| message.message_id == parent_id)
                    .cloned()
                    .ok_or_else(|| not_found("Reply parent message does not exist."))?,
            )
        } else {
            None
        };
        if let Some(parent) = &parent
            && request
                .thread_id
                .as_deref()
                .is_some_and(|thread| thread != parent.thread_id)
        {
            return Err(conflict(
                "Reply threadId does not match its parent message.",
            ));
        }
        let request_hash = sha256_json(&request)?;
        let idempotency_key = format!("message:{}", request.client_request_id);
        if let Some(effect) = snapshot
            .effects
            .iter()
            .find(|effect| effect.idempotency_key == idempotency_key)
        {
            if effect.kind != OrchestrationEffectKind::SendMessage
                || effect.request_hash != request_hash
            {
                return Err(conflict(
                    "clientRequestId already belongs to a different message.",
                ));
            }
            let message = snapshot
                .messages
                .iter()
                .find(|message| message.message_id == effect.target_id)
                .ok_or_else(|| blocked("The durable message intent is incomplete."))?;
            return encode(project_orchestration_message_for_control(message, true));
        }
        let at = self.clock.now();
        let message_id = format!("message_{}", Uuid::new_v4());
        let message = MailboxMessageV1 {
            version: 1,
            message_id: message_id.clone(),
            thread_id: parent
                .as_ref()
                .map(|message| message.thread_id.clone())
                .or(request.thread_id)
                .unwrap_or_else(|| format!("thread_{}", Uuid::new_v4())),
            parent_message_id: parent.map(|message| message.message_id),
            origin: if request.sender_agent_id.is_some() {
                MessageOrigin::Agent
            } else {
                MessageOrigin::Human
            },
            sender_agent_id: request.sender_agent_id,
            recipient_agent_id: request.recipient_agent_id,
            body_digest: sha256_text(&request.body),
            body_bytes: u64::try_from(request.body.len()).unwrap_or(u64::MAX),
            body: Some(request.body.clone()),
            expect_reply: request.expect_reply,
            effect_id: None,
            notice_summary: None,
            receipts: vec![MessageDeliveryReceiptV1 {
                version: 1,
                receipt_id: format!("receipt_{}", Uuid::new_v4()),
                state: MessageDeliveryState::Requested,
                at: at.clone(),
                provider_receipt_id: None,
                error_code: None,
                detail: None,
            }],
            created_at: at.clone(),
            updated_at: at.clone(),
        };
        let effect = self.reserved_effect(
            OrchestrationEffectKind::SendMessage,
            OrchestrationEffectTargetKind::Message,
            message_id.clone(),
            idempotency_key,
            request_hash,
            at.clone(),
        );
        self.apply(OrchestrationMutation::EnqueueMessageIntent {
            message,
            effect: effect.clone(),
            meta: human_meta_at(at, "Requested agent message"),
        })
        .await?;
        self.append_receipt(
            &message_id,
            MessageDeliveryState::Queued,
            "Message queued by the orchestration host.",
        )
        .await?;
        self.apply(OrchestrationMutation::StartEffect {
            effect_id: effect.effect_id.clone(),
            meta: self.meta("Crossed provider boundary for message delivery"),
        })
        .await?;
        let outcome = self.runtime.send(&key, request.body).await;
        self.settle_effect(&effect.effect_id, &outcome, "message delivery")
            .await?;
        match outcome {
            ExternalOutcome::Succeeded { .. } => {
                self.append_receipt(
                    &message_id,
                    MessageDeliveryState::ProviderAccepted,
                    "Provider accepted the message.",
                )
                .await?;
                self.append_receipt(
                    &message_id,
                    MessageDeliveryState::Settled,
                    "Delivery effect settled exactly once.",
                )
                .await?;
                if matches!(
                    recipient.state,
                    AgentNodeState::Ready | AgentNodeState::Waiting | AgentNodeState::Idle
                ) {
                    let current = find_agent(&*self.snapshot()?, &recipient.agent_id)?.clone();
                    if matches!(
                        current.state,
                        AgentNodeState::Ready | AgentNodeState::Waiting | AgentNodeState::Idle
                    ) {
                        self.apply(OrchestrationMutation::SetAgentState {
                            agent_id: current.agent_id.clone(),
                            state: AgentNodeState::Working,
                            session_key: None,
                            provider_session_id: None,
                            meta: self.meta("Delivered message started an observed provider turn"),
                        })
                        .await?;
                        self.processing_messages
                            .lock()
                            .map_err(|_| store_lock_error())?
                            .entry(current.agent_id)
                            .or_default()
                            .insert(message_id.clone());
                    }
                }
            }
            ExternalOutcome::DefinitelyUnsent { message, .. } => {
                self.append_receipt(&message_id, MessageDeliveryState::Failed, &message)
                    .await?;
            }
            ExternalOutcome::Unknown { .. } => {
                self.append_receipt(
                    &message_id,
                    MessageDeliveryState::Unknown,
                    "Delivery may have crossed the provider boundary; Interface will not resend it.",
                )
                .await?;
            }
        }
        let snapshot = self.snapshot()?;
        let message = snapshot
            .messages
            .iter()
            .find(|message| message.message_id == message_id)
            .expect("durable message remains present");
        encode(project_orchestration_message_for_control(message, true))
    }

    async fn stop_agent(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentStopRequest =
            decode(CHANNELS.stop_orchestration_agent, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        if agent_state_is_terminal(agent.state) {
            return encode(project_orchestration_agent_for_control(&agent));
        }
        let key = agent
            .session_key
            .clone()
            .ok_or_else(|| blocked("Agent has no provider session to stop."))?;
        let effect = self
            .reserve_agent_effect(&agent, OrchestrationEffectKind::StopAgent, None)
            .await?;
        let outcome = self.runtime.stop(&key).await;
        self.settle_effect(&effect.effect_id, &outcome, "stop-agent")
            .await?;
        let state = match outcome {
            ExternalOutcome::Succeeded { .. } => AgentNodeState::Cancelled,
            ExternalOutcome::DefinitelyUnsent { message, .. } => {
                return Err(DispatchError::new(
                    "effect-definitely-unsent",
                    message,
                    false,
                    true,
                ));
            }
            ExternalOutcome::Unknown { .. } => AgentNodeState::Unknown,
        };
        self.apply(OrchestrationMutation::SetAgentState {
            agent_id: agent.agent_id.clone(),
            state,
            session_key: None,
            provider_session_id: None,
            meta: self.human_meta("Human stopped agent"),
        })
        .await?;
        self.agent_result(&agent.agent_id)
    }

    async fn steer_agent(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentSteerRequest =
            decode(CHANNELS.steer_orchestration_agent, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        if !agent_is_settleable(agent.state) {
            return Err(blocked(format!(
                "Agent cannot be steered while {:?}.",
                agent.state
            )));
        }
        let key = agent
            .session_key
            .clone()
            .ok_or_else(|| blocked("Agent has no provider session to steer."))?;
        let stable = (
            format!("steer:{}", request.client_request_id),
            sha256_json(&json!({"agentId": request.agent_id, "body": request.body}))?,
        );
        let snapshot = self.snapshot()?;
        if let Some(effect) = snapshot
            .effects
            .iter()
            .find(|effect| effect.idempotency_key == stable.0)
        {
            if effect.request_hash != stable.1
                || effect.target_id != agent.agent_id
                || effect.kind != OrchestrationEffectKind::WakeAgent
            {
                return Err(conflict(
                    "clientRequestId already belongs to a different agent steer.",
                ));
            }
            return encode(project_orchestration_agent_for_control(&agent));
        }
        if !agent.capabilities.iter().any(|capability| {
            capability.facet == OrchestrationCapabilityFacet::Wake
                && capability.state == OrchestrationCapabilityState::Available
        }) {
            return Err(blocked(
                "This agent did not advertise a safe wake/steer capability.",
            ));
        }
        let effect = self
            .reserve_agent_effect(&agent, OrchestrationEffectKind::WakeAgent, Some(stable))
            .await?;
        let outcome = self.runtime.send(&key, request.body).await;
        self.settle_effect(&effect.effect_id, &outcome, "wake-agent")
            .await?;
        let next = match outcome {
            ExternalOutcome::Succeeded { .. } => AgentNodeState::Working,
            ExternalOutcome::DefinitelyUnsent { message, .. } => {
                return Err(DispatchError::new(
                    "effect-definitely-unsent",
                    message,
                    false,
                    true,
                ));
            }
            ExternalOutcome::Unknown { .. } => AgentNodeState::Unknown,
        };
        if agent.state != next {
            self.apply(OrchestrationMutation::SetAgentState {
                agent_id: agent.agent_id.clone(),
                state: next,
                session_key: None,
                provider_session_id: None,
                meta: self.human_meta("Agent state changed after steer"),
            })
            .await?;
        }
        self.agent_result(&agent.agent_id)
    }

    async fn claim_role(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationRoleClaimRequest =
            decode(CHANNELS.claim_orchestration_role, payload)?;
        let snapshot = self.snapshot()?;
        let agent = find_agent(&snapshot, &request.agent_id)?;
        let at = self.clock.now();
        self.apply(OrchestrationMutation::ClaimRole {
            claim: RoleClaimV1 {
                version: 1,
                claim_id: format!("claim_{}", Uuid::new_v4()),
                agent_id: request.agent_id,
                role: request.role,
                responsibility: request.responsibility,
                scope_digest: agent.scope.scope_digest.clone(),
                state: RoleClaimState::Active,
                claimed_at: at.clone(),
                updated_at: at.clone(),
                relinquished_at: None,
            },
            meta: human_meta_at(at, "Agent claimed an advisory role"),
        })
        .await?;
        self.snapshot_result()
    }

    async fn relinquish_role(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationRoleRelinquishRequest =
            decode(CHANNELS.relinquish_orchestration_role, payload)?;
        self.apply(OrchestrationMutation::RelinquishRole {
            claim_id: request.claim_id,
            meta: self.human_meta("Role claim relinquished"),
        })
        .await?;
        self.snapshot_result()
    }

    #[allow(clippy::too_many_lines)]
    async fn set_policy(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestratorPolicySetRequestV1 =
            decode(CHANNELS.set_orchestrator_policy, payload)?;
        let event_id = idempotent_event_id("set-orchestrator-policy", &request.client_request_id);
        let snapshot = self.snapshot()?;
        if snapshot
            .events
            .iter()
            .any(|event| event.event_id == event_id)
        {
            let policy = snapshot
                .orchestrator_policies
                .iter()
                .find(|policy| policy.agent_id == request.agent_id)
                .ok_or_else(|| blocked("The replayed policy event has no policy."))?;
            return encode(json!({"policy": policy}));
        }
        find_agent(&snapshot, &request.agent_id)?;
        let existing = snapshot
            .orchestrator_policies
            .iter()
            .find(|policy| policy.agent_id == request.agent_id);
        let at = self.clock.now();
        let max_workers = request
            .max_workers
            .or_else(|| existing.map(|policy| policy.max_workers))
            .unwrap_or(ORCHESTRATOR_POLICY_DEFAULTS.max_workers);
        let max_concurrent = request
            .max_concurrent
            .or_else(|| existing.map(|policy| policy.max_concurrent))
            .unwrap_or(ORCHESTRATOR_POLICY_DEFAULTS.max_concurrent);
        let max_delegations = request
            .max_delegations
            .or_else(|| existing.map(|policy| policy.max_delegations))
            .unwrap_or(ORCHESTRATOR_POLICY_DEFAULTS.max_delegations);
        let wall_clock_ms = request
            .wall_clock_ms
            .or_else(|| existing.map(|policy| policy.wall_clock_ms))
            .unwrap_or(ORCHESTRATOR_POLICY_DEFAULTS.wall_clock_ms);
        if !(ORCHESTRATOR_POLICY_BOUNDS.max_workers.0..=ORCHESTRATOR_POLICY_BOUNDS.max_workers.1)
            .contains(&max_workers)
            || !(ORCHESTRATOR_POLICY_BOUNDS.max_concurrent.0
                ..=ORCHESTRATOR_POLICY_BOUNDS.max_concurrent.1)
                .contains(&max_concurrent)
            || !(ORCHESTRATOR_POLICY_BOUNDS.max_delegations.0
                ..=ORCHESTRATOR_POLICY_BOUNDS.max_delegations.1)
                .contains(&max_delegations)
            || !(ORCHESTRATOR_POLICY_BOUNDS.wall_clock_ms.0
                ..=ORCHESTRATOR_POLICY_BOUNDS.wall_clock_ms.1)
                .contains(&wall_clock_ms)
        {
            return Err(invalid_payload(
                CHANNELS.set_orchestrator_policy,
                "policy bounds are invalid",
            ));
        }
        let policy = OrchestratorPolicyV1 {
            version: 1,
            agent_id: request.agent_id,
            autonomy: request
                .autonomy
                .or_else(|| existing.map(|policy| policy.autonomy))
                .unwrap_or(OrchestratorAutonomy::Active),
            max_access_mode: request
                .max_access_mode
                .or_else(|| existing.map(|policy| policy.max_access_mode))
                .unwrap_or(ORCHESTRATOR_POLICY_DEFAULTS.max_access_mode),
            max_workers,
            max_concurrent,
            max_delegations,
            wall_clock_ms,
            activated_at: existing.map_or_else(|| at.clone(), |policy| policy.activated_at.clone()),
            updated_at: at.clone(),
            delegations_used: existing.map_or(0, |policy| policy.delegations_used),
            spawns_used: existing.map_or(0, |policy| policy.spawns_used),
            integration: request
                .integration
                .or_else(|| existing.and_then(|policy| policy.integration))
                .or(Some(ORCHESTRATOR_POLICY_DEFAULTS.integration)),
            coordination_required: request
                .coordination_required
                .or_else(|| existing.and_then(|policy| policy.coordination_required))
                .or(Some(ORCHESTRATOR_POLICY_DEFAULTS.coordination_required)),
            allow_unverified_fence: request
                .allow_unverified_fence
                .or_else(|| existing.and_then(|policy| policy.allow_unverified_fence))
                .or(Some(ORCHESTRATOR_POLICY_DEFAULTS.allow_unverified_fence)),
            workers_write: request
                .workers_write
                .or_else(|| existing.and_then(|policy| policy.workers_write)),
            model_catalogue: request
                .model_catalogue
                .or_else(|| existing.and_then(|policy| policy.model_catalogue))
                .or(Some(ORCHESTRATOR_POLICY_DEFAULTS.model_catalogue)),
            model_allowlist: request
                .model_allowlist
                .or_else(|| existing.and_then(|policy| policy.model_allowlist.clone())),
            model_notes: request
                .model_notes
                .or_else(|| existing.and_then(|policy| policy.model_notes.clone())),
        };
        self.apply(OrchestrationMutation::SetOrchestratorPolicy {
            policy: policy.clone(),
            meta: OrchestrationMutationMetaV1 {
                event_id,
                ..human_meta_at(at, "Set orchestrator policy")
            },
        })
        .await?;
        encode(json!({"policy": policy}))
    }

    async fn clear_policy(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestratorPolicyClearRequestV1 =
            decode(CHANNELS.clear_orchestrator_policy, payload)?;
        let event_id = idempotent_event_id("clear-orchestrator-policy", &request.client_request_id);
        let snapshot = self.snapshot()?;
        if !snapshot
            .events
            .iter()
            .any(|event| event.event_id == event_id)
        {
            self.apply(OrchestrationMutation::ClearOrchestratorPolicy {
                agent_id: request.agent_id.clone(),
                meta: OrchestrationMutationMetaV1 {
                    event_id,
                    ..self.human_meta("Cleared orchestrator policy")
                },
            })
            .await?;
        }
        encode(json!({"agentId": request.agent_id}))
    }

    async fn bind_parent(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentBindParentRequestV1 =
            decode(CHANNELS.bind_orchestration_parent, payload)?;
        let event_id = idempotent_event_id("bind-agent-parent", &request.client_request_id);
        let snapshot = self.snapshot()?;
        if !snapshot
            .events
            .iter()
            .any(|event| event.event_id == event_id)
        {
            self.apply(OrchestrationMutation::BindAgentParent {
                agent_id: request.agent_id.clone(),
                parent_agent_id: request.parent_agent_id,
                meta: OrchestrationMutationMetaV1 {
                    event_id,
                    ..self.human_meta("Changed orchestration lineage")
                },
            })
            .await?;
        }
        self.agent_envelope(&request.agent_id)
    }

    async fn register_session(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationAgentRegisterSessionRequestV1 =
            decode(CHANNELS.register_orchestration_session, payload)?;
        let snapshot = self.snapshot()?;
        if let Some(agent) = snapshot
            .agents
            .iter()
            .find(|agent| agent.session_key.as_deref() == Some(&request.session_key))
        {
            return encode(json!({"agent": project_orchestration_agent_for_control(agent)}));
        }
        let description = self
            .runtime
            .describe_session(&request.session_key)
            .map_err(runtime_error)?
            .ok_or_else(|| not_found("That provider session is not available."))?;
        let at = self.clock.now();
        let agent_id = format!("agent_{}", Uuid::new_v4());
        let capabilities = agent_capabilities(self.runtime.as_ref(), &description.engine_id).await;
        let canonical_project_root = description
            .canonical_project_root
            .clone()
            .unwrap_or_else(|| description.canonical_cwd.clone());
        let agent = AgentNodeV1 {
            version: 1,
            agent_id: agent_id.clone(),
            name: request.name.or(description.name),
            engine_id: description.engine_id,
            session_key: Some(request.session_key),
            provider_session_id: Some(description.provider_session_id),
            runtime_epoch: self.runtime_epoch.clone(),
            parent_agent_id: None,
            role: None,
            depth: 0,
            state: description.state,
            capabilities,
            scope: AgentScopeV1 {
                canonical_cwd: description.canonical_cwd.clone(),
                canonical_project_root: Some(canonical_project_root.clone()),
                access_mode: "project".into(),
                scope_digest: sha256_json(&json!({
                    "canonicalCwd": description.canonical_cwd,
                    "canonicalProjectRoot": canonical_project_root,
                    "accessMode": "project",
                }))?,
                worktree_lease_id: None,
            },
            created_at: at.clone(),
            updated_at: at.clone(),
        };
        self.apply(OrchestrationMutation::RegisterAgentFromSession {
            agent,
            meta: OrchestrationMutationMetaV1 {
                event_id: idempotent_event_id(
                    "register-agent-from-session",
                    &request.client_request_id,
                ),
                ..human_meta_at(at, "Registered provider session")
            },
        })
        .await?;
        self.agent_envelope(&agent_id)
    }

    #[allow(clippy::too_many_lines)]
    async fn escrow_worktree(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: OrchestrationWorktreeEscrowRequest =
            decode(CHANNELS.escrow_orchestration_worktree, payload)?;
        let snapshot = self.snapshot()?;
        let lease = snapshot
            .worktree_leases
            .iter()
            .find(|lease| lease.lease_id == request.lease_id)
            .cloned()
            .ok_or_else(|| not_found("Managed worktree lease not found."))?;
        if lease.coordinator.is_some() {
            return Err(blocked(
                "A coordinated checkout belongs to its coordinator task and cannot be transferred.",
            ));
        }
        if lease.owner_agent_id != request.source_agent_id
            || lease.owner_revision != request.source_owner_revision
            || lease.state != WorktreeLeaseState::Active
        {
            return Err(conflict(
                "Managed worktree ownership changed before escrow.",
            ));
        }
        find_agent(&snapshot, &request.source_agent_id)?;
        find_agent(&snapshot, &request.target_agent_id)?;
        let idempotency_key = format!(
            "worktree-transfer:{}:{}",
            request.lease_id, request.source_owner_revision
        );
        let request_hash = sha256_json(&request)?;
        if let Some(effect) = snapshot
            .effects
            .iter()
            .find(|effect| effect.idempotency_key == idempotency_key)
        {
            if effect.request_hash != request_hash {
                return Err(conflict(
                    "This worktree owner revision belongs to a different transfer.",
                ));
            }
            return self.snapshot_result();
        }
        let at = self.clock.now();
        let effect = self.reserved_effect(
            OrchestrationEffectKind::TransferWorktree,
            OrchestrationEffectTargetKind::WorktreeLease,
            request.lease_id.clone(),
            idempotency_key,
            request_hash,
            at.clone(),
        );
        self.apply(OrchestrationMutation::BeginWorktreeEscrowIntent {
            lease_id: request.lease_id.clone(),
            escrow: WorktreeEscrowV1 {
                escrow_id: format!("escrow_{}", Uuid::new_v4()),
                source_agent_id: request.source_agent_id.clone(),
                target_agent_id: request.target_agent_id.clone(),
                source_owner_revision: request.source_owner_revision,
                state: WorktreeEscrowState::Prepared,
                prepared_at: at.clone(),
                settled_at: None,
            },
            effect: effect.clone(),
            meta: human_meta_at(at, "Prepared managed worktree ownership escrow"),
        })
        .await?;
        self.apply(OrchestrationMutation::StartEffect {
            effect_id: effect.effect_id.clone(),
            meta: self.meta("Crossed provider boundary for managed worktree transfer"),
        })
        .await?;
        let outcome = self
            .coordination
            .transfer_worktree(
                &request.lease_id,
                &request.source_agent_id,
                &request.target_agent_id,
                request.source_owner_revision,
            )
            .await;
        self.settle_effect(&effect.effect_id, &outcome, "worktree-transfer")
            .await?;
        self.apply(OrchestrationMutation::SettleWorktreeEscrow {
            lease_id: request.lease_id,
            state: match outcome {
                ExternalOutcome::Succeeded { .. } => SettledEscrowState::Completed,
                ExternalOutcome::DefinitelyUnsent { .. } => SettledEscrowState::Failed,
                ExternalOutcome::Unknown { .. } => SettledEscrowState::Unknown,
            },
            meta: self.meta("Settled managed worktree escrow"),
        })
        .await?;
        self.snapshot_result()
    }

    async fn worker_submit(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: WorkerSubmitRequestV1 = decode(CHANNELS.submit_coordinated_worker, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        let result = self.coordination.submit(&agent).await.map_err(blocked)?;
        encode(OrchestratorSubmitResultV1 {
            submission_id: result.submission_id,
            changes: result.changes,
            state: result.state,
        })
    }

    async fn worker_integrate(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: WorkerIntegrateRequestV1 =
            decode(CHANNELS.integrate_coordinated_worker, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        let dry_run = request.dry_run.unwrap_or(false);
        let result = self
            .coordination
            .integrate(&agent, dry_run)
            .await
            .map_err(blocked)?;
        encode(json!({
            "agentId": request.agent_id,
            "taskId": result.task_id,
            "dryRun": dry_run,
            "state": "verifying",
        }))
    }

    async fn worker_abandon(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: WorkerAbandonRequestV1 = decode(CHANNELS.abandon_coordinated_task, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        self.coordination.abandon(&agent).await.map_err(blocked)?;
        encode(json!({"agentId": request.agent_id, "abandoned": true}))
    }

    async fn worker_ack(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: WorkerAckRequestV1 = decode(CHANNELS.ack_coordinated_obligation, payload)?;
        let agent = find_agent(&*self.snapshot()?, &request.agent_id)?.clone();
        let pending = self
            .coordination
            .acknowledge(&agent, &request.keep)
            .await
            .map_err(blocked)?;
        encode(json!({
            "agentId": request.agent_id,
            "kept": request.keep,
            "pending": pending,
        }))
    }

    async fn coordination_status(&self, payload: Value) -> Result<Value, DispatchError> {
        let request: CoordinationStatusRequestV1 =
            decode(CHANNELS.get_coordination_status, payload)?;
        encode(
            self.coordination
                .status(&request.canonical_project_root)
                .await,
        )
    }

    async fn models(&self, payload: &Value) -> Result<Value, DispatchError> {
        ensure_empty(CHANNELS.get_orchestrator_models, payload)?;
        let engine_ids = self.runtime.engine_ids();
        // A cold catalogue probes each provider behind its own lock, so ask
        // them together: the first call waits for the slowest, not the sum.
        let catalogues = futures::future::join_all(
            engine_ids
                .iter()
                .map(|engine_id| self.runtime.model_catalogue(engine_id)),
        )
        .await;
        let mut engines = Vec::with_capacity(engine_ids.len());
        for (engine_id, models) in engine_ids.into_iter().zip(catalogues) {
            let ok = models.as_ref().is_some_and(|items| !items.is_empty());
            let default = models.as_ref().and_then(|items| {
                items
                    .iter()
                    .find(|model| model.is_default == Some(true))
                    .map(|model| model.value.clone())
            });
            engines.push(OrchestratorEngineModelsV1 {
                engine_id,
                ok,
                default,
                models: models
                    .unwrap_or_default()
                    .into_iter()
                    .map(|model| OrchestratorModelRowV1 {
                        id: model.value,
                        name: model.display_name,
                        efforts: model.effort_levels,
                        default: model.is_default,
                        note: model.description,
                    })
                    .collect(),
            });
        }
        let live = engines.iter().filter(|engine| engine.ok).count();
        encode(OrchestratorModelsV1 {
            catalogue: if live == 0 {
                OrchestratorCatalogueState::Unavailable
            } else if live == engines.len() {
                OrchestratorCatalogueState::Live
            } else {
                OrchestratorCatalogueState::Partial
            },
            engines,
            synthetic: OrchestratorSyntheticModelsV1 {
                ultracode: ULTRACODE_NOTE.into(),
            },
            guidance: None,
            allowlist: None,
        })
    }
}

#[async_trait]
impl HostDispatcher for OrchestrationHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        self.ensure_open()?;
        match operation {
            op if op == CHANNELS.get_orchestration_snapshot => {
                ensure_empty(operation, &payload)?;
                self.snapshot_result()
            }
            op if op == CHANNELS.read_orchestration_timeline => self.timeline(payload),
            op if op == CHANNELS.read_orchestration_transcript => self.transcript(payload).await,
            op if op == CHANNELS.read_orchestration_inbox => self.inbox(payload),
            op if op == CHANNELS.get_coordination_status => self.coordination_status(payload).await,
            op if op == CHANNELS.get_orchestrator_models => self.models(&payload).await,
            _ => {
                let _guard = self.gate.lock().await;
                self.ensure_open()?;
                match operation {
                    op if op == CHANNELS.start_adversarial_review => {
                        self.start_adversarial_review(payload).await
                    }
                    op if op == CHANNELS.create_orchestration_agent => {
                        self.create_agent(payload).await
                    }
                    op if op == CHANNELS.send_orchestration_message => {
                        self.send_message(payload, false).await
                    }
                    op if op == CHANNELS.reply_orchestration_message => {
                        self.send_message(payload, true).await
                    }
                    op if op == CHANNELS.stop_orchestration_agent => self.stop_agent(payload).await,
                    op if op == CHANNELS.steer_orchestration_agent => {
                        self.steer_agent(payload).await
                    }
                    op if op == CHANNELS.claim_orchestration_role => self.claim_role(payload).await,
                    op if op == CHANNELS.relinquish_orchestration_role => {
                        self.relinquish_role(payload).await
                    }
                    op if op == CHANNELS.escrow_orchestration_worktree => {
                        self.escrow_worktree(payload).await
                    }
                    op if op == CHANNELS.set_orchestrator_policy => self.set_policy(payload).await,
                    op if op == CHANNELS.clear_orchestrator_policy => {
                        self.clear_policy(payload).await
                    }
                    op if op == CHANNELS.bind_orchestration_parent => {
                        self.bind_parent(payload).await
                    }
                    op if op == CHANNELS.register_orchestration_session => {
                        self.register_session(payload).await
                    }
                    op if op == CHANNELS.submit_coordinated_worker => {
                        self.worker_submit(payload).await
                    }
                    op if op == CHANNELS.integrate_coordinated_worker => {
                        self.worker_integrate(payload).await
                    }
                    op if op == CHANNELS.abandon_coordinated_task => {
                        self.worker_abandon(payload).await
                    }
                    op if op == CHANNELS.ack_coordinated_obligation => {
                        self.worker_ack(payload).await
                    }
                    _ => Err(DispatchError::new(
                        "operation-not-migrated",
                        "This Interface operation has not migrated to the Rust host.",
                        false,
                        true,
                    )),
                }
            }
        }
    }
}

async fn agent_capabilities(
    runtime: &dyn OrchestrationRuntimePort,
    engine_id: &str,
) -> Vec<AgentCapabilityV1> {
    if !runtime
        .engine_ids()
        .iter()
        .any(|registered| registered == engine_id)
    {
        return unavailable_capabilities(
            "No active Interface adapter is registered for this provider.",
        );
    }
    let history = runtime.supports_provider_history(engine_id);
    let probe = runtime.probe_orchestration_engine(engine_id).await;
    derived_capabilities(history, probe)
}

fn derived_capabilities(
    supports_history: bool,
    probe: Option<RuntimeEngineProbe>,
) -> Vec<AgentCapabilityV1> {
    vec![
        available_capability(OrchestrationCapabilityFacet::Reference),
        if supports_history {
            available_capability(OrchestrationCapabilityFacet::Transcript)
        } else {
            unavailable_capability(
                OrchestrationCapabilityFacet::Transcript,
                "This provider does not expose stored transcript replay.",
            )
        },
        available_capability(OrchestrationCapabilityFacet::Message),
        if probe.is_some_and(|info| info.steer) {
            available_capability(OrchestrationCapabilityFacet::Wake)
        } else {
            unavailable_capability(
                OrchestrationCapabilityFacet::Wake,
                "This provider cannot safely steer a live turn.",
            )
        },
        if probe.is_some() {
            available_capability(OrchestrationCapabilityFacet::Spawn)
        } else {
            unavailable_capability(
                OrchestrationCapabilityFacet::Spawn,
                "The provider could not be probed by the active Interface host.",
            )
        },
        available_capability(OrchestrationCapabilityFacet::Worktree),
    ]
}

fn available_capability(facet: OrchestrationCapabilityFacet) -> AgentCapabilityV1 {
    AgentCapabilityV1 {
        facet,
        state: OrchestrationCapabilityState::Available,
        reason: None,
    }
}

fn unavailable_capability(facet: OrchestrationCapabilityFacet, reason: &str) -> AgentCapabilityV1 {
    AgentCapabilityV1 {
        facet,
        state: OrchestrationCapabilityState::Unavailable,
        reason: Some(reason.to_owned()),
    }
}

fn unavailable_capabilities(reason: &str) -> Vec<AgentCapabilityV1> {
    [
        OrchestrationCapabilityFacet::Reference,
        OrchestrationCapabilityFacet::Transcript,
        OrchestrationCapabilityFacet::Message,
        OrchestrationCapabilityFacet::Wake,
        OrchestrationCapabilityFacet::Spawn,
        OrchestrationCapabilityFacet::Worktree,
    ]
    .into_iter()
    .map(|facet| unavailable_capability(facet, reason))
    .collect()
}

const fn agent_is_settleable(state: AgentNodeState) -> bool {
    matches!(
        state,
        AgentNodeState::Ready
            | AgentNodeState::Working
            | AgentNodeState::Waiting
            | AgentNodeState::Idle
    )
}

const fn capability_name(facet: OrchestrationCapabilityFacet) -> &'static str {
    match facet {
        OrchestrationCapabilityFacet::Reference => "reference",
        OrchestrationCapabilityFacet::Transcript => "transcript",
        OrchestrationCapabilityFacet::Message => "message",
        OrchestrationCapabilityFacet::Wake => "wake",
        OrchestrationCapabilityFacet::Spawn => "spawn",
        OrchestrationCapabilityFacet::Worktree => "worktree",
    }
}

fn find_agent<'a>(
    snapshot: &'a OrchestrationSnapshotV1,
    agent_id: &str,
) -> Result<&'a AgentNodeV1, DispatchError> {
    snapshot
        .agents
        .iter()
        .find(|agent| agent.agent_id == agent_id)
        .ok_or_else(|| not_found("Agent not found."))
}

/// Builds host-actor mutation metadata around a timestamp the caller already
/// read, so a single clock read reaches both a record and the event
/// describing it.
fn host_meta_at(at: String, summary: impl Into<String>) -> OrchestrationMutationMetaV1 {
    OrchestrationMutationMetaV1 {
        event_id: format!("event_{}", Uuid::new_v4()),
        at,
        actor: OrchestrationActorV1 {
            kind: OrchestrationActorKind::Host,
            actor_id: "interface-host".into(),
        },
        summary: summary.into(),
    }
}

fn human_meta_at(at: String, summary: impl Into<String>) -> OrchestrationMutationMetaV1 {
    OrchestrationMutationMetaV1 {
        actor: OrchestrationActorV1 {
            kind: OrchestrationActorKind::Human,
            actor_id: "desktop-or-cli-user".into(),
        },
        ..host_meta_at(at, summary)
    }
}

fn sha256_text(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

fn validate_access_mode(operation: &str, access_mode: &str) -> Result<(), DispatchError> {
    if matches!(
        access_mode,
        "read-only" | "coordinated" | "isolated" | "project"
    ) {
        Ok(())
    } else {
        Err(invalid_payload(
            operation,
            "accessMode must be one of read-only, coordinated, isolated, project",
        ))
    }
}

fn sha256_json(value: &impl Serialize) -> Result<String, DispatchError> {
    let encoded = serde_json::to_vec(value).map_err(encode_error)?;
    Ok(sha256_hex(&encoded))
}

fn idempotent_event_id(kind: &str, client_request_id: &str) -> String {
    format!(
        "event_{}",
        &sha256_text(&format!("{kind}:{client_request_id}"))[..48]
    )
}

fn decode<T: DeserializeOwned>(operation: &str, payload: Value) -> Result<T, DispatchError> {
    serde_json::from_value(payload).map_err(|error| invalid_payload(operation, error.to_string()))
}

fn ensure_empty(operation: &str, payload: &Value) -> Result<(), DispatchError> {
    crate::payload::require_empty(payload, || {
        invalid_payload(operation, "operation does not accept a payload")
    })
}

fn encode(value: impl Serialize) -> Result<Value, DispatchError> {
    serde_json::to_value(value).map_err(encode_error)
}

#[allow(clippy::needless_pass_by_value)]
fn encode_error(error: serde_json::Error) -> DispatchError {
    DispatchError::new(
        "orchestration-encode-failed",
        error.to_string(),
        false,
        true,
    )
}

fn invalid_payload(operation: &str, message: impl Into<String>) -> DispatchError {
    DispatchError::new(
        INVALID_PAYLOAD_CODE,
        format!("Invalid payload for {operation}: {}", message.into()),
        false,
        true,
    )
}

fn not_found(message: impl Into<String>) -> DispatchError {
    DispatchError::new("orchestration-not-found", message, false, true)
}

fn blocked(message: impl Into<String>) -> DispatchError {
    DispatchError::new("orchestration-blocked", message, false, true)
}

fn conflict(message: impl Into<String>) -> DispatchError {
    DispatchError::new("orchestration-conflict", message, true, true)
}

/// Whether `path` is an existing directory, checked off the async workers.
async fn is_directory(path: &std::path::Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

fn store_lock_error() -> DispatchError {
    DispatchError::new(
        "orchestration-store-unavailable",
        "The orchestration store lock was poisoned.",
        false,
        false,
    )
}

fn orchestration_error(error: OrchestrationError) -> DispatchError {
    let replay_safe = !matches!(
        error.code,
        OrchestrationErrorCode::WriteUncertain | OrchestrationErrorCode::StorePoisoned
    );
    let retryable = matches!(
        error.code,
        OrchestrationErrorCode::CasConflict
            | OrchestrationErrorCode::WriterLeaseLost
            | OrchestrationErrorCode::WriteFailed
    );
    DispatchError::new(
        match error.code {
            OrchestrationErrorCode::NotFound => "orchestration-not-found",
            OrchestrationErrorCode::AlreadyExists
            | OrchestrationErrorCode::IdempotencyConflict
            | OrchestrationErrorCode::CasConflict => "orchestration-conflict",
            OrchestrationErrorCode::WriteUncertain | OrchestrationErrorCode::StorePoisoned => {
                "effect-outcome-unknown"
            }
            OrchestrationErrorCode::UnsafeStorage
            | OrchestrationErrorCode::StateCorrupt
            | OrchestrationErrorCode::WriterLeaseLost => "orchestration-storage-unsafe",
            _ => "orchestration-blocked",
        },
        error.message,
        retryable,
        replay_safe,
    )
}

fn runtime_error(error: RuntimeError) -> DispatchError {
    DispatchError::new(
        "orchestration-runtime-failed",
        error.message,
        error.retryable,
        matches!(error.certainty, EffectCertainty::DefinitelyUnsent),
    )
}

#[cfg(test)]
#[path = "orchestration_tests.rs"]
mod tests;
