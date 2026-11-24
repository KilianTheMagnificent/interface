//! `DurableCollaborationService`: the durable drive loop over the
//! coordinator, previews and approvals, the approval sweep, terminal cleanup
//! and restart reconciliation.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use interface_collaboration::{
    ApprovalAction, CollaborationCoordinator, CollaborationMcpAccessRequest,
    CollaborationMcpIntent, CollaborationModelAuthority, CollaborationModelIngress,
    CoordinatorError, IngressCancellation, LockedSessionRole, McpIngressError, PacketBuildResult,
    PeerAction, PendingPeer, SourceSnapshot,
    coordinator::HumanPreviewMaterial,
    state::{
        CollaborationErrorCode, CollaborationIntentInput, CollaborationLiveStatus,
        CollaborationMode as DomainMode, CollaborationOrigin, CollaborationReceiptV1,
        CollaborationState as DomainState, effect_inputs_required, maintenance_effect_for_state,
    },
};
use interface_contracts::{
    engine::EngineEvent,
    ipc::{
        CHANNELS, CollaborationApprovalAction, CollaborationDraftStateUpdate,
        CollaborationEventDto, CollaborationPeerAction, CollaborationState, CollaborationStatusDto,
        CollaborationSubmitRequest,
    },
};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::{
    CollaborationDesktopControl, CollaborationPortError, CollaborationRuntimePort,
    CollaborationServiceError, CollaborationSessionHooks, DesktopCollaborationProjection,
    ProjectedSendOutcome, SessionRuntimeCollaborationPort, engine_domain, mode_domain, status_dto,
    validate_identifier,
};
use crate::{dispatcher::DispatchError, protocol::PushFrame};

const APPROVAL_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const UNKNOWN_RECONCILIATION_MARKER: &str = "host-recovery-unknown-failed";

#[derive(Default)]
pub(super) struct DriveState {
    pub(super) active: HashSet<String>,
    dirty: HashSet<String>,
    pub(super) source_index: HashMap<String, HashSet<String>>,
    /// Collaborations whose last drive found them waiting on a person or on
    /// restart (awaiting approval or peer review, or unknown). Source events
    /// skip them; every explicit schedule and every terminal cleanup
    /// un-parks, so a missed update costs only a spawn, never a wakeup.
    pub(super) parked: HashSet<String>,
}

/// How a drive left its collaboration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DriveEnd {
    Done,
    /// Waiting on a person or on restart; only an explicit action moves it.
    Parked,
}

impl DriveState {
    pub(super) fn remember_source(&mut self, source_session_key: &str, collaboration_id: &str) {
        self.source_index
            .entry(source_session_key.to_owned())
            .or_default()
            .insert(collaboration_id.to_owned());
    }

    pub(super) fn forget_source(&mut self, source_session_key: &str, collaboration_id: &str) {
        self.parked.remove(collaboration_id);
        let Some(ids) = self.source_index.get_mut(source_session_key) else {
            return;
        };
        ids.remove(collaboration_id);
        if ids.is_empty() {
            self.source_index.remove(source_session_key);
        }
    }

    pub(super) fn source_has(&self, source_session_key: &str) -> bool {
        self.source_index
            .get(source_session_key)
            .is_some_and(|ids| !ids.is_empty())
    }

    /// The source's collaborations that a source event can move (not parked).
    pub(super) fn source_collaborations(&self, source_session_key: &str) -> Vec<String> {
        self.source_index
            .get(source_session_key)
            .map(|ids| {
                ids.iter()
                    .filter(|id| !self.parked.contains(*id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub(super) struct CollaborationServiceInner<P> {
    coordinator: Arc<Mutex<CollaborationCoordinator>>,
    port: Arc<P>,
    projection: Arc<DesktopCollaborationProjection>,
    runtime_epoch: String,
    pushes: Option<mpsc::Sender<PushFrame>>,
    cancellation: CancellationToken,
    pub(super) drives: StdMutex<DriveState>,
    drive_changed: Notify,
}

pub struct DurableCollaborationService<P> {
    pub(super) inner: Arc<CollaborationServiceInner<P>>,
    approval_sweep: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<P> CollaborationServiceInner<P> {
    /// Runs one durable coordinator transition on the blocking pool. Each one
    /// commits the journal and appends the audit segment, several fsyncs, so
    /// it must not run on a Tokio worker. The coordinator lock is held for
    /// exactly the closure, as wide as before, and still admits callers in
    /// FIFO order. A transition that panics leaves the in-memory journal
    /// unproven: the coordinator stops admitting work and the caller gets an
    /// ambiguous, never-replayed failure.
    pub(super) async fn transition<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut CollaborationCoordinator) -> Result<T, CoordinatorError>
        + Send
        + 'static,
    ) -> Result<T, CoordinatorError> {
        let mut coordinator = Arc::clone(&self.coordinator).lock_owned().await;
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&mut coordinator)))
                .unwrap_or_else(|_| {
                    coordinator.quiesce();
                    Err(interrupted_transition())
                })
        })
        .await
        .unwrap_or_else(|_| Err(interrupted_transition()))
    }
}

fn interrupted_transition() -> CoordinatorError {
    tracing::error!(
        "a collaboration transition was interrupted; admission stopped and nothing is replayed"
    );
    CoordinatorError::Ingress {
        code: CollaborationErrorCode::RecoveryRequired,
        collaboration_id: None,
        retryable: false,
    }
}

/// Startup-order seam used by the host: the owner-private MCP broker can bind
/// before provider adapters are constructed, while all ingress remains
/// fail-closed until the recovered durable service is attached exactly once.
#[derive(Default)]
pub struct DeferredCollaborationModelIngress {
    target: StdMutex<Option<Arc<dyn CollaborationModelIngress>>>,
}

