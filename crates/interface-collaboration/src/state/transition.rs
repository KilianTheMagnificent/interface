//! The legal-transition, maintenance-effect and effect-dependency tables, the
//! transition-delta checks and `transition_collaboration_record`.

use uuid::Uuid;

use super::{
    CollaborationApprovalDecision, CollaborationApprovalRecordV1, CollaborationEffectKind,
    CollaborationEffectMapV1, CollaborationEffectRecordV1, CollaborationEffectStatus,
    CollaborationMode, CollaborationPacketRecordV1, CollaborationPeerDecision,
    CollaborationPeerResultState, CollaborationRecordV1, CollaborationState,
    CollaborationTransitionRequest, CollaborationTransitionResult, CollaborationWorktreeOwnerV1,
    CollaborationWorktreeV1, ContractError, ContractErrorReason, ContractResult,
    audit_event_for_validated_transition, bounded_identifier, safe_integer, timestamp_millis,
};

#[must_use]
pub fn legal_collaboration_transitions(
    state: CollaborationState,
    mode: CollaborationMode,
) -> &'static [CollaborationState] {
    use CollaborationState as S;
    match (state, mode) {
        (S::Requested, _) => &[S::AwaitingApproval, S::Cancelled, S::Failed],
        (S::AwaitingApproval, _) => &[S::Approved, S::Denied, S::Cancelled, S::Failed],
        (S::Approved, _) => &[S::AwaitingApproval, S::Preflight, S::Cancelled, S::Failed],
        (S::Preflight, _) => &[
            S::AwaitingApproval,
            S::PreparingTarget,
            S::Cancelled,
            S::Failed,
        ],
        (S::PreparingTarget, CollaborationMode::Consult) => &[
            S::RunningTarget,
            S::AwaitingApproval,
            S::Cancelled,
            S::Failed,
            S::Unknown,
        ],
        (S::PreparingTarget, CollaborationMode::Handoff) => &[
            S::ParkingSource,
            S::AwaitingApproval,
            S::Cancelled,
            S::Failed,
            S::Unknown,
        ],
        (S::RunningTarget, _) => &[S::ResultReady, S::Cancelled, S::Failed, S::Unknown],
        (S::ResultReady, _) => &[S::AwaitingPeerReview, S::Cancelled, S::Failed],
        (S::AwaitingPeerReview, _) => &[S::DeliveryPending, S::Completed, S::Cancelled, S::Failed],
        (S::DeliveryPending, _) => &[S::Completed, S::Cancelled, S::Failed, S::Unknown],
        (S::ParkingSource, CollaborationMode::Handoff) => {
            &[S::OwnershipTransfer, S::Cancelled, S::Failed, S::Unknown]
        }
        (S::ParkingSource, CollaborationMode::Consult) => &[S::Cancelled, S::Failed, S::Unknown],
        (S::OwnershipTransfer, CollaborationMode::Handoff) => {
            &[S::ActivatingTarget, S::Failed, S::Unknown]
        }
        (S::OwnershipTransfer, CollaborationMode::Consult) => &[S::Failed, S::Unknown],
        (S::ActivatingTarget, _) => &[S::Completed, S::Failed, S::Unknown],
        (S::Unknown, _) => &[S::Completed, S::Failed],
        (S::Completed | S::Denied | S::Cancelled | S::Failed, _) => &[],
    }
}

#[must_use]
pub const fn maintenance_effect_for_state(
    state: CollaborationState,
    mode: CollaborationMode,
) -> Option<CollaborationEffectKind> {
    match (state, mode) {
        (CollaborationState::PreparingTarget, _) => Some(CollaborationEffectKind::PrepareTarget),
        (CollaborationState::RunningTarget, CollaborationMode::Consult) => {
            Some(CollaborationEffectKind::RunTarget)
        }
        (CollaborationState::DeliveryPending, CollaborationMode::Consult) => {
            Some(CollaborationEffectKind::DeliverResult)
        }
        (CollaborationState::ParkingSource, CollaborationMode::Handoff) => {
            Some(CollaborationEffectKind::ParkSource)
        }
        (CollaborationState::OwnershipTransfer, CollaborationMode::Handoff) => {
            Some(CollaborationEffectKind::TransferOwnership)
        }
        (CollaborationState::ActivatingTarget, CollaborationMode::Handoff) => {
            Some(CollaborationEffectKind::ActivateTarget)
        }
        _ => None,
    }
}

