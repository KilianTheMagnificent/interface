//! Receipt and audit-event projections of a validated record.

use super::{
    AuditApprovalDecision, CollaborationApprovalDecision, CollaborationAuditEventV1,
    CollaborationErrorV1, CollaborationReceiptV1, CollaborationRecordV1, CollaborationState,
    ContractError, ContractErrorReason, ContractResult, JS_MAX_SAFE_INTEGER, PACKET_BYTES_MAX,
    RESULT_BYTES_MAX, SAFE_NEXT_ACTION_BYTES_MAX, bounded_identifier, bounded_safe_line,
    cancellation_is_safe, legal_collaboration_transitions, maintenance_effect_for_state,
    safe_integer, timestamp_millis,
};

pub fn receipt_for_collaboration(
    record: &CollaborationRecordV1,
) -> ContractResult<CollaborationReceiptV1> {
    record.validate()?;
    let receipt = receipt_for_validated_collaboration(record);
    receipt.validate()?;
    Ok(receipt)
}

pub(crate) fn receipt_for_validated_collaboration(
    record: &CollaborationRecordV1,
) -> CollaborationReceiptV1 {
    let error = record.failure.as_ref().map(|failure| CollaborationErrorV1 {
        version: 1,
        code: failure.code,
        collaboration_id: Some(record.collaboration_id.clone()),
        safe_next_action: failure.code.safe_next_action().to_owned(),
        retryable: failure.retryable,
    });
    CollaborationReceiptV1 {
        version: 1,
        collaboration_id: record.collaboration_id.clone(),
        revision: record.revision,
        state: record.state,
        mode: record.intent.mode,
        source_engine: record.source.engine,
        target_engine: record.target.engine,
        approval_required: matches!(
            record.state,
            CollaborationState::Requested | CollaborationState::AwaitingApproval
        ),
        cancellation_safe: cancellation_is_safe(record),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        error,
    }
}

impl CollaborationErrorV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "error.version",
            "must be the supported contract version"
        );
        if let Some(collaboration_id) = self.collaboration_id.as_deref() {
            bounded_identifier(collaboration_id, "error.collaborationId")?;
        }
        bounded_safe_line(
            &self.safe_next_action,
            SAFE_NEXT_ACTION_BYTES_MAX,
            "error.safeNextAction",
        )?;
        invalid_if!(
            self.safe_next_action != self.code.safe_next_action(),
            "error.safeNextAction",
            "must use the static action for its error code"
        );
        invalid_if!(
            self.code.is_ambiguous() && self.retryable,
            "error.retryable",
            "ambiguous outcomes are never retryable"
        );
        Ok(())
    }
}

impl CollaborationReceiptV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "receipt.version",
            "must be the supported contract version"
        );
        bounded_identifier(&self.collaboration_id, "receipt.collaborationId")?;
        safe_integer(self.revision, "receipt.revision")?;
        invalid_if!(
            self.source_engine == self.target_engine,
            "receipt.targetEngine",
            "must differ from source"
        );
        let created = timestamp_millis(&self.created_at, "receipt.createdAt")?;
        let updated = timestamp_millis(&self.updated_at, "receipt.updatedAt")?;
        invalid_if!(updated < created, "receipt.updatedAt", "precedes creation");
        if let Some(error) = self.error.as_ref() {
            error.validate()?;
            invalid_if!(
                error
                    .collaboration_id
                    .as_ref()
                    .is_some_and(|id| id != &self.collaboration_id),
                "receipt.error.collaborationId",
                "does not match receipt"
            );
        }
        Ok(())
    }
}

pub fn audit_event_for_transition(
    previous: &CollaborationRecordV1,
    next: &CollaborationRecordV1,
    event_id: String,
) -> ContractResult<CollaborationAuditEventV1> {
    previous.validate()?;
    next.validate()?;
    audit_event_for_validated_transition(previous, next, event_id)
}

pub(super) fn audit_event_for_validated_transition(
    previous: &CollaborationRecordV1,
    next: &CollaborationRecordV1,
    event_id: String,
) -> ContractResult<CollaborationAuditEventV1> {
    if next.collaboration_id != previous.collaboration_id
        || next.revision != previous.revision.saturating_add(1)
        || next.intent.mode != previous.intent.mode
        || (previous.state != next.state
            && !legal_collaboration_transitions(previous.state, previous.intent.mode)
                .contains(&next.state))
        || (previous.state == next.state
            && previous.state != CollaborationState::AwaitingApproval
            && maintenance_effect_for_state(previous.state, previous.intent.mode).is_none()
            && previous.state != CollaborationState::DeliveryPending)
    {
        return Err(ContractError::transition(
            ContractErrorReason::IllegalTransition,
            "Audit projection requires one legal transition of the same collaboration record.",
        ));
    }
    bounded_identifier(&event_id, "audit.eventId")?;
    let created = timestamp_millis(&next.created_at, "record.createdAt")?;
    let updated = timestamp_millis(&next.updated_at, "record.updatedAt")?;
    let approval_decision = next
        .approval
        .as_ref()
        .and_then(|approval| match approval.decision {
            CollaborationApprovalDecision::Approved => Some(AuditApprovalDecision::Approved),
            CollaborationApprovalDecision::Denied => Some(AuditApprovalDecision::Denied),
            CollaborationApprovalDecision::Expired => Some(AuditApprovalDecision::Expired),
            CollaborationApprovalDecision::Pending => None,
        });
    let event = CollaborationAuditEventV1 {
        version: 1,
        event_id,
        collaboration_id: next.collaboration_id.clone(),
        revision: next.revision,
        at: next.updated_at.clone(),
        mode: next.intent.mode,
        source_engine: next.source.engine,
        target_engine: next.target.engine,
        from: previous.state,
        to: next.state,
        reason_code: next.failure.as_ref().map(|failure| failure.code),
        packet_bytes: next.packet.as_ref().map(|packet| packet.bytes),
        result_bytes: next.peer_result.as_ref().map(|result| result.bytes),
        duration_ms: Some(
            u64::try_from(updated.signed_duration_since(created).num_milliseconds()).unwrap_or(0),
        ),
        approval_decision,
    };
    event.validate()?;
    Ok(event)
}

impl CollaborationAuditEventV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1 || self.revision == 0 || self.revision > JS_MAX_SAFE_INTEGER,
            "audit.version",
            "must be version one with a positive revision"
        );
        bounded_identifier(&self.event_id, "audit.eventId")?;
        bounded_identifier(&self.collaboration_id, "audit.collaborationId")?;
        timestamp_millis(&self.at, "audit.at")?;
        invalid_if!(
            self.source_engine == self.target_engine,
            "audit.targetEngine",
            "must differ from source"
        );
        invalid_if!(
            self.from != self.to
                && !legal_collaboration_transitions(self.from, self.mode).contains(&self.to),
            "audit.to",
            "is not a legal transition for mode"
        );
        invalid_if!(
            self.from == self.to
                && self.from != CollaborationState::AwaitingApproval
                && maintenance_effect_for_state(self.from, self.mode).is_none()
                && self.from != CollaborationState::DeliveryPending,
            "audit.to",
            "is not a legal maintenance state for mode"
        );
        invalid_if!(
            self.packet_bytes
                .is_some_and(|bytes| bytes > PACKET_BYTES_MAX)
                || self
                    .result_bytes
                    .is_some_and(|bytes| bytes > RESULT_BYTES_MAX)
                || self
                    .duration_ms
                    .is_some_and(|duration| duration > JS_MAX_SAFE_INTEGER),
            "audit.packetBytes",
            "is outside its version-one bound"
        );
        Ok(())
    }
}