impl DeferredCollaborationModelIngress {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target: StdMutex::new(None),
        }
    }

    pub fn attach(
        &self,
        target: Arc<dyn CollaborationModelIngress>,
    ) -> Result<(), CollaborationServiceError> {
        let mut current = self.target.lock().map_err(|_| {
            CollaborationServiceError::unavailable()
                .with_diagnostic("collaboration model-ingress lock was poisoned")
        })?;
        if current.is_some() {
            return Err(CollaborationServiceError::unavailable()
                .with_diagnostic("collaboration model ingress was already attached"));
        }
        *current = Some(target);
        Ok(())
    }

    pub(super) fn target(&self) -> Result<Arc<dyn CollaborationModelIngress>, McpIngressError> {
        self.target
            .lock()
            .ok()
            .and_then(|target| target.clone())
            .ok_or_else(|| McpIngressError::new(CollaborationErrorCode::BrokerUnavailable))
    }
}

#[async_trait]
impl CollaborationModelIngress for DeferredCollaborationModelIngress {
    async fn submit(
        &self,
        intent: CollaborationMcpIntent,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.target()?.submit(intent, authority, cancellation).await
    }

    async fn status(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.target()?
            .status(request, authority, cancellation)
            .await
    }

    async fn cancel(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.target()?
            .cancel(request, authority, cancellation)
            .await
    }
}

impl<P> DurableCollaborationService<P>
where
    P: CollaborationRuntimePort,
{
    pub async fn start(
        mut coordinator: CollaborationCoordinator,
        port: Arc<P>,
        projection: Arc<DesktopCollaborationProjection>,
        runtime_epoch: String,
        pushes: Option<mpsc::Sender<PushFrame>>,
    ) -> Result<Self, CollaborationServiceError> {
        validate_identifier(&runtime_epoch)?;
        coordinator
            .recover()
            .map_err(CollaborationServiceError::domain)?;
        reconcile_unknown_records(&mut coordinator)?;
        let inner = Arc::new(CollaborationServiceInner {
            coordinator: Arc::new(Mutex::new(coordinator)),
            port,
            projection: Arc::clone(&projection),
            runtime_epoch,
            pushes,
            cancellation: CancellationToken::new(),
            drives: StdMutex::new(DriveState::default()),
            drive_changed: Notify::new(),
        });
        let weak = Arc::downgrade(&inner);
        projection.set_source_changed(Arc::new(move |session_key| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let has_live = inner
                .drives
                .lock()
                .ok()
                .is_some_and(|drives| drives.source_has(session_key));
            if !has_live {
                return;
            }
            schedule_source_collaborations(&inner, session_key);
        }));
        let service = Self {
            inner,
            approval_sweep: StdMutex::new(None),
        };
        service.restore_projections_and_drives().await?;
        let sweep = spawn_approval_sweep(&service.inner);
        *service
            .approval_sweep
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sweep);
        Ok(service)
    }

    #[must_use]
    pub fn projection(&self) -> Arc<DesktopCollaborationProjection> {
        Arc::clone(&self.inner.projection)
    }

    pub fn observe_event(
        &self,
        session_key: &str,
        event: &EngineEvent,
    ) -> Result<(), CollaborationServiceError> {
        self.inner.projection.observe_event(session_key, event)
    }

    pub fn user_send_started(&self, session_key: &str) -> Result<(), CollaborationServiceError> {
        self.inner.projection.user_send_started(session_key)
    }

    pub fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), CollaborationServiceError> {
        self.inner
            .projection
            .user_send_settled(session_key, text, outcome)
    }

    async fn restore_projections_and_drives(&self) -> Result<(), CollaborationServiceError> {
        let (receipts, peers) = {
            let coordinator = &mut *self.inner.coordinator.lock().await;
            let receipts = coordinator
                .all_receipts()
                .map_err(CollaborationServiceError::domain)?;
            let peers = coordinator
                .pending_peers()
                .map_err(CollaborationServiceError::domain)?;
            for receipt in &receipts {
                if receipt.state.is_terminal() {
                    continue;
                }
                if let Ok(participants) = coordinator.human_participants(&receipt.collaboration_id)
                {
                    sync_source_index(
                        &self.inner,
                        &participants.source_session_key,
                        &receipt.collaboration_id,
                        false,
                    );
                }
            }
            (receipts, peers)
        };
        for peer in peers {
            self.inner
                .port
                .hold_peer(&peer, &self.inner.projection)
                .await
                .map_err(port_service_error)?;
        }
        for receipt in receipts {
            if should_drive(receipt.state) {
                schedule_drive(Arc::clone(&self.inner), receipt.collaboration_id);
            }
        }
        Ok(())
    }

    async fn status_for(
        &self,
        collaboration_id: &str,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        status_for_inner(&self.inner, collaboration_id).await
    }

    async fn publish(&self, collaboration_id: &str) {
        publish_status(&self.inner, collaboration_id).await;
    }

    /// Builds the status once, pushes it to the desktop when it could be
    /// built, and returns it.
    async fn publish_and_status(
        &self,
        collaboration_id: &str,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let status = self.status_for(collaboration_id).await;
        if let Ok(status) = &status
            && self.inner.pushes.is_some()
        {
            publish_status_dto(&self.inner, status.clone()).await;
        }
        status
    }

    async fn resolve_preview(
        &self,
        material: &HumanPreviewMaterial,
    ) -> Result<(SourceSnapshot, PacketBuildResult), CollaborationPortError> {
        resolve_preview(&self.inner, material).await
    }

    async fn cleanup_terminal(&self, collaboration_id: &str) {
        cleanup_terminal(&self.inner, collaboration_id).await;
    }
}