/// The state a successfully settled effect moves its record to, and the
/// effect that state reserves next: the forward edge of each ladder, read
/// back by [`maintenance_effect_for_state`].
#[must_use]
pub(crate) const fn effect_successor(
    kind: CollaborationEffectKind,
    mode: CollaborationMode,
) -> (CollaborationState, Option<CollaborationEffectKind>) {
    use CollaborationEffectKind as K;
    use CollaborationState as S;
    match (kind, mode) {
        (K::PrepareTarget, CollaborationMode::Consult) => (S::RunningTarget, Some(K::RunTarget)),
        (K::PrepareTarget, CollaborationMode::Handoff) => (S::ParkingSource, Some(K::ParkSource)),
        (K::RunTarget, _) => (S::ResultReady, None),
        (K::ParkSource, _) => (S::OwnershipTransfer, Some(K::TransferOwnership)),
        (K::TransferOwnership, _) => (S::ActivatingTarget, Some(K::ActivateTarget)),
        (K::ActivateTarget | K::DeliverResult, _) => (S::Completed, None),
    }
}

#[must_use]
pub const fn effect_inputs_required(kind: CollaborationEffectKind) -> (bool, bool) {
    match kind {
        CollaborationEffectKind::PrepareTarget | CollaborationEffectKind::ParkSource => {
            (true, true)
        }
        CollaborationEffectKind::DeliverResult => (true, false),
        CollaborationEffectKind::RunTarget
        | CollaborationEffectKind::TransferOwnership
        | CollaborationEffectKind::ActivateTarget => (false, false),
    }
}

pub(super) const fn allowed_effect(mode: CollaborationMode, kind: CollaborationEffectKind) -> bool {
    matches!(
        (mode, kind),
        (
            CollaborationMode::Consult,
            CollaborationEffectKind::PrepareTarget
                | CollaborationEffectKind::RunTarget
                | CollaborationEffectKind::DeliverResult
        ) | (
            CollaborationMode::Handoff,
            CollaborationEffectKind::PrepareTarget
                | CollaborationEffectKind::ParkSource
                | CollaborationEffectKind::TransferOwnership
                | CollaborationEffectKind::ActivateTarget
        )
    )
}

#[must_use]
pub fn cancellation_is_safe(record: &CollaborationRecordV1) -> bool {
    use CollaborationState as S;
    match record.state {
        S::Requested | S::AwaitingApproval | S::Approved | S::Preflight => true,
        S::PreparingTarget => record.effects.has_status(
            CollaborationEffectKind::PrepareTarget,
            CollaborationEffectStatus::Reserved,
        ),
        S::RunningTarget | S::ResultReady | S::AwaitingPeerReview => {
            record.intent.mode == CollaborationMode::Consult
        }
        S::DeliveryPending => record.effects.has_status(
            CollaborationEffectKind::DeliverResult,
            CollaborationEffectStatus::Reserved,
        ),
        S::ParkingSource => record.effects.has_status(
            CollaborationEffectKind::ParkSource,
            CollaborationEffectStatus::Reserved,
        ),
        _ => false,
    }
}

fn approval_refresh_is_safe(record: &CollaborationRecordV1) -> bool {
    matches!(
        record.state,
        CollaborationState::AwaitingApproval
            | CollaborationState::Approved
            | CollaborationState::Preflight
    ) || (record.state == CollaborationState::PreparingTarget
        && record.effects.has_status(
            CollaborationEffectKind::PrepareTarget,
            CollaborationEffectStatus::Reserved,
        ))
}

pub(super) fn require_only_effects(
    effects: &CollaborationEffectMapV1,
    expected: &[CollaborationEffectKind],
) -> ContractResult<()> {
    debug_assert!(
        expected
            .iter()
            .enumerate()
            .all(|(index, kind)| !expected[..index].contains(kind)),
        "count plus containment is set equality only for a duplicate-free expectation"
    );
    invalid_if!(
        effects.iter().count() != expected.len()
            || !effects.iter().all(|effect| expected.contains(&effect.kind)),
        "record.effects",
        "does not match the state boundary"
    );
    Ok(())
}

