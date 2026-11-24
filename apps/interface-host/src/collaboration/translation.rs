//! Frozen payload validation and DTO translation; effect coordination stays in the parent.
use super::{
    ApprovalPreview, CollaborationApprovalAction, CollaborationApprovalDto,
    CollaborationDraftStateUpdate, CollaborationEngine, CollaborationEngineId,
    CollaborationErrorDto, CollaborationMode, CollaborationPeerAction, CollaborationPeerDto,
    CollaborationPeerState, CollaborationReceiptDto, CollaborationReceiptV1,
    CollaborationServiceError, CollaborationState, CollaborationStatusDto,
    CollaborationSubmitRequest, DeserializeOwned, DomainMode, DomainState, MAX_IDENTIFIER_BYTES,
    PendingPeer, PendingPeerState, Serialize, SessionParticipants, Value, parse_key,
};

pub(super) fn decode_empty(payload: &Value) -> Result<(), CollaborationServiceError> {
    crate::payload::require_empty(payload, CollaborationServiceError::invalid_request)
}

pub(super) fn decode<T: DeserializeOwned>(payload: Value) -> Result<T, CollaborationServiceError> {
    crate::payload::decode_object(payload, |_| CollaborationServiceError::invalid_request())
}

pub(super) fn decode_submit(
    payload: Value,
) -> Result<CollaborationSubmitRequest, CollaborationServiceError> {
    let input = decode::<CollaborationSubmitRequest>(payload)?;
    validate_session_key(&input.source_session_key)?;
    validate_identifier(&input.client_request_id)?;
    if input.objective.trim().is_empty() {
        return Err(CollaborationServiceError::invalid_request());
    }
    Ok(input)
}

pub(super) fn decode_approval(
    payload: Value,
) -> Result<CollaborationApprovalAction, CollaborationServiceError> {
    let action = decode::<CollaborationApprovalAction>(payload)?;
    validate_identifier(&action.collaboration_id)?;
    validate_identifier(&action.approval_id)?;
    validate_revision(action.expected_revision)?;
    Ok(action)
}

pub(super) fn decode_peer(
    payload: Value,
) -> Result<CollaborationPeerAction, CollaborationServiceError> {
    let action = decode::<CollaborationPeerAction>(payload)?;
    validate_identifier(&action.collaboration_id)?;
    validate_identifier(&action.result_id)?;
    validate_identifier(&action.result_digest)?;
    validate_revision(action.expected_revision)?;
    Ok(action)
}

pub(super) fn decode_draft(
    payload: Value,
) -> Result<CollaborationDraftStateUpdate, CollaborationServiceError> {
    let update = decode::<CollaborationDraftStateUpdate>(payload)?;
    validate_session_key(&update.source_session_key)?;
    validate_revision(update.revision)?;
    Ok(update)
}

pub(super) fn validate_revision(revision: u64) -> Result<(), CollaborationServiceError> {
    if revision <= interface_collaboration::state::JS_MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(CollaborationServiceError::invalid_request())
    }
}

pub(super) fn validate_identifier(value: &str) -> Result<(), CollaborationServiceError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(|character| {
            let code = u32::from(character);
            code < 0x20 || (0x7f..=0x9f).contains(&code)
        })
    {
        Err(CollaborationServiceError::invalid_request())
    } else {
        Ok(())
    }
}

pub(super) fn validate_session_key(value: &str) -> Result<(), CollaborationServiceError> {
    validate_identifier(value)?;
    let (engine, _) = parse_key(value).map_err(|_| CollaborationServiceError::invalid_request())?;
    if CollaborationEngineId::from_wire(engine.as_str()).is_some() {
        Ok(())
    } else {
        Err(CollaborationServiceError::invalid_request())
    }
}

pub(super) fn encode(value: impl Serialize) -> Result<Value, CollaborationServiceError> {
    serde_json::to_value(value).map_err(|_| CollaborationServiceError::encoding())
}

pub(super) fn status_dto(
    receipt: CollaborationReceiptV1,
    participants: SessionParticipants,
    approval: Option<ApprovalPreview>,
    peer: Option<PendingPeer>,
) -> CollaborationStatusDto {
    CollaborationStatusDto {
        receipt: receipt_dto(receipt),
        source_session_key: participants.source_session_key,
        target_session_key: participants.target_session_key,
        approval: approval.map(approval_dto),
        peer: peer.map(peer_dto),
    }
}