#[async_trait]
impl<P> CollaborationDesktopControl for DurableCollaborationService<P>
where
    P: CollaborationRuntimePort,
{
    async fn list(&self) -> Result<Vec<CollaborationStatusDto>, CollaborationServiceError> {
        let mut coordinator = self.inner.coordinator.lock().await;
        let receipts = coordinator
            .active_receipts()
            .map_err(CollaborationServiceError::domain)?;
        let mut peers = coordinator
            .pending_peers()
            .map_err(CollaborationServiceError::domain)?
            .into_iter()
            .map(|peer| (peer.collaboration_id.clone(), peer))
            .collect::<HashMap<_, _>>();
        let mut statuses = Vec::with_capacity(receipts.len());
        for receipt in receipts {
            let collaboration_id = receipt.collaboration_id.clone();
            let participants = coordinator
                .human_participants(&collaboration_id)
                .map_err(CollaborationServiceError::domain)?;
            let approval = coordinator
                .approval_preview(&collaboration_id)
                .map_err(CollaborationServiceError::domain)?;
            let peer = peers.remove(&collaboration_id);
            statuses.push(status_dto(receipt, participants, approval, peer));
        }
        Ok(statuses)
    }

    async fn submit(
        &self,
        input: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let began = self
            .inner
            .transition(move |coordinator| {
                coordinator.begin_human(CollaborationIntentInput {
                    version: Some(1),
                    mode: mode_domain(input.mode),
                    source_session_key: input.source_session_key,
                    target_engine: engine_domain(input.target_engine),
                    objective: input.objective,
                    client_request_id: input.client_request_id,
                    origin: CollaborationOrigin::Human,
                })
            })
            .await;
        // Even a refused admission may already have expired others.
        unpark_after_admission(&self.inner);
        let receipt = began.map_err(CollaborationServiceError::domain)?;
        if receipt.state == DomainState::Requested {
            let material = {
                self.inner
                    .coordinator
                    .lock()
                    .await
                    .human_preview_material(&receipt.collaboration_id)
                    .map_err(CollaborationServiceError::domain)?
            };
            let preview = self.resolve_preview(&material).await;
            complete_preview(&self.inner, &receipt.collaboration_id, preview)
                .await
                .map_err(CollaborationServiceError::domain)?;
        }
        let status = self.publish_and_status(&receipt.collaboration_id).await?;
        sync_source_index(
            &self.inner,
            &status.source_session_key,
            &status.receipt.collaboration_id,
            matches!(
                status.receipt.state,
                CollaborationState::Completed
                    | CollaborationState::Denied
                    | CollaborationState::Cancelled
                    | CollaborationState::Failed
            ),
        );
        Ok(status)
    }

    async fn status(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        self.status_for(&collaboration_id).await
    }

    async fn approve(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let domain_action = ApprovalAction {
            collaboration_id: action.collaboration_id.clone(),
            approval_id: action.approval_id,
            expected_revision: action.expected_revision,
        };
        // Expiry and the state check share one lock, as before, so an
        // approval can never observe a state that expired between them.
        let collaboration_id = action.collaboration_id.clone();
        let (expired, state) = self
            .inner
            .transition(move |coordinator| {
                let (_, expired) = expire_pending_approvals_locked(coordinator)?;
                let state = coordinator.human_status(&collaboration_id)?.state;
                Ok((expired, state))
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        // Other approvals this expiry ended are released here: the sweep
        // collects its candidates first and will never see them.
        settle_expired_approvals(
            &self.inner,
            expired
                .into_iter()
                .filter(|(collaboration_id, _)| *collaboration_id != action.collaboration_id)
                .collect(),
        )
        .await;
        if state != DomainState::AwaitingApproval {
            let status = self.publish_and_status(&action.collaboration_id).await;
            // A drive releases it if it just expired, and otherwise
            // finds it wherever it already is.
            schedule_drive(Arc::clone(&self.inner), action.collaboration_id.clone());
            return status;
        }
        let material = {
            self.inner
                .coordinator
                .lock()
                .await
                .human_preview_material(&action.collaboration_id)
                .map_err(CollaborationServiceError::domain)?
        };
        let preview = self.resolve_preview(&material).await;
        let collaboration_id = action.collaboration_id.clone();
        self.inner
            .transition(move |coordinator| match preview {
                Ok((source, packet)) => {
                    coordinator.approve_human(&domain_action, &source, &packet)?;
                    Ok(())
                }
                Err(error) => {
                    coordinator.fail_human_operation(
                        &collaboration_id,
                        error.code,
                        error.retryable,
                    )?;
                    Ok(())
                }
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        let status = self.publish_and_status(&action.collaboration_id).await;
        // Approved continues; a failed preview left it Failed, and the drive's
        // terminal arm releases it (it may be parked, so no source event would).
        schedule_drive(Arc::clone(&self.inner), action.collaboration_id.clone());
        status
    }

    async fn deny(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let collaboration_id = action.collaboration_id.clone();
        self.inner
            .transition(move |coordinator| {
                coordinator.deny_human(&ApprovalAction {
                    collaboration_id: action.collaboration_id,
                    approval_id: action.approval_id,
                    expected_revision: action.expected_revision,
                })
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        // Cleanup does not change the status, and still runs when building it fails.
        let status = self.publish_and_status(&collaboration_id).await;
        self.cleanup_terminal(&collaboration_id).await;
        status
    }

    async fn accept_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let source_key = {
            self.inner
                .coordinator
                .lock()
                .await
                .human_participants(&action.collaboration_id)
                .map_err(CollaborationServiceError::domain)?
                .source_session_key
        };
        let source = self
            .inner
            .port
            .resolve_source(
                &source_key,
                &self.inner.runtime_epoch,
                &self.inner.projection,
            )
            .await
            .map_err(port_service_error)?
            .ok_or_else(|| {
                port_service_error(CollaborationPortError::new(
                    CollaborationErrorCode::SourceNotLive,
                    CollaborationErrorCode::SourceNotLive.safe_next_action(),
                    true,
                ))
            })?;
        let collaboration_id = action.collaboration_id.clone();
        self.inner
            .transition(move |coordinator| {
                coordinator.accept_peer_human(
                    &PeerAction {
                        collaboration_id: action.collaboration_id,
                        result_id: action.result_id,
                        result_digest: action.result_digest,
                        expected_revision: action.expected_revision,
                    },
                    &source,
                )
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        let status = self.publish_and_status(&collaboration_id).await;
        schedule_drive(Arc::clone(&self.inner), collaboration_id.clone());
        status
    }

    async fn discard_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let collaboration_id = action.collaboration_id.clone();
        self.inner
            .transition(move |coordinator| {
                coordinator.discard_peer_human(&PeerAction {
                    collaboration_id: action.collaboration_id,
                    result_id: action.result_id,
                    result_digest: action.result_digest,
                    expected_revision: action.expected_revision,
                })
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        // Cleanup does not change the status, and still runs when building it fails.
        let status = self.publish_and_status(&collaboration_id).await;
        self.cleanup_terminal(&collaboration_id).await;
        status
    }

    async fn cancel(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError> {
        let cancelled = collaboration_id.clone();
        self.inner
            .transition(move |coordinator| coordinator.cancel_human(&cancelled))
            .await
            .map_err(CollaborationServiceError::domain)?;
        // Cleanup does not change the status, and still runs when building it fails.
        let status = self.publish_and_status(&collaboration_id).await;
        self.cleanup_terminal(&collaboration_id).await;
        status
    }

    fn set_draft_state(
        &self,
        update: CollaborationDraftStateUpdate,
    ) -> Result<(), CollaborationServiceError> {
        self.inner.projection.set_draft_state(&update)
    }

    async fn close(&self) -> Result<(), CollaborationServiceError> {
        self.inner.cancellation.cancel();
        let sweep = self
            .approval_sweep
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(sweep) = sweep
            && let Err(error) = sweep.await
        {
            tracing::warn!(%error, "collaboration approval expiry sweep did not stop cleanly");
        }
        {
            self.inner.coordinator.lock().await.quiesce();
        }
        loop {
            // Register for the wakeup before reading `active`, otherwise a
            // drive that finishes in between stores no permit and this await
            // never returns.
            let notified = self.inner.drive_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let empty = self
                .inner
                .drives
                .lock()
                .map(|drives| drives.active.is_empty())
                .unwrap_or(true);
            if empty {
                break;
            }
            notified.await;
        }
        self.inner
            .port
            .shutdown()
            .await
            .map_err(port_service_error)?;
        self.inner
            .transition(CollaborationCoordinator::stop)
            .await
            .map_err(CollaborationServiceError::domain)
    }
}

#[async_trait]
impl<P> CollaborationModelIngress for DurableCollaborationService<P>
where
    P: CollaborationRuntimePort,
{
    async fn submit(
        &self,
        intent: CollaborationMcpIntent,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        if cancellation.is_cancelled() {
            return Err(McpIngressError::new(
                CollaborationErrorCode::BrokerUnavailable,
            ));
        }
        let input = CollaborationIntentInput {
            version: Some(1),
            mode: intent.mode,
            source_session_key: intent.source_session_key.unwrap_or_default(),
            target_engine: intent.target_engine,
            objective: intent.objective,
            client_request_id: intent.client_request_id,
            origin: CollaborationOrigin::Model,
        };
        let model = authority.clone();
        let began = self
            .inner
            .transition(move |coordinator| coordinator.begin_model(input, &model))
            .await;
        // Even a refused admission may already have expired others.
        unpark_after_admission(&self.inner);
        let started = began.map_err(mcp_coordinator_error)?;
        if started.state == DomainState::Requested {
            let material = self
                .inner
                .coordinator
                .lock()
                .await
                .human_preview_material(&started.collaboration_id)
                .map_err(mcp_coordinator_error)?;
            let preview = self.resolve_preview(&material).await;
            complete_preview(&self.inner, &started.collaboration_id, preview)
                .await
                .map_err(mcp_coordinator_error)?;
        }
        self.publish(&started.collaboration_id).await;
        let receipt = self
            .inner
            .coordinator
            .lock()
            .await
            .model_status(&started.collaboration_id, &authority)
            .map_err(mcp_coordinator_error)?;
        if let Ok(participants) = self
            .inner
            .coordinator
            .lock()
            .await
            .human_participants(&started.collaboration_id)
        {
            sync_source_index(
                &self.inner,
                &participants.source_session_key,
                &started.collaboration_id,
                receipt.state.is_terminal(),
            );
        }
        Ok(receipt)
    }

    async fn status(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        if cancellation.is_cancelled() {
            return Err(McpIngressError::new(
                CollaborationErrorCode::BrokerUnavailable,
            ));
        }
        self.inner
            .coordinator
            .lock()
            .await
            .model_status(&request.collaboration_id, &authority)
            .map_err(mcp_coordinator_error)
    }

    async fn cancel(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        if cancellation.is_cancelled() {
            return Err(McpIngressError::new(
                CollaborationErrorCode::BrokerUnavailable,
            ));
        }
        let collaboration_id = request.collaboration_id.clone();
        let receipt = self
            .inner
            .transition(move |coordinator| coordinator.cancel_model(&collaboration_id, &authority))
            .await
            .map_err(mcp_coordinator_error)?;
        self.publish(&request.collaboration_id).await;
        self.cleanup_terminal(&request.collaboration_id).await;
        Ok(receipt)
    }
}

fn mcp_coordinator_error(error: CoordinatorError) -> McpIngressError {
    match error {
        CoordinatorError::Ingress {
            code,
            collaboration_id,
            retryable,
        } => McpIngressError {
            code,
            collaboration_id,
            retryable,
        },
        CoordinatorError::Contract(_) => McpIngressError::new(CollaborationErrorCode::StateCorrupt),
        CoordinatorError::Journal(_) | CoordinatorError::Audit(_) => {
            McpIngressError::new(CollaborationErrorCode::BrokerUnavailable)
        }
    }
}

#[async_trait]
impl CollaborationSessionHooks for DurableCollaborationService<SessionRuntimeCollaborationPort> {
    async fn guard_session_mutation(
        &self,
        session_key: &str,
        action: &str,
        allow_safe_stop: bool,
    ) -> Result<(), DispatchError> {
        let locks = self
            .inner
            .coordinator
            .lock()
            .await
            .locked_sessions()
            .map_err(CollaborationServiceError::domain)?;
        let target = locks
            .iter()
            .any(|lock| lock.session_key == session_key && lock.role == LockedSessionRole::Target);
        let source = locks
            .iter()
            .any(|lock| lock.session_key == session_key && lock.role == LockedSessionRole::Source);
        if !target && (!source || allow_safe_stop) {
            return Ok(());
        }
        Err(collaboration_session_locked(target, action))
    }

    async fn guard_bulk_mutation(&self, action: &str) -> Result<(), DispatchError> {
        let locks = self
            .inner
            .coordinator
            .lock()
            .await
            .locked_sessions()
            .map_err(CollaborationServiceError::domain)?;
        let Some(lock) = locks.first() else {
            return Ok(());
        };
        Err(collaboration_session_locked(
            lock.role == LockedSessionRole::Target,
            action,
        ))
    }

    fn user_send_started(&self, session_key: &str) -> Result<(), DispatchError> {
        DurableCollaborationService::user_send_started(self, session_key).map_err(Into::into)
    }

    fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), DispatchError> {
        DurableCollaborationService::user_send_settled(self, session_key, text, outcome)
            .map_err(Into::into)
    }

    fn observe_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), DispatchError> {
        DurableCollaborationService::observe_event(self, session_key, event).map_err(Into::into)
    }

    fn observe_registry_change(&self, session_key: &str) -> Result<(), DispatchError> {
        self.inner
            .port
            .observe_registry_change(session_key)
            .map_err(port_service_error)
            .map_err(Into::into)
    }
}

fn collaboration_session_locked(target: bool, action: &str) -> DispatchError {
    let lead = if target {
        "That peer target is host-controlled."
    } else {
        "That collaboration source is host-controlled."
    };
    DispatchError::new(
        "effect-definitely-unsent",
        format!(
            "{lead}\nInterface has not durably settled its collaboration yet, so ordinary {action} cannot mutate this session. Wait for terminal recovery or cleanup."
        ),
        false,
        true,
    )
}

fn port_service_error(error: CollaborationPortError) -> CollaborationServiceError {
    let CollaborationPortError {
        code,
        message,
        retryable,
    } = error;
    CollaborationServiceError {
        code: format!("COLLABORATION_{}", code.as_str()),
        message: code.safe_next_action().to_owned(),
        retryable,
        replay_safe: !code.is_ambiguous(),
    }
    .with_diagnostic(&message)
}

fn schedule_drive<P>(inner: Arc<CollaborationServiceInner<P>>, collaboration_id: String)
where
    P: CollaborationRuntimePort,
{
    if inner.cancellation.is_cancelled() {
        return;
    }
    let should_spawn = match inner.drives.lock() {
        Ok(mut drives) => {
            // An explicit schedule always un-parks, in the same critical
            // section that decides between marking dirty and spawning.
            drives.parked.remove(&collaboration_id);
            if drives.active.contains(&collaboration_id) {
                drives.dirty.insert(collaboration_id.clone());
                false
            } else {
                drives.active.insert(collaboration_id.clone());
                true
            }
        }
        Err(_) => false,
    };
    if !should_spawn {
        return;
    }
    tokio::spawn(async move {
        loop {
            let mut parked = false;
            if !inner.cancellation.is_cancelled() {
                match drive_collaboration(&inner, &collaboration_id).await {
                    Ok(end) => parked = end == DriveEnd::Parked,
                    Err(error) => {
                        tracing::error!(
                            collaboration_id,
                            code = error.code,
                            message = error.message,
                            "collaboration drive failed"
                        );
                        fail_unexpected(&inner, &collaboration_id).await;
                    }
                }
            }
            let rerun = match inner.drives.lock() {
                Ok(mut drives) => {
                    if drives.dirty.remove(&collaboration_id) && !inner.cancellation.is_cancelled()
                    {
                        true
                    } else {
                        drives.active.remove(&collaboration_id);
                        // Only here, where the drive is no longer active: a
                        // concurrent explicit schedule marked it dirty instead
                        // and forced the rerun above.
                        if parked {
                            drives.parked.insert(collaboration_id.clone());
                        }
                        false
                    }
                }
                Err(_) => false,
            };
            inner.drive_changed.notify_waiters();
            if !rerun {
                break;
            }
        }
    });
}

/// Admission expires due approvals of other collaborations without a publish
/// or cleanup. Un-park everything so the next source event drives each one
/// once, as before parking existed, and a terminal one reaches cleanup.
fn unpark_after_admission<P>(inner: &CollaborationServiceInner<P>) {
    if let Ok(mut drives) = inner.drives.lock() {
        drives.parked.clear();
    }
}

fn sync_source_index<P>(
    inner: &CollaborationServiceInner<P>,
    source_session_key: &str,
    collaboration_id: &str,
    terminal: bool,
) {
    let Ok(mut drives) = inner.drives.lock() else {
        return;
    };
    if terminal {
        drives.forget_source(source_session_key, collaboration_id);
    } else {
        drives.remember_source(source_session_key, collaboration_id);
    }
}

fn schedule_source_collaborations<P>(inner: &Arc<CollaborationServiceInner<P>>, session_key: &str)
where
    P: CollaborationRuntimePort,
{
    let identifiers = match inner.drives.lock() {
        Ok(drives) => drives.source_collaborations(session_key),
        Err(_) => return,
    };
    for identifier in identifiers {
        schedule_drive(Arc::clone(inner), identifier);
    }
}

async fn drive_collaboration<P>(
    inner: &Arc<CollaborationServiceInner<P>>,
    collaboration_id: &str,
) -> Result<DriveEnd, CollaborationServiceError>
where
    P: CollaborationRuntimePort,
{
    loop {
        if inner.cancellation.is_cancelled() {
            return Ok(DriveEnd::Done);
        }
        let receipt = inner
            .coordinator
            .lock()
            .await
            .human_status(collaboration_id)
            .map_err(CollaborationServiceError::domain)?;
        match receipt.state {
            DomainState::Approved | DomainState::Preflight => {
                let material = inner
                    .coordinator
                    .lock()
                    .await
                    .human_preview_material(collaboration_id)
                    .map_err(CollaborationServiceError::domain)?;
                let preview = resolve_preview(inner, &material).await;
                let id = collaboration_id.to_owned();
                inner
                    .transition(move |coordinator| match preview {
                        Ok((source, packet)) => {
                            coordinator.reserve_prepare(&id, &source, &packet)?;
                            Ok(())
                        }
                        Err(error) => {
                            coordinator.fail_human_operation(&id, error.code, error.retryable)?;
                            Ok(())
                        }
                    })
                    .await
                    .map_err(CollaborationServiceError::domain)?;
                publish_status(inner, collaboration_id).await;
            }
            DomainState::PreparingTarget
            | DomainState::RunningTarget
            | DomainState::DeliveryPending
            | DomainState::ParkingSource
            | DomainState::OwnershipTransfer
            | DomainState::ActivatingTarget => {
                if let Some(end) =
                    run_maintenance_effect(inner, collaboration_id, receipt.state).await?
                {
                    return Ok(end);
                }
            }
            DomainState::ResultReady => {
                let id = collaboration_id.to_owned();
                inner
                    .transition(move |coordinator| coordinator.hold_peer_result(&id))
                    .await
                    .map_err(CollaborationServiceError::domain)?;
                publish_status(inner, collaboration_id).await;
                if let Some(peer) = pending_peer(inner, collaboration_id).await? {
                    inner
                        .port
                        .hold_peer(&peer, &inner.projection)
                        .await
                        .map_err(port_service_error)?;
                }
            }
            DomainState::AwaitingPeerReview
            | DomainState::AwaitingApproval
            | DomainState::Unknown => return Ok(DriveEnd::Parked),
            state if state.is_terminal() => {
                cleanup_terminal(inner, collaboration_id).await;
                return Ok(DriveEnd::Done);
            }
            DomainState::Requested => {
                let material = inner
                    .coordinator
                    .lock()
                    .await
                    .human_preview_material(collaboration_id)
                    .map_err(CollaborationServiceError::domain)?;
                let preview = resolve_preview(inner, &material).await;
                complete_preview(inner, collaboration_id, preview)
                    .await
                    .map_err(CollaborationServiceError::domain)?;
                publish_status(inner, collaboration_id).await;
            }
            _ => return Ok(DriveEnd::Done),
        }
    }
}

/// Runs the maintenance effect the record's state reserves (prepare, run,
/// deliver, park, transfer or activate): resolves its source and packet,
/// starts the reserved effect, executes it on the port and settles the
/// outcome. `Some` ends the drive; `None` returns to the loop, which reads
/// the settled state.
async fn run_maintenance_effect<P>(
    inner: &CollaborationServiceInner<P>,
    collaboration_id: &str,
    state: DomainState,
) -> Result<Option<DriveEnd>, CollaborationServiceError>
where
    P: CollaborationRuntimePort,
{
    let record = inner
        .coordinator
        .lock()
        .await
        .runtime_record(collaboration_id)
        .map_err(CollaborationServiceError::domain)?;
    let (needs_source, needs_packet) =
        maintenance_effect_for_state(record.state, record.intent.mode)
            .map_or((false, false), effect_inputs_required);
    let source = if needs_source {
        inner
            .port
            .resolve_source(
                &record.source.session_key,
                &inner.runtime_epoch,
                &inner.projection,
            )
            .await
            .map_err(port_service_error)?
    } else {
        None
    };
    if needs_source && source.is_none() {
        // Waits for its source: a later source event re-drives it,
        // so it is deliberately not parked.
        if matches!(state, DomainState::DeliveryPending) {
            return Ok(Some(DriveEnd::Done));
        }
        let id = collaboration_id.to_owned();
        inner
            .transition(move |coordinator| {
                coordinator.fail_human_operation(&id, CollaborationErrorCode::SourceNotLive, true)
            })
            .await
            .map_err(CollaborationServiceError::domain)?;
        publish_status(inner, collaboration_id).await;
        return Ok(None);
    }
    let packet = if needs_packet {
        let material = inner
            .coordinator
            .lock()
            .await
            .human_preview_material(collaboration_id)
            .map_err(CollaborationServiceError::domain)?;
        Some(
            inner
                .port
                .build_packet(
                    &material,
                    source.as_ref().expect("source required with packet"),
                    &inner.projection,
                )
                .await
                .map_err(port_service_error)?,
        )
    } else {
        None
    };
    let id = collaboration_id.to_owned();
    let command = inner
        .transition(move |coordinator| {
            coordinator.start_reserved_effect(&id, source.as_ref(), packet.as_ref())
        })
        .await
        .map_err(CollaborationServiceError::domain)?;
    let Some(command) = command else {
        return Ok(Some(DriveEnd::Done));
    };
    publish_status(inner, collaboration_id).await;
    let effect_id = command.effect_id().to_owned();
    let outcome = inner
        .port
        .execute(command, inner.cancellation.child_token())
        .await;
    let id = collaboration_id.to_owned();
    let settled = inner
        .transition(move |coordinator| coordinator.settle_effect(&id, &effect_id, outcome))
        .await
        .map_err(CollaborationServiceError::domain)?;
    publish_status(inner, collaboration_id).await;
    if settled.state == DomainState::ResultReady {
        let record = inner
            .coordinator
            .lock()
            .await
            .runtime_record(collaboration_id)
            .map_err(CollaborationServiceError::domain)?;
        if let Err(error) = inner
            .port
            .release_target(&record, inner.cancellation.child_token())
            .await
        {
            tracing::warn!(%error, collaboration_id, "consult target release failed");
        }
    }
    Ok(None)
}

async fn resolve_preview<P>(
    inner: &CollaborationServiceInner<P>,
    material: &HumanPreviewMaterial,
) -> Result<(SourceSnapshot, PacketBuildResult), CollaborationPortError>
where
    P: CollaborationRuntimePort,
{
    let source = inner
        .port
        .resolve_source(
            &material.source_session_key,
            &inner.runtime_epoch,
            &inner.projection,
        )
        .await?
        .ok_or_else(|| {
            CollaborationPortError::new(
                CollaborationErrorCode::SourceNotFound,
                "source session is not live",
                true,
            )
        })?;
    if source.live_status == CollaborationLiveStatus::Failed {
        return Err(CollaborationPortError::new(
            CollaborationErrorCode::SourceNotLive,
            "source session is failed",
            true,
        ));
    }
    inner
        .port
        .check_target(&source, material.target_engine, material.mode)
        .await?;
    let packet = inner
        .port
        .build_packet(material, &source, &inner.projection)
        .await?;
    Ok((source, packet))
}

/// Records a resolved preview, or the reason it could not be built, in one
/// durable transition.
async fn complete_preview<P>(
    inner: &CollaborationServiceInner<P>,
    collaboration_id: &str,
    preview: Result<(SourceSnapshot, PacketBuildResult), CollaborationPortError>,
) -> Result<(), CoordinatorError> {
    let id = collaboration_id.to_owned();
    inner
        .transition(move |coordinator| match preview {
            Ok((source, packet)) => {
                coordinator.complete_human_preview(&id, &source, &packet)?;
                Ok(())
            }
            Err(error) => {
                coordinator.fail_human_preview(&id, error.code, error.retryable)?;
                Ok(())
            }
        })
        .await
}

async fn pending_peer<P>(
    inner: &CollaborationServiceInner<P>,
    collaboration_id: &str,
) -> Result<Option<PendingPeer>, CollaborationServiceError> {
    inner
        .coordinator
        .lock()
        .await
        .pending_peer(collaboration_id)
        .map_err(CollaborationServiceError::domain)
}

async fn status_for_inner<P>(
    inner: &CollaborationServiceInner<P>,
    collaboration_id: &str,
) -> Result<CollaborationStatusDto, CollaborationServiceError> {
    let mut coordinator = inner.coordinator.lock().await;
    let receipt = coordinator
        .human_status(collaboration_id)
        .map_err(CollaborationServiceError::domain)?;
    let participants = coordinator
        .human_participants(collaboration_id)
        .map_err(CollaborationServiceError::domain)?;
    let approval = coordinator
        .approval_preview(collaboration_id)
        .map_err(CollaborationServiceError::domain)?;
    let peer = coordinator
        .pending_peer(collaboration_id)
        .map_err(CollaborationServiceError::domain)?;
    Ok(status_dto(receipt, participants, approval, peer))
}

async fn publish_status<P>(inner: &CollaborationServiceInner<P>, collaboration_id: &str) {
    if inner.pushes.is_none() {
        return;
    }
    let Ok(status) = status_for_inner(inner, collaboration_id).await else {
        return;
    };
    publish_status_dto(inner, status).await;
}

async fn publish_status_dto<P>(
    inner: &CollaborationServiceInner<P>,
    status: CollaborationStatusDto,
) {
    let Some(pushes) = &inner.pushes else {
        return;
    };
    let event = CollaborationEventDto {
        version: 1,
        kind: "changed".to_owned(),
        status,
    };
    let Ok(payload) = serde_json::to_value(event) else {
        return;
    };
    let Ok(frame) = PushFrame::new(CHANNELS.collaboration_event, payload) else {
        return;
    };
    let _ = pushes.send(frame).await;
}

fn spawn_approval_sweep<P>(inner: &Arc<CollaborationServiceInner<P>>) -> tokio::task::JoinHandle<()>
where
    P: CollaborationRuntimePort,
{
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = inner.cancellation.cancelled() => break,
                () = tokio::time::sleep(APPROVAL_SWEEP_INTERVAL) => {
                    // Usually nothing awaits approval: skip the transition's
                    // blocking-pool hop and its journal scan.
                    if !inner.coordinator.lock().await.has_awaiting_approval() {
                        continue;
                    }
                    if let Err(error) = expire_pending_approvals_and_publish(&inner).await {
                        tracing::warn!(
                            code = %error.code,
                            "collaboration approval expiry sweep failed"
                        );
                    }
                }
            }
        }
    })
}

pub(super) async fn expire_pending_approvals_and_publish<P>(
    inner: &Arc<CollaborationServiceInner<P>>,
) -> Result<u64, CollaborationServiceError>
where
    P: CollaborationRuntimePort,
{
    let (expired, changed) = inner
        .transition(expire_pending_approvals_locked)
        .await
        .map_err(CollaborationServiceError::domain)?;
    settle_expired_approvals(inner, changed).await;
    Ok(expired)
}

/// Expires every due approval inside one coordinator transition. Returns the
/// count and each approval candidate whose receipt changed, with whether it is
/// now terminal.
fn expire_pending_approvals_locked(
    coordinator: &mut CollaborationCoordinator,
) -> Result<(u64, Vec<(String, bool)>), CoordinatorError> {
    let candidates = coordinator
        .active_receipts()?
        .into_iter()
        .filter(|receipt| receipt.state == DomainState::AwaitingApproval)
        .map(|receipt| (receipt.collaboration_id, receipt.revision))
        .collect::<Vec<_>>();
    let expired = coordinator.expire_pending_approvals()?;
    let mut changed = Vec::new();
    for (collaboration_id, revision) in candidates {
        let receipt = coordinator.human_status(&collaboration_id)?;
        if receipt.revision != revision {
            changed.push((collaboration_id, receipt.state.is_terminal()));
        }
    }
    Ok((expired, changed))
}

async fn settle_expired_approvals<P>(
    inner: &CollaborationServiceInner<P>,
    changed: Vec<(String, bool)>,
) where
    P: CollaborationRuntimePort,
{
    for (collaboration_id, is_terminal) in changed {
        publish_status(inner, &collaboration_id).await;
        // Expiry drives the record straight to a terminal state without going
        // through the normal drive loop, so the source-index row (and any
        // parking) and the target session have to be released here or they
        // stay pinned forever.
        if is_terminal {
            cleanup_terminal(inner, &collaboration_id).await;
        }
    }
}

async fn cleanup_terminal<P>(inner: &CollaborationServiceInner<P>, collaboration_id: &str)
where
    P: CollaborationRuntimePort,
{
    let (record, peer) = {
        let coordinator = inner.coordinator.lock().await;
        let Ok(record) = coordinator.runtime_record(collaboration_id) else {
            return;
        };
        let peer = coordinator.pending_peer(collaboration_id).ok().flatten();
        (record, peer)
    };
    sync_source_index(
        inner,
        &record.source.session_key,
        collaboration_id,
        record.state.is_terminal(),
    );
    if let Some(peer) = peer {
        let _ = inner.port.remove_held_peer(&peer, &inner.projection).await;
    } else if let Some(result) = record.peer_result.as_ref()
        && let Some(queue_entry_id) = result.queue_entry_id.as_deref()
    {
        let _ = inner.projection.remove_peer(
            &record.source.session_key,
            collaboration_id,
            queue_entry_id,
            &result.result_id,
        );
    }
    if matches!(record.state, DomainState::Completed) && record.intent.mode == DomainMode::Handoff {
        let _ = inner
            .port
            .release_target(&record, inner.cancellation.child_token())
            .await;
    } else if matches!(
        record.state,
        DomainState::Failed | DomainState::Cancelled | DomainState::Denied | DomainState::Completed
    ) {
        let _ = inner
            .port
            .dispose_target(&record, inner.cancellation.child_token())
            .await;
    }
}

async fn fail_unexpected<P>(inner: &CollaborationServiceInner<P>, collaboration_id: &str)
where
    P: CollaborationRuntimePort,
{
    let id = collaboration_id.to_owned();
    let committed = inner
        .transition(move |coordinator| {
            coordinator.fail_human_operation(&id, CollaborationErrorCode::BrokerUnavailable, true)
        })
        .await;
    if committed.is_ok() {
        publish_status(inner, collaboration_id).await;
        cleanup_terminal(inner, collaboration_id).await;
    }
}

/// Settles every record recovery left in `Unknown` as a proven terminal
/// failure. An ambiguous effect has no other exit, and while the record lives
/// its source session stays pinned by `locked_sessions`, so a single ambiguous
/// outcome would refuse that session new work for the rest of its life. The
/// effect is never dispatched again: this only records the marker and the
/// failure, so an effect that silently landed still reads as failed.
fn reconcile_unknown_records(
    coordinator: &mut CollaborationCoordinator,
) -> Result<(), CollaborationServiceError> {
    let unknown = coordinator
        .all_receipts()
        .map_err(CollaborationServiceError::domain)?
        .into_iter()
        .filter(|receipt| receipt.state == DomainState::Unknown)
        .map(|receipt| receipt.collaboration_id)
        .collect::<Vec<_>>();
    for collaboration_id in unknown {
        // A marker recorded during target staging is immutable, so reuse it
        // rather than replacing it with the recovery marker.
        let marker = coordinator
            .runtime_record(&collaboration_id)
            .ok()
            .and_then(|record| record.target.reconciliation_marker)
            .unwrap_or_else(|| UNKNOWN_RECONCILIATION_MARKER.to_owned());
        if let Err(error) = coordinator.reconcile_unknown_failed(
            &collaboration_id,
            &marker,
            CollaborationErrorCode::SourceDeliveryUnknown,
        ) {
            // A refused reconciliation leaves the record untouched and its
            // source locked, which is the safe direction; the rest of the
            // journal still recovers.
            tracing::warn!(
                %error,
                collaboration_id,
                "unknown collaboration record could not be reconciled at startup"
            );
        }
    }
    Ok(())
}

fn should_drive(state: DomainState) -> bool {
    matches!(
        state,
        DomainState::Requested
            | DomainState::Approved
            | DomainState::Preflight
            | DomainState::PreparingTarget
            | DomainState::RunningTarget
            | DomainState::ResultReady
            | DomainState::DeliveryPending
            | DomainState::ParkingSource
            | DomainState::OwnershipTransfer
            | DomainState::ActivatingTarget
    )
}