pub(super) fn require_effect_status(
    effects: &CollaborationEffectMapV1,
    kind: CollaborationEffectKind,
    statuses: &[CollaborationEffectStatus],
) -> ContractResult<()> {
    invalid_if!(
        !effects
            .get(kind)
            .is_some_and(|effect| statuses.contains(&effect.status)),
        format!("record.effects.{}", kind.as_str()),
        "does not have the required status for this state"
    );
    Ok(())
}

pub(super) fn validate_effect_dependencies(record: &CollaborationRecordV1) -> ContractResult<()> {
    use CollaborationEffectKind as K;
    let effects = &record.effects;
    let succeeded = |kind: K| effects.has_status(kind, CollaborationEffectStatus::Succeeded);
    invalid_if!(
        effects.run_target.is_some() && !succeeded(K::PrepareTarget),
        "record.effects.run-target",
        "requires successful target preparation"
    );
    invalid_if!(
        effects.deliver_result.is_some()
            && (!succeeded(K::RunTarget)
                || record
                    .peer_result
                    .as_ref()
                    .and_then(|result| result.acceptance.as_ref())
                    .map(|acceptance| acceptance.decision)
                    != Some(CollaborationPeerDecision::Accepted)),
        "record.effects.deliver-result",
        "requires a settled result and exact human acceptance"
    );
    invalid_if!(
        effects.park_source.is_some() && !succeeded(K::PrepareTarget),
        "record.effects.park-source",
        "requires successful target preparation"
    );
    invalid_if!(
        effects.transfer_ownership.is_some() && !succeeded(K::ParkSource),
        "record.effects.transfer-ownership",
        "requires successful source parking"
    );
    invalid_if!(
        effects.activate_target.is_some() && !succeeded(K::TransferOwnership),
        "record.effects.activate-target",
        "requires committed ownership transfer"
    );
    Ok(())
}

fn validate_effect_delta(
    previous: &CollaborationEffectMapV1,
    next: &CollaborationEffectMapV1,
    maintenance: Option<CollaborationEffectKind>,
    updated_at: &str,
) -> ContractResult<()> {
    use CollaborationEffectStatus as S;
    for kind in [
        CollaborationEffectKind::PrepareTarget,
        CollaborationEffectKind::RunTarget,
        CollaborationEffectKind::ParkSource,
        CollaborationEffectKind::TransferOwnership,
        CollaborationEffectKind::ActivateTarget,
        CollaborationEffectKind::DeliverResult,
    ] {
        match (previous.get(kind), next.get(kind)) {
            (None, None) => {}
            (None, Some(after)) => {
                invalid_if!(
                    after.created_at != updated_at
                        || after.status != S::Reserved
                        || after.attempts != 0,
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "new effects must be durably reserved before an attempt starts"
                );
            }
            (Some(_), None) => {
                return Err(ContractError::invalid(
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "cannot be removed",
                ));
            }
            (Some(before), Some(after)) => {
                invalid_if!(
                    before.version != after.version
                        || before.effect_id != after.effect_id
                        || before.kind != after.kind
                        || before.created_at != after.created_at
                        || before.deadline_at != after.deadline_at,
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "effect identity is immutable once reserved"
                );
                let status_ok = matches!(
                    (before.status, after.status),
                    (S::Reserved, S::Reserved | S::Started)
                        | (
                            S::Started,
                            S::Started | S::Succeeded | S::Failed | S::Unknown
                        )
                        | (S::Succeeded, S::Succeeded)
                        | (S::Failed, S::Failed)
                        | (S::Unknown, S::Unknown)
                );
                invalid_if!(
                    !status_ok || after.attempts < before.attempts,
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "cannot regress or replay an effect"
                );
                invalid_if!(
                    before.status == S::Reserved
                        && after.status == S::Started
                        && maintenance != Some(kind),
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "an effect attempt must start in its own same-state maintenance revision"
                );
                invalid_if!(
                    before.status == after.status && before != after,
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "effect facts cannot change without a status boundary"
                );
                invalid_if!(
                    before
                        .started_at
                        .as_ref()
                        .is_some_and(|value| after.started_at.as_ref() != Some(value))
                        || before
                            .settled_at
                            .as_ref()
                            .is_some_and(|value| after.settled_at.as_ref() != Some(value))
                        || before
                            .provider_effect_id
                            .as_ref()
                            .is_some_and(|value| after.provider_effect_id.as_ref() != Some(value))
                        || before
                            .error_code
                            .is_some_and(|value| after.error_code != Some(value)),
                    format!("transition.patch.effects.{}", kind.as_str()),
                    "recorded effect facts are immutable"
                );
            }
        }
    }
    Ok(())
}