pub(super) fn receipt_dto(value: CollaborationReceiptV1) -> CollaborationReceiptDto {
    CollaborationReceiptDto {
        version: 1,
        collaboration_id: value.collaboration_id,
        revision: value.revision,
        state: state_dto(value.state),
        mode: mode_dto(value.mode),
        source_engine: engine_dto(value.source_engine),
        target_engine: engine_dto(value.target_engine),
        approval_required: value.approval_required,
        cancellation_safe: value.cancellation_safe,
        created_at: value.created_at,
        updated_at: value.updated_at,
        error: value.error.map(|error| CollaborationErrorDto {
            code: error.code.as_str().to_owned(),
            safe_next_action: error.safe_next_action,
            retryable: error.retryable,
        }),
    }
}

pub(super) fn approval_dto(value: ApprovalPreview) -> CollaborationApprovalDto {
    CollaborationApprovalDto {
        collaboration_id: value.collaboration_id,
        approval_id: value.approval_id,
        expected_revision: value.expected_revision,
        expires_at: value.expires_at,
        mode: mode_dto(value.mode),
        source_engine: engine_dto(value.source_engine),
        source_session_key: value.source_session_key,
        source_title: value.source_title,
        canonical_cwd: value.canonical_cwd,
        canonical_project_root: value.canonical_project_root,
        target_engine: engine_dto(value.target_engine),
        objective: value.objective,
        packet_hash: value.packet_hash,
        packet_preview: value.packet_preview,
        packet_bytes: value.packet_bytes,
        context_bytes: value.context_bytes,
        omissions: value.omissions,
        managed_worktree: value.managed_worktree,
    }
}

pub(super) fn peer_dto(value: PendingPeer) -> CollaborationPeerDto {
    CollaborationPeerDto {
        state: match value.state {
            PendingPeerState::AwaitingReview => CollaborationPeerState::AwaitingReview,
            PendingPeerState::DeliveryPending => CollaborationPeerState::DeliveryPending,
        },
        collaboration_id: value.collaboration_id,
        expected_revision: value.expected_revision,
        result_id: value.result_id,
        result_digest: value.result_digest,
        result_bytes: value.result_bytes,
        source_session_key: value.source_session_key,
        source_engine: engine_dto(value.source_engine),
        target_session_key: value.target_session_key,
        target_engine: engine_dto(value.target_engine),
        objective: value.objective,
        body: value.body,
    }
}

pub(super) const fn engine_dto(value: CollaborationEngineId) -> CollaborationEngine {
    match value {
        CollaborationEngineId::Claude => CollaborationEngine::Claude,
        CollaborationEngineId::Codex => CollaborationEngine::Codex,
        CollaborationEngineId::AcpGrok => CollaborationEngine::AcpGrok,
    }
}

pub(super) const fn engine_domain(value: CollaborationEngine) -> CollaborationEngineId {
    match value {
        CollaborationEngine::Claude => CollaborationEngineId::Claude,
        CollaborationEngine::Codex => CollaborationEngineId::Codex,
        CollaborationEngine::AcpGrok => CollaborationEngineId::AcpGrok,
    }
}

pub(super) const fn mode_dto(value: DomainMode) -> CollaborationMode {
    match value {
        DomainMode::Consult => CollaborationMode::Consult,
        DomainMode::Handoff => CollaborationMode::Handoff,
    }
}

pub(super) const fn mode_domain(value: CollaborationMode) -> DomainMode {
    match value {
        CollaborationMode::Consult => DomainMode::Consult,
        CollaborationMode::Handoff => DomainMode::Handoff,
    }
}

pub(super) const fn state_dto(value: DomainState) -> CollaborationState {
    match value {
        DomainState::Requested => CollaborationState::Requested,
        DomainState::AwaitingApproval => CollaborationState::AwaitingApproval,
        DomainState::Approved => CollaborationState::Approved,
        DomainState::Preflight => CollaborationState::Preflight,
        DomainState::PreparingTarget => CollaborationState::PreparingTarget,
        DomainState::RunningTarget => CollaborationState::RunningTarget,
        DomainState::ResultReady => CollaborationState::ResultReady,
        DomainState::AwaitingPeerReview => CollaborationState::AwaitingPeerReview,
        DomainState::DeliveryPending => CollaborationState::DeliveryPending,
        DomainState::ParkingSource => CollaborationState::ParkingSource,
        DomainState::OwnershipTransfer => CollaborationState::OwnershipTransfer,
        DomainState::ActivatingTarget => CollaborationState::ActivatingTarget,
        DomainState::Completed => CollaborationState::Completed,
        DomainState::Denied => CollaborationState::Denied,
        DomainState::Cancelled => CollaborationState::Cancelled,
        DomainState::Failed => CollaborationState::Failed,
        DomainState::Unknown => CollaborationState::Unknown,
    }
}