fn validate_transition_delta(
    previous: &CollaborationRecordV1,
    next: &CollaborationRecordV1,
    maintenance: Option<CollaborationEffectKind>,
) -> ContractResult<()> {
    invalid_if!(
        previous.limits != next.limits,
        "transition.patch.limits",
        "policy limits are immutable"
    );
    let refreshing = next.state == CollaborationState::AwaitingApproval;
    invalid_if!(
        previous.state != CollaborationState::Requested
            && !refreshing
            && previous.source != next.source,
        "transition.patch.source",
        "source snapshots change only during approval refresh"
    );
    match (&previous.packet, &next.packet) {
        (None, Some(_)) if next.state != CollaborationState::AwaitingApproval => {
            return Err(ContractError::invalid(
                "transition.patch.packet",
                "can be created only with an approval preview",
            ));
        }
        (Some(_), None) => {
            return Err(ContractError::invalid(
                "transition.patch.packet",
                "packet metadata is immutable",
            ));
        }
        (Some(before), Some(after)) if !refreshing => {
            // Exhaustive destructure: a new packet field must be classified here.
            let CollaborationPacketRecordV1 {
                version,
                hash,
                bytes,
                context_bytes,
                omissions,
                body,
            } = before;
            invalid_if!(
                *version != after.version
                    || *hash != after.hash
                    || *bytes != after.bytes
                    || *context_bytes != after.context_bytes
                    || *omissions != after.omissions
                    || (!next.state.is_terminal() && *body != after.body),
                "transition.patch.packet",
                "packet metadata and active body are immutable"
            );
        }
        _ => {}
    }
    match (&previous.approval, &next.approval) {
        (None, Some(_)) if next.state != CollaborationState::AwaitingApproval => {
            return Err(ContractError::invalid(
                "transition.patch.approval",
                "can be created only with an approval preview",
            ));
        }
        (Some(_), None) => {
            return Err(ContractError::invalid(
                "transition.patch.approval",
                "approval metadata cannot be removed",
            ));
        }
        (Some(before), Some(after)) if !refreshing => {
            // Exhaustive destructure: a new approval field must be classified
            // here as part of the binding or as part of the recorded decision.
            let CollaborationApprovalRecordV1 {
                version,
                approval_id,
                collaboration_id,
                packet_hash,
                source_registry_revision,
                source_guard_hash,
                target_engine,
                created_at,
                expires_at,
                decision,
                decided_at: _,
            } = before;
            invalid_if!(
                *version != after.version
                    || *approval_id != after.approval_id
                    || *collaboration_id != after.collaboration_id
                    || *packet_hash != after.packet_hash
                    || *source_registry_revision != after.source_registry_revision
                    || *source_guard_hash != after.source_guard_hash
                    || *target_engine != after.target_engine
                    || *created_at != after.created_at
                    || *expires_at != after.expires_at,
                "transition.patch.approval",
                "approval binding is immutable"
            );
            invalid_if!(
                *decision != CollaborationApprovalDecision::Pending && before != after,
                "transition.patch.approval",
                "a consumed approval is immutable"
            );
        }
        _ => {}
    }
    invalid_if!(
        previous.target.engine != next.target.engine,
        "transition.patch.target.engine",
        "is immutable"
    );
    for (before, after, path) in [
        (
            previous.target.session_key.as_ref(),
            next.target.session_key.as_ref(),
            "transition.patch.target.sessionKey",
        ),
        (
            previous.target.provider_session_id.as_ref(),
            next.target.provider_session_id.as_ref(),
            "transition.patch.target.providerSessionId",
        ),
    ] {
        invalid_if!(
            before.is_some() && before != after,
            path,
            "cannot be removed or replaced"
        );
        invalid_if!(
            before.is_none()
                && after.is_some()
                && previous.state != CollaborationState::PreparingTarget
                && !(previous.state == CollaborationState::Unknown && next.state.is_terminal()),
            path,
            "appeared outside staging"
        );
    }
    invalid_if!(
        previous.target.reconciliation_marker.is_some()
            && previous.target.reconciliation_marker != next.target.reconciliation_marker,
        "transition.patch.target.reconciliationMarker",
        "is immutable once recorded"
    );
    invalid_if!(
        previous.target.reconciliation_marker.is_none()
            && next.target.reconciliation_marker.is_some()
            && previous.state != CollaborationState::PreparingTarget
            && !(previous.state == CollaborationState::Unknown && next.state.is_terminal()),
        "transition.patch.target.reconciliationMarker",
        "requires target staging or unknown reconciliation"
    );
    validate_effect_delta(
        &previous.effects,
        &next.effects,
        maintenance,
        &next.updated_at,
    )?;
    validate_peer_result_delta(previous, next)?;
    validate_worktree_delta(previous, next, refreshing)?;
    invalid_if!(
        previous.failure.is_some()
            && next.failure.is_none()
            && !(previous.state == CollaborationState::Unknown
                && next.state == CollaborationState::Completed),
        "transition.patch.failure",
        "can be cleared only by successful reconciliation"
    );
    invalid_if!(
        previous.failure.is_some()
            && next.failure.is_some()
            && previous.failure != next.failure
            && !(previous.state == CollaborationState::Unknown
                && next.state == CollaborationState::Failed),
        "transition.patch.failure",
        "cannot rewrite an existing failure"
    );
    invalid_if!(
        refreshing
            && previous.state != CollaborationState::Requested
            && previous.approval.as_ref().map(|value| &value.approval_id)
                == next.approval.as_ref().map(|value| &value.approval_id),
        "transition.patch.approval.approvalId",
        "approval refresh requires a fresh single-use approval record"
    );
    if let Some(kind) = maintenance {
        let before = previous.effects.get(kind);
        let after = next.effects.get(kind);
        invalid_if!(
            !matches!(
                (before, after),
                (
                    Some(CollaborationEffectRecordV1 {
                        status: CollaborationEffectStatus::Reserved,
                        attempts: 0,
                        ..
                    }),
                    Some(CollaborationEffectRecordV1 {
                        status: CollaborationEffectStatus::Started,
                        attempts: 1,
                        ..
                    })
                )
            ) || after.is_some_and(|effect| {
                effect.started_at.as_deref() != Some(next.updated_at.as_str())
                    || effect.settled_at.is_some()
                    || effect.provider_effect_id.is_some()
                    || effect.error_code.is_some()
            }),
            format!("transition.patch.effects.{}", kind.as_str()),
            "must durably change one reserved effect into one unconfirmed started attempt"
        );
    }
    Ok(())
}

fn validate_peer_result_delta(
    previous: &CollaborationRecordV1,
    next: &CollaborationRecordV1,
) -> ContractResult<()> {
    let (Some(before), Some(after)) = (&previous.peer_result, &next.peer_result) else {
        invalid_if!(
            previous.peer_result.is_none()
                && next.peer_result.is_some()
                && next.state != CollaborationState::ResultReady,
            "transition.patch.peerResult",
            "can be created only at result readiness"
        );
        invalid_if!(
            previous.peer_result.is_some() && next.peer_result.is_none(),
            "transition.patch.peerResult",
            "cannot be removed"
        );
        return Ok(());
    };
    invalid_if!(
        before.version != after.version
            || before.result_id != after.result_id
            || before.digest != after.digest
            || before.bytes != after.bytes
            || before.created_at != after.created_at
            || before.target_session_key != after.target_session_key,
        "transition.patch.peerResult",
        "result identity is immutable"
    );
    invalid_if!(
        !next.state.is_terminal() && before.body != after.body,
        "transition.patch.peerResult.body",
        "is immutable before terminal cleanup"
    );
    invalid_if!(
        before.queue_entry_id.is_some() && before.queue_entry_id != after.queue_entry_id,
        "transition.patch.peerResult.queueEntryId",
        "is immutable once queued"
    );
    let state_ok = matches!(
        (before.state, after.state),
        (
            CollaborationPeerResultState::Ready,
            CollaborationPeerResultState::Ready | CollaborationPeerResultState::Held
        ) | (
            CollaborationPeerResultState::Held,
            CollaborationPeerResultState::Held
                | CollaborationPeerResultState::Accepted
                | CollaborationPeerResultState::Discarded
                | CollaborationPeerResultState::DeliveryPending
        ) | (
            CollaborationPeerResultState::Accepted,
            CollaborationPeerResultState::Accepted | CollaborationPeerResultState::DeliveryPending
        ) | (
            CollaborationPeerResultState::Discarded,
            CollaborationPeerResultState::Discarded
        ) | (
            CollaborationPeerResultState::DeliveryPending,
            CollaborationPeerResultState::DeliveryPending
                | CollaborationPeerResultState::Delivered
                | CollaborationPeerResultState::Unknown
        ) | (
            CollaborationPeerResultState::Delivered,
            CollaborationPeerResultState::Delivered
        ) | (
            CollaborationPeerResultState::Unknown,
            CollaborationPeerResultState::Unknown
        )
    );
    invalid_if!(
        !state_ok,
        "transition.patch.peerResult.state",
        "cannot regress or replay delivery"
    );
    Ok(())
}

fn validate_worktree_delta(
    previous: &CollaborationRecordV1,
    next: &CollaborationRecordV1,
    refreshing: bool,
) -> ContractResult<()> {
    match (&previous.worktree, &next.worktree) {
        (None, Some(_)) if next.state != CollaborationState::AwaitingApproval => {
            Err(ContractError::invalid(
                "transition.patch.worktree",
                "can be discovered only during source resolution",
            ))
        }
        (Some(_), None) => Err(ContractError::invalid(
            "transition.patch.worktree",
            "cannot be removed",
        )),
        (Some(before), Some(after)) if !refreshing => match (before, after) {
            (CollaborationWorktreeV1::Managed(before), CollaborationWorktreeV1::Managed(after)) => {
                invalid_if!(
                    before.canonical_path != after.canonical_path
                        || before.branch != after.branch
                        || before
                            .transfer_id
                            .as_ref()
                            .is_some_and(|value| after.transfer_id.as_ref() != Some(value))
                        || after.owner_revision < before.owner_revision,
                    "transition.patch.worktree",
                    "cannot replace or regress managed ownership"
                );
                let edge_ok = matches!(
                    (&before.owner, &after.owner),
                    (
                        CollaborationWorktreeOwnerV1::Source { .. },
                        CollaborationWorktreeOwnerV1::Source { .. }
                            | CollaborationWorktreeOwnerV1::Escrow { .. }
                    ) | (
                        CollaborationWorktreeOwnerV1::Escrow { .. },
                        CollaborationWorktreeOwnerV1::Escrow { .. }
                            | CollaborationWorktreeOwnerV1::Target { .. }
                    ) | (
                        CollaborationWorktreeOwnerV1::Target { .. },
                        CollaborationWorktreeOwnerV1::Target { .. }
                    )
                );
                invalid_if!(
                    !edge_ok,
                    "transition.patch.worktree.owner",
                    "cannot regress or create dual ownership"
                );
                invalid_if!(
                    before.owner != after.owner
                        && after.owner_revision != before.owner_revision.saturating_add(1),
                    "transition.patch.worktree.ownerRevision",
                    "must advance once with ownership"
                );
                Ok(())
            }
            _ if before == after => Ok(()),
            _ => Err(ContractError::invalid(
                "transition.patch.worktree",
                "is immutable",
            )),
        },
        _ => Ok(()),
    }
}

pub fn transition_collaboration_record(
    current: &CollaborationRecordV1,
    request: CollaborationTransitionRequest,
) -> ContractResult<CollaborationTransitionResult> {
    current.validate()?;
    safe_integer(request.expected_revision, "transition.expectedRevision")?;
    if current.revision != request.expected_revision {
        return Err(ContractError::transition(
            ContractErrorReason::StaleRevision,
            format!(
                "Expected collaboration revision {}, but current revision is {}.",
                request.expected_revision, current.revision
            ),
        ));
    }
    if current.state != request.expected_state {
        return Err(ContractError::transition(
            ContractErrorReason::StaleState,
            format!(
                "Expected collaboration state {}, but current state is {}.",
                request.expected_state, current.state
            ),
        ));
    }
    let maintenance = request.to == current.state;
    let approval_refresh = maintenance
        && current.state == CollaborationState::AwaitingApproval
        && request.maintenance_effect.is_none();
    let delivery_rebind = maintenance
        && current.state == CollaborationState::DeliveryPending
        && request.maintenance_effect.is_none();
    let expected_maintenance = maintenance_effect_for_state(current.state, current.intent.mode);
    if maintenance
        && !approval_refresh
        && !delivery_rebind
        && (request.maintenance_effect.is_none()
            || request.maintenance_effect != expected_maintenance)
    {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            format!(
                "Collaboration state {} does not permit that maintenance effect.",
                current.state
            ),
        ));
    }
    if !maintenance && request.maintenance_effect.is_some() {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            "Effect-start maintenance cannot be combined with a logical state transition.",
        ));
    }
    if !maintenance
        && !legal_collaboration_transitions(current.state, current.intent.mode)
            .contains(&request.to)
    {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            format!(
                "Transition {} -> {} is not legal for {:?}.",
                current.state, request.to, current.intent.mode
            ),
        ));
    }
    if request.to == CollaborationState::Cancelled && !cancellation_is_safe(current) {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            format!(
                "Cancellation is no longer safe from collaboration state {}.",
                current.state
            ),
        ));
    }
    if request.to == CollaborationState::AwaitingApproval
        && current.state != CollaborationState::Requested
        && !approval_refresh_is_safe(current)
    {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            format!(
                "Approval refresh is no longer safe from collaboration state {}.",
                current.state
            ),
        ));
    }
    let at = timestamp_millis(&request.at, "transition.at")?;
    let current_updated = timestamp_millis(&current.updated_at, "record.updatedAt")?;
    if at < current_updated {
        return Err(ContractError::transition(
            ContractErrorReason::StaleRevision,
            "Transition timestamp precedes the current collaboration revision.",
        ));
    }
    if let Some(event_id) = request.event_id.as_deref() {
        bounded_identifier(event_id, "transition.eventId")?;
    }
    invalid_if!(
        request.patch.clear_failure && request.patch.failure.is_some(),
        "transition.patch.failure",
        "cannot both clear and replace a failure"
    );
    let mut next = current.clone();
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| ContractError::invalid("record.revision", "exhausted its revision range"))?;
    next.state = request.to;
    next.updated_at = request.at;
    if let Some(source) = request.patch.source {
        next.source = source;
    }
    if let Some(target) = request.patch.target {
        next.target = target;
    }
    if let Some(packet) = request.patch.packet {
        next.packet = Some(packet);
    }
    if let Some(approval) = request.patch.approval {
        next.approval = Some(approval);
    }
    if let Some(effects) = request.patch.effects {
        next.effects = effects;
    }
    if let Some(peer_result) = request.patch.peer_result {
        next.peer_result = Some(peer_result);
    }
    if let Some(worktree) = request.patch.worktree {
        next.worktree = Some(worktree);
    }
    if request.patch.clear_failure {
        next.failure = None;
    } else if let Some(failure) = request.patch.failure {
        next.failure = Some(failure);
    }
    if next.state.is_terminal() {
        next.terminal_at = Some(next.updated_at.clone());
        next.intent.objective = None;
        if let Some(packet) = next.packet.as_mut() {
            packet.body = None;
        }
        if let Some(result) = next.peer_result.as_mut() {
            result.body = None;
        }
    } else {
        next.terminal_at = None;
    }
    next.validate()?;
    validate_transition_delta(current, &next, request.maintenance_effect)?;
    // Exhaustive destructure: a new record field must be classified here as one
    // effect-start maintenance may move (`revision`, `updated_at`, `effects`) or
    // one it must leave alone.
    let CollaborationRecordV1 {
        version,
        collaboration_id,
        revision: _,
        state,
        intent,
        dedupe_key,
        payload_hash,
        created_at,
        updated_at: _,
        source,
        target,
        packet,
        approval,
        effects: _,
        peer_result,
        worktree,
        limits,
        failure,
        terminal_at,
    } = current;
    if let Some(kind) = request.maintenance_effect
        && (*version != next.version
            || *collaboration_id != next.collaboration_id
            || *dedupe_key != next.dedupe_key
            || *payload_hash != next.payload_hash
            || *created_at != next.created_at
            || *state != next.state
            || *intent != next.intent
            || *source != next.source
            || *target != next.target
            || *packet != next.packet
            || *approval != next.approval
            || *peer_result != next.peer_result
            || *worktree != next.worktree
            || *limits != next.limits
            || *failure != next.failure
            || *terminal_at != next.terminal_at)
    {
        return Err(ContractError::invalid(
            "transition.patch",
            &format!(
                "effect-start maintenance may change only the selected {} effect",
                kind.as_str()
            ),
        ));
    }
    let audit = audit_event_for_validated_transition(
        current,
        &next,
        request
            .event_id
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
    )?;
    Ok(CollaborationTransitionResult {
        record: next,
        audit,
    })
}
