//! Per-sub-record `validate()` rules, record creation and full
//! `CollaborationRecordV1::validate`, one `invalid_if!` per rule in
//! first-failure order.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::{
    APPROVAL_TIMEOUT_MS, COLLABORATION_PACKET_VERSION, CONSULTATION_HARD_TIMEOUT_MS,
    CONTEXT_BYTES_MAX, CollaborationApprovalDecision, CollaborationApprovalRecordV1,
    CollaborationEffectKind, CollaborationEffectMapV1, CollaborationEffectRecordV1,
    CollaborationEffectStatus, CollaborationEngineId, CollaborationErrorCode,
    CollaborationFailureV1, CollaborationLiveStatus, CollaborationMode,
    CollaborationPacketRecordV1, CollaborationPeerAcceptanceV1, CollaborationPeerDecision,
    CollaborationPeerResultState, CollaborationPeerResultV1, CollaborationPolicyLimitsV1,
    CollaborationRecordV1, CollaborationSourceV1, CollaborationState, CollaborationTargetV1,
    CollaborationWorktreeOwnerV1, CollaborationWorktreeV1, ContractError, ContractResult,
    CreateCollaborationRecordOptions, HANDOFF_PREPARE_TIMEOUT_MS, JS_MAX_SAFE_INTEGER,
    MAX_ACTIVE_CONSULTATIONS_PER_SOURCE, MAX_ACTIVE_HANDOFFS_PER_SOURCE,
    MAX_ACTIVE_HANDOFFS_PER_WORKTREE, MAX_DEPTH, MAX_RUNNING_TARGETS, ManagedWorktreeV1,
    OBJECTIVE_BYTES_MAX, OBJECTIVE_CHARACTERS_MAX, PACKET_BYTES_MAX, PATH_BYTES_MAX,
    PROVIDER_SESSION_ID_BYTES_MAX, REQUEST_TOOL_TIMEOUT_MS, RESULT_BYTES_MAX,
    SOURCE_SESSION_KEY_BYTES_MAX, SOURCE_TITLE_BYTES_MAX, allowed_effect, assert_normalized_opaque,
    bounded_body, bounded_identifier, bounded_path, bounded_safe_line, canonical_json,
    dedupe_key_for_validated_intent, domain_hash, engine_from_session_key, hash_value,
    payload_hash_for_validated_intent, peer_result_id, provider_id_from_session_key,
    require_effect_status, require_only_effects, safe_integer, sha256, timestamp_millis,
    validate_effect_dependencies,
};

impl CollaborationSourceV1 {
    pub fn validate(&self) -> ContractResult<()> {
        assert_normalized_opaque(
            &self.session_key,
            SOURCE_SESSION_KEY_BYTES_MAX,
            1,
            "record.source.sessionKey",
        )?;
        invalid_if!(
            engine_from_session_key(&self.session_key)? != self.engine,
            "record.source.engine",
            "does not match the source session key"
        );
        if let Some(provider_id) = self.provider_session_id.as_deref() {
            assert_normalized_opaque(
                provider_id,
                PROVIDER_SESSION_ID_BYTES_MAX,
                1,
                "record.source.providerSessionId",
            )?;
            invalid_if!(
                provider_id != provider_id_from_session_key(&self.session_key)?,
                "record.source.providerSessionId",
                "does not match the source session key"
            );
        }
        if let Some(title) = self.title.as_deref() {
            bounded_safe_line(title, SOURCE_TITLE_BYTES_MAX, "record.source.title")?;
        }
        if let Some(runtime_epoch) = self.runtime_epoch.as_deref() {
            bounded_identifier(runtime_epoch, "record.source.runtimeEpoch")?;
        }
        if let Some(path) = self.canonical_cwd.as_deref() {
            bounded_path(path, "record.source.canonicalCwd")?;
        }
        if let Some(path) = self.canonical_project_root.as_deref() {
            bounded_path(path, "record.source.canonicalProjectRoot")?;
        }
        if let Some(hash) = self.pending_approval_set_hash.as_deref() {
            hash_value(hash, "record.source.pendingApprovalSetHash")?;
        }
        for (value, path) in [
            (self.registry_revision, "record.source.registryRevision"),
            (
                self.live_handle_generation,
                "record.source.liveHandleGeneration",
            ),
            (self.queue_revision, "record.source.queueRevision"),
            (self.delivery_revision, "record.source.deliveryRevision"),
            (
                self.worktree_owner_revision,
                "record.source.worktreeOwnerRevision",
            ),
        ] {
            if let Some(value) = value {
                safe_integer(value, path)?;
            }
        }
        Ok(())
    }

    pub fn require_approval_bound(&self) -> ContractResult<()> {
        self.validate()?;
        invalid_if!(
            self.provider_session_id.is_none()
                || self.registry_revision.is_none()
                || self.runtime_epoch.is_none()
                || self.live_handle_generation.is_none()
                || self.live_status.is_none()
                || self.canonical_cwd.is_none()
                || self.canonical_project_root.is_none()
                || self.pending_approval_set_hash.is_none()
                || self.queue_revision.is_none()
                || self.delivery_revision.is_none()
                || self.delivery_unknown.is_none(),
            "record.source",
            "must contain every approval-bound live source guard"
        );
        Ok(())
    }
}

impl CollaborationTargetV1 {
    pub fn validate(&self) -> ContractResult<()> {
        if let Some(session_key) = self.session_key.as_deref() {
            assert_normalized_opaque(
                session_key,
                SOURCE_SESSION_KEY_BYTES_MAX,
                1,
                "record.target.sessionKey",
            )?;
            invalid_if!(
                engine_from_session_key(session_key)? != self.engine,
                "record.target.sessionKey",
                "does not match the target engine"
            );
        }
        if let Some(provider_id) = self.provider_session_id.as_deref() {
            assert_normalized_opaque(
                provider_id,
                PROVIDER_SESSION_ID_BYTES_MAX,
                1,
                "record.target.providerSessionId",
            )?;
            if let Some(session_key) = self.session_key.as_deref()
                && provider_id != provider_id_from_session_key(session_key)?
            {
                return Err(ContractError::invalid(
                    "record.target.providerSessionId",
                    "does not match the target session key",
                ));
            }
        }
        if let Some(marker) = self.reconciliation_marker.as_deref() {
            bounded_identifier(marker, "record.target.reconciliationMarker")?;
        }
        Ok(())
    }

    fn require_session(&self) -> ContractResult<()> {
        invalid_if!(
            self.session_key.is_none() || self.provider_session_id.is_none(),
            "record.target",
            "must contain the staged live target session"
        );
        Ok(())
    }
}

pub fn source_guard_hash_for_approval(source: &CollaborationSourceV1) -> ContractResult<String> {
    source.require_approval_bound()?;
    let value = serde_json::json!({
        "engine": source.engine,
        "sessionKey": source.session_key,
        "providerSessionId": source.provider_session_id,
        "registryRevision": source.registry_revision,
        "runtimeEpoch": source.runtime_epoch,
        "liveHandleGeneration": source.live_handle_generation,
        "liveStatus": source.live_status,
        "canonicalCwd": source.canonical_cwd,
        "canonicalProjectRoot": source.canonical_project_root,
        "pendingApprovalSetHash": source.pending_approval_set_hash,
        "queueRevision": source.queue_revision,
        "deliveryRevision": source.delivery_revision,
        "deliveryUnknown": source.delivery_unknown,
        "worktreeOwnerRevision": source.worktree_owner_revision,
    });
    Ok(domain_hash(
        "iface-collaboration-source-guard-v1",
        &canonical_json(&value),
    ))
}

impl CollaborationPolicyLimitsV1 {
    pub fn validate(&self, mode: CollaborationMode) -> ContractResult<()> {
        invalid_if!(
            self.version != 1
                || self.depth > MAX_DEPTH
                || !(1..=OBJECTIVE_CHARACTERS_MAX as u64).contains(&self.max_objective_characters)
                || !(1..=OBJECTIVE_BYTES_MAX as u64).contains(&self.max_objective_bytes)
                || !(1..=PACKET_BYTES_MAX).contains(&self.max_packet_bytes)
                || self.max_context_bytes > CONTEXT_BYTES_MAX
                || self.max_context_bytes > self.max_packet_bytes
                || !(1..=RESULT_BYTES_MAX).contains(&self.max_result_bytes)
                || !(1..=MAX_ACTIVE_CONSULTATIONS_PER_SOURCE)
                    .contains(&self.max_active_consultations_per_source)
                || !(1..=MAX_ACTIVE_HANDOFFS_PER_SOURCE)
                    .contains(&self.max_active_handoffs_per_source)
                || !(1..=MAX_ACTIVE_HANDOFFS_PER_WORKTREE)
                    .contains(&self.max_active_handoffs_per_worktree)
                || !(1..=MAX_RUNNING_TARGETS).contains(&self.max_running_targets)
                || !(1..=APPROVAL_TIMEOUT_MS).contains(&self.approval_timeout_ms)
                || !(1..=REQUEST_TOOL_TIMEOUT_MS).contains(&self.request_tool_timeout_ms),
            "record.limits",
            "contains a value outside its version-one ceiling"
        );
        match mode {
            CollaborationMode::Consult => {
                invalid_if!(
                    !self.consultation_timeout_ms.is_some_and(|timeout| (1
                        ..=CONSULTATION_HARD_TIMEOUT_MS)
                        .contains(&timeout))
                        || self.handoff_prepare_timeout_ms.is_some(),
                    "record.limits",
                    "consult mode requires only a consultation timeout"
                );
            }
            CollaborationMode::Handoff => {
                invalid_if!(
                    !self
                        .handoff_prepare_timeout_ms
                        .is_some_and(|timeout| (1..=HANDOFF_PREPARE_TIMEOUT_MS).contains(&timeout))
                        || self.consultation_timeout_ms.is_some(),
                    "record.limits",
                    "handoff mode requires only a handoff preparation timeout"
                );
            }
        }
        Ok(())
    }
}

pub fn create_collaboration_record(
    options: CreateCollaborationRecordOptions,
) -> ContractResult<CollaborationRecordV1> {
    bounded_identifier(&options.collaboration_id, "collaborationId")?;
    options.intent.validate()?;
    timestamp_millis(&options.created_at, "createdAt")?;
    invalid_if!(
        options.depth > MAX_DEPTH,
        "limits.depth",
        "is outside its bound"
    );
    let mut limits = CollaborationPolicyLimitsV1::defaults(options.intent.mode, options.depth);
    macro_rules! apply {
        ($field:ident) => {
            if let Some(value) = options.limits.$field {
                limits.$field = value;
            }
        };
    }
    apply!(max_objective_characters);
    apply!(max_objective_bytes);
    apply!(max_packet_bytes);
    apply!(max_context_bytes);
    apply!(max_result_bytes);
    apply!(max_active_consultations_per_source);
    apply!(max_active_handoffs_per_source);
    apply!(max_active_handoffs_per_worktree);
    apply!(max_running_targets);
    apply!(approval_timeout_ms);
    apply!(request_tool_timeout_ms);
    if let Some(value) = options.limits.consultation_timeout_ms {
        limits.consultation_timeout_ms = Some(value);
    }
    if let Some(value) = options.limits.handoff_prepare_timeout_ms {
        limits.handoff_prepare_timeout_ms = Some(value);
    }
    limits.validate(options.intent.mode)?;
    let source_engine = engine_from_session_key(&options.intent.source_session_key)?;
    let dedupe_key = dedupe_key_for_validated_intent(&options.intent);
    let payload_hash = payload_hash_for_validated_intent(&options.intent);
    let record = CollaborationRecordV1 {
        version: 1,
        collaboration_id: options.collaboration_id,
        revision: 0,
        state: CollaborationState::Requested,
        intent: options.intent,
        dedupe_key,
        payload_hash,
        created_at: options.created_at.clone(),
        updated_at: options.created_at,
        source: CollaborationSourceV1 {
            engine: source_engine,
            session_key: String::new(),
            provider_session_id: None,
            title: None,
            registry_revision: None,
            runtime_epoch: None,
            live_handle_generation: None,
            live_status: None,
            canonical_cwd: None,
            canonical_project_root: None,
            pending_approval_set_hash: None,
            queue_revision: None,
            delivery_revision: None,
            delivery_unknown: None,
            worktree_owner_revision: None,
        },
        target: CollaborationTargetV1 {
            engine: CollaborationEngineId::Claude,
            session_key: None,
            provider_session_id: None,
            reconciliation_marker: None,
        },
        packet: None,
        approval: None,
        effects: CollaborationEffectMapV1::default(),
        peer_result: None,
        worktree: None,
        limits,
        failure: None,
        terminal_at: None,
    };
    // Fill identity fields after moving the intent without cloning its body.
    let mut record = record;
    record.source.session_key = record.intent.source_session_key.clone();
    record.target.engine = record.intent.target_engine;
    record.validate()?;
    Ok(record)
}

impl CollaborationPacketRecordV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != COLLABORATION_PACKET_VERSION,
            "record.packet.version",
            "must be the supported packet version"
        );
        hash_value(&self.hash, "record.packet.hash")?;
        invalid_if!(
            self.bytes > PACKET_BYTES_MAX
                || self.context_bytes > CONTEXT_BYTES_MAX
                || self.context_bytes > self.bytes
                || self.omissions > JS_MAX_SAFE_INTEGER,
            "record.packet.bytes",
            "is outside the packet or context bound"
        );
        if let Some(body) = self.body.as_deref() {
            bounded_body(
                body,
                usize::try_from(PACKET_BYTES_MAX).expect("packet limit fits usize"),
                "record.packet.body",
            )?;
            invalid_if!(
                u64::try_from(body.len()).unwrap_or(u64::MAX) != self.bytes,
                "record.packet.bytes",
                "does not match packet body"
            );
            invalid_if!(
                sha256(body) != self.hash,
                "record.packet.hash",
                "does not match packet body"
            );
        }
        Ok(())
    }
}

impl CollaborationApprovalRecordV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "record.approval.version",
            "must be the supported contract version"
        );
        bounded_identifier(&self.approval_id, "record.approval.approvalId")?;
        bounded_identifier(&self.collaboration_id, "record.approval.collaborationId")?;
        hash_value(&self.packet_hash, "record.approval.packetHash")?;
        safe_integer(
            self.source_registry_revision,
            "record.approval.sourceRegistryRevision",
        )?;
        hash_value(&self.source_guard_hash, "record.approval.sourceGuardHash")?;
        let created = timestamp_millis(&self.created_at, "record.approval.createdAt")?;
        let expires = timestamp_millis(&self.expires_at, "record.approval.expiresAt")?;
        let lifetime = expires.signed_duration_since(created).num_milliseconds();
        invalid_if!(
            lifetime <= 0 || u64::try_from(lifetime).unwrap_or(u64::MAX) > APPROVAL_TIMEOUT_MS,
            "record.approval.expiresAt",
            "must be within the approval timeout ceiling"
        );
        match (self.decision, self.decided_at.as_deref()) {
            (CollaborationApprovalDecision::Pending, Some(_)) => {
                return Err(ContractError::invalid(
                    "record.approval.decidedAt",
                    "is forbidden while approval is pending",
                ));
            }
            (CollaborationApprovalDecision::Pending, None) => {}
            (_, None) => {
                return Err(ContractError::invalid(
                    "record.approval.decidedAt",
                    "is required after an approval decision",
                ));
            }
            (_, Some(decided_at)) => {
                let decided = timestamp_millis(decided_at, "record.approval.decidedAt")?;
                invalid_if!(
                    decided < created,
                    "record.approval.decidedAt",
                    "precedes approval creation"
                );
            }
        }
        Ok(())
    }
}

impl CollaborationEffectRecordV1 {
    pub fn validate(&self) -> ContractResult<()> {
        let kind = self.kind.as_str();
        invalid_if!(
            self.version != 1,
            format_args!("record.effects.{kind}.version"),
            "must be the supported contract version"
        );
        bounded_identifier(
            &self.effect_id,
            format_args!("record.effects.{kind}.effectId"),
        )?;
        invalid_if!(
            self.attempts > 1,
            format_args!("record.effects.{kind}.attempts"),
            "must be zero or one"
        );
        let created = timestamp_millis(
            &self.created_at,
            format_args!("record.effects.{kind}.createdAt"),
        )?;
        let deadline = timestamp_millis(
            &self.deadline_at,
            format_args!("record.effects.{kind}.deadlineAt"),
        )?;
        invalid_if!(
            deadline <= created,
            format_args!("record.effects.{kind}.deadlineAt"),
            "must follow effect creation"
        );
        let started = self
            .started_at
            .as_deref()
            .map(|at| timestamp_millis(at, format_args!("record.effects.{kind}.startedAt")))
            .transpose()?;
        let settled = self
            .settled_at
            .as_deref()
            .map(|at| timestamp_millis(at, format_args!("record.effects.{kind}.settledAt")))
            .transpose()?;
        invalid_if!(
            started.is_some_and(|at| at < created)
                || settled
                    .is_some_and(|at| started.is_none_or(|start| at < start) || at > deadline),
            format_args!("record.effects.{kind}"),
            "contains unordered effect timestamps"
        );
        if let Some(provider_effect_id) = self.provider_effect_id.as_deref() {
            bounded_identifier(
                provider_effect_id,
                format_args!("record.effects.{kind}.providerEffectId"),
            )?;
        }
        match self.status {
            CollaborationEffectStatus::Reserved => {
                invalid_if!(
                    self.attempts != 0
                        || started.is_some()
                        || settled.is_some()
                        || self.provider_effect_id.is_some()
                        || self.error_code.is_some(),
                    format_args!("record.effects.{kind}"),
                    "reserved effects cannot contain dispatch or settlement facts"
                );
            }
            CollaborationEffectStatus::Started => {
                invalid_if!(
                    self.attempts != 1
                        || started.is_none()
                        || settled.is_some()
                        || self.error_code.is_some(),
                    format_args!("record.effects.{kind}"),
                    "started effects require one unsettled attempt"
                );
            }
            CollaborationEffectStatus::Succeeded
            | CollaborationEffectStatus::Failed
            | CollaborationEffectStatus::Unknown => {
                invalid_if!(
                    self.attempts != 1 || started.is_none() || settled.is_none(),
                    format_args!("record.effects.{kind}"),
                    "settled effects require one started attempt and settlement time"
                );
                match (self.status, self.error_code) {
                    (CollaborationEffectStatus::Succeeded, Some(_)) => {
                        return Err(ContractError::invalid(
                            format_args!("record.effects.{kind}.errorCode"),
                            "is forbidden for a successful effect",
                        ));
                    }
                    (CollaborationEffectStatus::Failed, Some(code)) if !code.is_ambiguous() => {}
                    (CollaborationEffectStatus::Unknown, Some(code)) if code.is_ambiguous() => {}
                    (CollaborationEffectStatus::Failed, _) => {
                        return Err(ContractError::invalid(
                            format_args!("record.effects.{kind}.errorCode"),
                            "must describe a known unsuccessful outcome",
                        ));
                    }
                    (CollaborationEffectStatus::Unknown, _) => {
                        return Err(ContractError::invalid(
                            format_args!("record.effects.{kind}.errorCode"),
                            "must describe an ambiguous outcome",
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

impl CollaborationEffectMapV1 {
    fn validate(&self) -> ContractResult<()> {
        let mut ids = BTreeSet::new();
        for effect in self.iter() {
            effect.validate()?;
            invalid_if!(
                self.get(effect.kind) != Some(effect),
                format!("record.effects.{}.kind", effect.kind.as_str()),
                "does not match its effect map key"
            );
            invalid_if!(
                !ids.insert(effect.effect_id.as_str()),
                "record.effects",
                "effect IDs must be unique"
            );
        }
        Ok(())
    }
}

impl CollaborationPeerAcceptanceV1 {
    fn validate(&self, result_id: &str, result_digest: &str) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "record.peerResult.acceptance.version",
            "must be the supported contract version"
        );
        bounded_identifier(&self.receipt_id, "record.peerResult.acceptance.receiptId")?;
        hash_value(&self.result_id, "record.peerResult.acceptance.resultId")?;
        hash_value(
            &self.result_digest,
            "record.peerResult.acceptance.resultDigest",
        )?;
        invalid_if!(
            self.result_id != result_id || self.result_digest != result_digest,
            "record.peerResult.acceptance",
            "does not match the held peer result"
        );
        timestamp_millis(&self.decided_at, "record.peerResult.acceptance.decidedAt")?;
        Ok(())
    }
}

impl CollaborationPeerResultV1 {
    fn validate(&self, collaboration_id: &str) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "record.peerResult.version",
            "must be the supported contract version"
        );
        hash_value(&self.result_id, "record.peerResult.resultId")?;
        hash_value(&self.digest, "record.peerResult.digest")?;
        invalid_if!(
            self.result_id != peer_result_id(collaboration_id, &self.digest)?,
            "record.peerResult.resultId",
            "does not match collaboration and result digest"
        );
        invalid_if!(
            self.bytes > RESULT_BYTES_MAX,
            "record.peerResult.bytes",
            "is outside its bound"
        );
        timestamp_millis(&self.created_at, "record.peerResult.createdAt")?;
        assert_normalized_opaque(
            &self.target_session_key,
            SOURCE_SESSION_KEY_BYTES_MAX,
            1,
            "record.peerResult.targetSessionKey",
        )?;
        if let Some(queue_entry_id) = self.queue_entry_id.as_deref() {
            bounded_identifier(queue_entry_id, "record.peerResult.queueEntryId")?;
        }
        if let Some(acceptance) = self.acceptance.as_ref() {
            acceptance.validate(&self.result_id, &self.digest)?;
        }
        if let Some(body) = self.body.as_deref() {
            bounded_body(
                body,
                usize::try_from(RESULT_BYTES_MAX).expect("result limit fits usize"),
                "record.peerResult.body",
            )?;
            invalid_if!(
                u64::try_from(body.len()).unwrap_or(u64::MAX) != self.bytes
                    || sha256(body) != self.digest,
                "record.peerResult.body",
                "does not match its digest and byte count"
            );
        }
        invalid_if!(
            self.state == CollaborationPeerResultState::Ready
                && (self.queue_entry_id.is_some() || self.acceptance.is_some()),
            "record.peerResult",
            "ready results cannot have queue or acceptance receipts"
        );
        invalid_if!(
            self.state != CollaborationPeerResultState::Ready && self.queue_entry_id.is_none(),
            "record.peerResult.queueEntryId",
            "is required after held-queue insertion"
        );
        match self.state {
            CollaborationPeerResultState::Ready | CollaborationPeerResultState::Held => {
                invalid_if!(
                    self.acceptance.is_some(),
                    "record.peerResult.acceptance",
                    "is forbidden before peer disposition"
                );
            }
            CollaborationPeerResultState::Accepted
            | CollaborationPeerResultState::DeliveryPending
            | CollaborationPeerResultState::Delivered
            | CollaborationPeerResultState::Unknown => {
                invalid_if!(
                    self.acceptance.as_ref().map(|value| value.decision)
                        != Some(CollaborationPeerDecision::Accepted),
                    "record.peerResult.acceptance",
                    "must record acceptance for delivery"
                );
            }
            CollaborationPeerResultState::Discarded => {
                invalid_if!(
                    self.acceptance.as_ref().map(|value| value.decision)
                        != Some(CollaborationPeerDecision::Discarded),
                    "record.peerResult.acceptance",
                    "must record discard"
                );
            }
        }
        Ok(())
    }
}

impl CollaborationWorktreeV1 {
    fn validate(&self, collaboration_id: &str) -> ContractResult<()> {
        match self {
            Self::Unmanaged(worktree) => {
                invalid_if!(
                    worktree.version != 1 || worktree.managed,
                    "record.worktree",
                    "unmanaged worktree must have version 1 and managed false"
                );
            }
            Self::Managed(worktree) => {
                invalid_if!(
                    worktree.version != 1 || !worktree.managed,
                    "record.worktree",
                    "managed worktree must have version 1 and managed true"
                );
                bounded_path(&worktree.canonical_path, "record.worktree.canonicalPath")?;
                bounded_safe_line(&worktree.branch, PATH_BYTES_MAX, "record.worktree.branch")?;
                safe_integer(worktree.owner_revision, "record.worktree.ownerRevision")?;
                if let Some(transfer_id) = worktree.transfer_id.as_deref() {
                    bounded_identifier(transfer_id, "record.worktree.transferId")?;
                }
                match &worktree.owner {
                    CollaborationWorktreeOwnerV1::Source { session_key }
                    | CollaborationWorktreeOwnerV1::Target { session_key } => {
                        assert_normalized_opaque(
                            session_key,
                            SOURCE_SESSION_KEY_BYTES_MAX,
                            1,
                            "record.worktree.owner.sessionKey",
                        )?;
                    }
                    CollaborationWorktreeOwnerV1::Escrow {
                        collaboration_id: owner_id,
                    } => {
                        bounded_identifier(owner_id, "record.worktree.owner.collaborationId")?;
                        invalid_if!(
                            owner_id != collaboration_id,
                            "record.worktree.owner.collaborationId",
                            "does not match record"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn managed(&self) -> Option<&ManagedWorktreeV1> {
        match self {
            Self::Managed(value) => Some(value),
            Self::Unmanaged(_) => None,
        }
    }
}

impl CollaborationFailureV1 {
    fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "record.failure.version",
            "must be the supported contract version"
        );
        timestamp_millis(&self.at, "record.failure.at")?;
        invalid_if!(
            self.ambiguous != self.code.is_ambiguous(),
            "record.failure.ambiguous",
            "does not match the error-code class"
        );
        invalid_if!(
            self.ambiguous && self.retryable,
            "record.failure.retryable",
            "ambiguous outcomes are never retryable"
        );
        Ok(())
    }
}

impl CollaborationRecordV1 {
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "record.version",
            "must be the supported contract version"
        );
        bounded_identifier(&self.collaboration_id, "record.collaborationId")?;
        safe_integer(self.revision, "record.revision")?;
        self.intent.validate()?;
        hash_value(&self.dedupe_key, "record.dedupeKey")?;
        hash_value(&self.payload_hash, "record.payloadHash")?;
        invalid_if!(
            self.dedupe_key != dedupe_key_for_validated_intent(&self.intent),
            "record.dedupeKey",
            "does not match intent"
        );
        invalid_if!(
            self.payload_hash != payload_hash_for_validated_intent(&self.intent),
            "record.payloadHash",
            "does not match intent"
        );
        let created = timestamp_millis(&self.created_at, "record.createdAt")?;
        let updated = timestamp_millis(&self.updated_at, "record.updatedAt")?;
        invalid_if!(updated < created, "record.updatedAt", "precedes creation");
        self.source.validate()?;
        self.target.validate()?;
        invalid_if!(
            self.source.session_key != self.intent.source_session_key
                || self.source.engine != engine_from_session_key(&self.intent.source_session_key)?,
            "record.source",
            "does not match intent source"
        );
        invalid_if!(
            self.target.engine != self.intent.target_engine
                || self.source.engine == self.target.engine,
            "record.target.engine",
            "does not match intent or differs from source"
        );
        if let Some(packet) = self.packet.as_ref() {
            packet.validate()?;
        }
        if let Some(approval) = self.approval.as_ref() {
            approval.validate()?;
        }
        self.effects.validate()?;
        if let Some(peer_result) = self.peer_result.as_ref() {
            peer_result.validate(&self.collaboration_id)?;
        }
        if let Some(worktree) = self.worktree.as_ref() {
            worktree.validate(&self.collaboration_id)?;
        }
        self.limits.validate(self.intent.mode)?;
        if let Some(failure) = self.failure.as_ref() {
            failure.validate()?;
        }
        let terminal = self
            .terminal_at
            .as_deref()
            .map(|at| timestamp_millis(at, "record.terminalAt"))
            .transpose()?;
        invalid_if!(
            terminal.is_some_and(|at| at < created),
            "record.terminalAt",
            "precedes creation"
        );

        invalid_if!(
            !self
                .effects
                .iter()
                .all(|effect| allowed_effect(self.intent.mode, effect.kind)),
            "record.effects",
            "contains an effect not valid for collaboration mode"
        );
        invalid_if!(
            self.intent.objective_characters > self.limits.max_objective_characters
                || self.intent.objective_bytes > self.limits.max_objective_bytes,
            "record.intent",
            "exceeds the applied objective policy"
        );
        invalid_if!(
            self.packet.as_ref().is_some_and(|packet| {
                packet.bytes > self.limits.max_packet_bytes
                    || packet.context_bytes > self.limits.max_context_bytes
            }),
            "record.packet",
            "exceeds the applied packet policy"
        );
        invalid_if!(
            self.peer_result
                .as_ref()
                .is_some_and(|result| result.bytes > self.limits.max_result_bytes),
            "record.peerResult.bytes",
            "exceeds the applied result policy"
        );

        invalid_if!(
            self.state.is_terminal() != self.terminal_at.is_some(),
            "record.terminalAt",
            "must exist exactly for a terminal state"
        );
        invalid_if!(
            !self.state.is_terminal() && self.intent.objective.is_none(),
            "record.intent.objective",
            "is required until terminal cleanup"
        );
        invalid_if!(
            self.state.is_terminal()
                && (self.intent.objective.is_some()
                    || self
                        .packet
                        .as_ref()
                        .is_some_and(|packet| packet.body.is_some())
                    || self
                        .peer_result
                        .as_ref()
                        .is_some_and(|result| result.body.is_some())),
            "record",
            "terminal records cannot retain transient objective, packet, or result text"
        );
        invalid_if!(
            self.terminal_at.is_some() && self.terminal_at != Some(self.updated_at.clone()),
            "record.terminalAt",
            "must equal the terminal transition timestamp"
        );

        if let Some(failure) = self.failure.as_ref() {
            invalid_if!(
                !matches!(
                    self.state,
                    CollaborationState::Failed
                        | CollaborationState::Denied
                        | CollaborationState::Unknown
                        | CollaborationState::Cancelled
                ),
                "record.failure",
                "is not valid for this state"
            );
            let failure_at = timestamp_millis(&failure.at, "record.failure.at")?;
            invalid_if!(
                failure_at < created || failure_at > updated,
                "record.failure.at",
                "falls outside the record lifetime"
            );
        }
        invalid_if!(
            matches!(
                self.state,
                CollaborationState::Failed
                    | CollaborationState::Denied
                    | CollaborationState::Unknown
            ) && self.failure.is_none(),
            "record.failure",
            "is required for this state"
        );
        invalid_if!(
            self.state == CollaborationState::Denied
                && self.failure.as_ref().map(|failure| failure.code)
                    != Some(CollaborationErrorCode::Denied),
            "record.failure.code",
            "must be DENIED for a denied record"
        );
        invalid_if!(
            self.state == CollaborationState::Unknown
                && !self
                    .failure
                    .as_ref()
                    .is_some_and(|failure| failure.ambiguous),
            "record.failure",
            "must describe an ambiguous outcome"
        );
        invalid_if!(
            self.state == CollaborationState::Failed
                && self
                    .failure
                    .as_ref()
                    .is_none_or(|failure| failure.ambiguous),
            "record.failure",
            "known failure cannot be ambiguous"
        );
        invalid_if!(
            self.state == CollaborationState::Cancelled
                && self.failure.as_ref().is_some_and(|failure| {
                    !matches!(
                        failure.code,
                        CollaborationErrorCode::ApprovalExpired
                            | CollaborationErrorCode::ApprovalStale
                    )
                }),
            "record.failure.code",
            "is not a cancellation reason"
        );

        if self.state == CollaborationState::Requested {
            invalid_if!(
                self.packet.is_some()
                    || self.approval.is_some()
                    || self.peer_result.is_some()
                    || !self.effects.is_empty()
                    || self.failure.is_some(),
                "record",
                "requested state cannot contain post-request facts"
            );
            return Ok(());
        }

        let may_terminate_before_preview = matches!(
            self.state,
            CollaborationState::Cancelled | CollaborationState::Failed
        ) && self.packet.is_none()
            && self.approval.is_none()
            && self.effects.is_empty();
        invalid_if!(
            !may_terminate_before_preview && (self.packet.is_none() || self.approval.is_none()),
            "record.packet",
            "packet and approval are required after request validation"
        );
        invalid_if!(
            self.packet.is_some() != self.approval.is_some(),
            "record.approval",
            "must be present exactly when a packet is present"
        );

        if let (Some(approval), Some(packet)) = (&self.approval, &self.packet) {
            invalid_if!(
                approval.collaboration_id != self.collaboration_id
                    || approval.packet_hash != packet.hash
                    || approval.target_engine != self.target.engine
                    || self.source.registry_revision != Some(approval.source_registry_revision)
                    || approval.source_guard_hash != source_guard_hash_for_approval(&self.source)?,
                "record.approval",
                "does not match the collaboration packet, target, or source guards"
            );
            let approval_created =
                timestamp_millis(&approval.created_at, "record.approval.createdAt")?;
            let expires = timestamp_millis(&approval.expires_at, "record.approval.expiresAt")?;
            let decided = approval
                .decided_at
                .as_deref()
                .map(|at| timestamp_millis(at, "record.approval.decidedAt"))
                .transpose()?;
            invalid_if!(
                approval_created < created || decided.is_some_and(|at| at > updated),
                "record.approval",
                "falls outside the current record lifetime"
            );
            invalid_if!(
                matches!(
                    approval.decision,
                    CollaborationApprovalDecision::Approved | CollaborationApprovalDecision::Denied
                ) && decided.is_some_and(|at| at > expires),
                "record.approval.decidedAt",
                "decision occurred after expiry"
            );
            invalid_if!(
                approval.decision == CollaborationApprovalDecision::Expired
                    && decided.is_some_and(|at| at < expires),
                "record.approval.decidedAt",
                "expiry decision precedes expiry"
            );
            let lifetime = expires
                .signed_duration_since(approval_created)
                .num_milliseconds();
            invalid_if!(
                u64::try_from(lifetime).unwrap_or(u64::MAX) > self.limits.approval_timeout_ms,
                "record.approval.expiresAt",
                "exceeds the applied approval timeout"
            );

            invalid_if!(
                self.state == CollaborationState::AwaitingApproval
                    && approval.decision != CollaborationApprovalDecision::Pending,
                "record.approval.decision",
                "must be pending while awaiting approval"
            );
            invalid_if!(
                self.state == CollaborationState::Denied
                    && approval.decision != CollaborationApprovalDecision::Denied,
                "record.approval.decision",
                "must record denial"
            );
            let approved_state = matches!(
                self.state,
                CollaborationState::Approved
                    | CollaborationState::Preflight
                    | CollaborationState::PreparingTarget
                    | CollaborationState::RunningTarget
                    | CollaborationState::ResultReady
                    | CollaborationState::AwaitingPeerReview
                    | CollaborationState::DeliveryPending
                    | CollaborationState::ParkingSource
                    | CollaborationState::OwnershipTransfer
                    | CollaborationState::ActivatingTarget
                    | CollaborationState::Completed
                    | CollaborationState::Unknown
            );
            invalid_if!(
                approved_state && approval.decision != CollaborationApprovalDecision::Approved,
                "record.approval.decision",
                "must record approval before effects"
            );
            invalid_if!(
                approved_state
                    && self.intent.mode == CollaborationMode::Handoff
                    && self.source.live_status != Some(CollaborationLiveStatus::Idle),
                "record.source.liveStatus",
                "must be idle for handoff approval consumption"
            );
        }

        if !may_terminate_before_preview {
            self.source.require_approval_bound()?;
        }
        self.validate_worktree_relationships()?;
        self.validate_peer_relationships(updated)?;
        self.validate_effect_timestamps(created, updated)?;
        validate_effect_dependencies(self)?;
        self.validate_state_boundary()?;
        Ok(())
    }

    fn validate_worktree_relationships(&self) -> ContractResult<()> {
        let Some(worktree) = self
            .worktree
            .as_ref()
            .and_then(CollaborationWorktreeV1::managed)
        else {
            return Ok(());
        };
        let Some(source_owner_revision) = self.source.worktree_owner_revision else {
            return Err(ContractError::invalid(
                "record.source.worktreeOwnerRevision",
                "is required for a managed worktree",
            ));
        };
        invalid_if!(
            worktree.owner_revision < source_owner_revision,
            "record.worktree.ownerRevision",
            "precedes the approval-bound owner revision"
        );
        match &worktree.owner {
            CollaborationWorktreeOwnerV1::Source { session_key }
                if session_key != &self.source.session_key =>
            {
                return Err(ContractError::invalid(
                    "record.worktree.owner.sessionKey",
                    "does not match the source session",
                ));
            }
            CollaborationWorktreeOwnerV1::Target { session_key }
                if self.target.session_key.as_ref() != Some(session_key) =>
            {
                return Err(ContractError::invalid(
                    "record.worktree.owner.sessionKey",
                    "does not match the target session",
                ));
            }
            CollaborationWorktreeOwnerV1::Source { .. } => {}
            _ if worktree.transfer_id.is_none() => {
                return Err(ContractError::invalid(
                    "record.worktree.transferId",
                    "is required after ownership leaves the source",
                ));
            }
            _ => {}
        }
        let before_transfer = self.intent.mode == CollaborationMode::Consult
            || matches!(
                self.state,
                CollaborationState::AwaitingApproval
                    | CollaborationState::Approved
                    | CollaborationState::Preflight
                    | CollaborationState::PreparingTarget
                    | CollaborationState::ParkingSource
            );
        invalid_if!(
            before_transfer
                && !matches!(worktree.owner, CollaborationWorktreeOwnerV1::Source { .. }),
            "record.worktree.owner",
            "must remain source-owned before atomic transfer"
        );
        Ok(())
    }

    fn validate_peer_relationships(&self, updated: DateTime<Utc>) -> ContractResult<()> {
        let Some(peer) = self.peer_result.as_ref() else {
            return Ok(());
        };
        invalid_if!(
            self.intent.mode != CollaborationMode::Consult,
            "record.peerResult",
            "is consult-only"
        );
        invalid_if!(
            self.target.session_key.as_deref() != Some(peer.target_session_key.as_str()),
            "record.peerResult.targetSessionKey",
            "does not match the target session"
        );
        let created = timestamp_millis(&peer.created_at, "record.peerResult.createdAt")?;
        invalid_if!(
            created > updated,
            "record.peerResult.createdAt",
            "is later than the record revision"
        );
        if let Some(acceptance) = peer.acceptance.as_ref()
            && timestamp_millis(
                &acceptance.decided_at,
                "record.peerResult.acceptance.decidedAt",
            )? < created
        {
            return Err(ContractError::invalid(
                "record.peerResult.acceptance.decidedAt",
                "precedes result creation",
            ));
        }
        Ok(())
    }

    fn validate_effect_timestamps(
        &self,
        created: DateTime<Utc>,
        updated: DateTime<Utc>,
    ) -> ContractResult<()> {
        let timeout = match self.intent.mode {
            CollaborationMode::Consult => self.limits.consultation_timeout_ms.unwrap_or(0),
            CollaborationMode::Handoff => self.limits.handoff_prepare_timeout_ms.unwrap_or(0),
        };
        for effect in self.effects.iter() {
            let effect_created = timestamp_millis(
                &effect.created_at,
                format_args!("record.effects.{}.createdAt", effect.kind.as_str()),
            )?;
            invalid_if!(
                effect_created < created || effect_created > updated,
                format_args!("record.effects.{}", effect.kind.as_str()),
                "contains a timestamp outside the record revision"
            );
            for (at, path) in [
                (effect.started_at.as_deref(), "startedAt"),
                (effect.settled_at.as_deref(), "settledAt"),
            ] {
                if let Some(at) = at {
                    let parsed = timestamp_millis(
                        at,
                        format_args!("record.effects.{}.{}", effect.kind.as_str(), path),
                    )?;
                    invalid_if!(
                        parsed < created || parsed > updated,
                        format_args!("record.effects.{}", effect.kind.as_str()),
                        "contains a timestamp outside the record revision"
                    );
                }
            }
            let deadline = timestamp_millis(&effect.deadline_at, "effect.deadlineAt")?;
            let duration = deadline
                .signed_duration_since(effect_created)
                .num_milliseconds();
            invalid_if!(
                u64::try_from(duration).unwrap_or(u64::MAX) > timeout,
                format_args!("record.effects.{}.deadlineAt", effect.kind.as_str()),
                "exceeds the applied provider timeout"
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn validate_state_boundary(&self) -> ContractResult<()> {
        use CollaborationEffectKind as K;
        use CollaborationEffectStatus as E;
        use CollaborationPeerResultState as P;
        use CollaborationState as S;
        match self.state {
            S::AwaitingApproval | S::Approved | S::Preflight => {
                invalid_if!(
                    !self.effects.is_empty()
                        || self.peer_result.is_some()
                        || self.target.session_key.is_some(),
                    "record",
                    "contains provider facts before target preparation"
                );
            }
            S::PreparingTarget => {
                require_only_effects(&self.effects, &[K::PrepareTarget])?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Reserved, E::Started])?;
            }
            S::RunningTarget => {
                require_only_effects(&self.effects, &[K::PrepareTarget, K::RunTarget])?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::RunTarget, &[E::Reserved, E::Started])?;
                invalid_if!(
                    self.peer_result.is_some(),
                    "record.peerResult",
                    "cannot precede result readiness"
                );
            }
            S::ResultReady => {
                require_only_effects(&self.effects, &[K::PrepareTarget, K::RunTarget])?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::RunTarget, &[E::Succeeded])?;
                invalid_if!(
                    self.peer_result.as_ref().map(|result| result.state) != Some(P::Ready),
                    "record.peerResult.state",
                    "must be ready"
                );
            }
            S::AwaitingPeerReview => {
                require_only_effects(&self.effects, &[K::PrepareTarget, K::RunTarget])?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::RunTarget, &[E::Succeeded])?;
                invalid_if!(
                    self.peer_result.as_ref().map(|result| result.state) != Some(P::Held),
                    "record.peerResult.state",
                    "must be held"
                );
            }
            S::DeliveryPending => {
                require_only_effects(
                    &self.effects,
                    &[K::PrepareTarget, K::RunTarget, K::DeliverResult],
                )?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::RunTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::DeliverResult, &[E::Reserved, E::Started])?;
                invalid_if!(
                    self.peer_result.as_ref().map(|result| result.state)
                        != Some(P::DeliveryPending),
                    "record.peerResult.state",
                    "must be delivery-pending"
                );
            }
            S::ParkingSource => {
                require_only_effects(&self.effects, &[K::PrepareTarget, K::ParkSource])?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::ParkSource, &[E::Reserved, E::Started])?;
            }
            S::OwnershipTransfer => {
                require_only_effects(
                    &self.effects,
                    &[K::PrepareTarget, K::ParkSource, K::TransferOwnership],
                )?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::ParkSource, &[E::Succeeded])?;
                require_effect_status(
                    &self.effects,
                    K::TransferOwnership,
                    &[E::Reserved, E::Started],
                )?;
                invalid_if!(
                    self.worktree
                        .as_ref()
                        .and_then(CollaborationWorktreeV1::managed)
                        .is_some_and(|worktree| {
                            !matches!(worktree.owner, CollaborationWorktreeOwnerV1::Escrow { .. })
                        }),
                    "record.worktree.owner",
                    "must be held by transaction escrow"
                );
            }
            S::ActivatingTarget => {
                require_only_effects(
                    &self.effects,
                    &[
                        K::PrepareTarget,
                        K::ParkSource,
                        K::TransferOwnership,
                        K::ActivateTarget,
                    ],
                )?;
                self.target.require_session()?;
                require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::ParkSource, &[E::Succeeded])?;
                require_effect_status(&self.effects, K::TransferOwnership, &[E::Succeeded])?;
                require_effect_status(
                    &self.effects,
                    K::ActivateTarget,
                    &[E::Reserved, E::Started],
                )?;
                invalid_if!(
                    self.worktree
                        .as_ref()
                        .and_then(CollaborationWorktreeV1::managed)
                        .is_some_and(|worktree| {
                            !matches!(
                                worktree.owner,
                                CollaborationWorktreeOwnerV1::Target { .. }
                                    | CollaborationWorktreeOwnerV1::Escrow { .. }
                            )
                        }),
                    "record.worktree.owner",
                    "must be committed to target or escrow"
                );
            }
            S::Unknown => {
                invalid_if!(
                    !self
                        .effects
                        .iter()
                        .any(|effect| effect.status == E::Unknown)
                        && self.failure.as_ref().map(|failure| failure.code)
                            != Some(CollaborationErrorCode::RecoveryRequired),
                    "record.effects",
                    "unknown state requires an ambiguous effect or recovery marker"
                );
            }
            S::Completed => self.validate_completed_record()?,
            S::Requested | S::Denied | S::Cancelled | S::Failed => {}
        }
        invalid_if!(
            matches!(self.state, S::Completed | S::Failed)
                && self
                    .effects
                    .iter()
                    .any(|effect| effect.status == E::Unknown)
                && self.target.reconciliation_marker.is_none(),
            "record.target.reconciliationMarker",
            "is required when reconciling an ambiguous provider effect"
        );
        Ok(())
    }

    fn validate_completed_record(&self) -> ContractResult<()> {
        use CollaborationEffectKind as K;
        use CollaborationEffectStatus as E;
        use CollaborationPeerResultState as P;
        if self.intent.mode == CollaborationMode::Consult {
            let result = self.peer_result.as_ref().ok_or_else(|| {
                ContractError::invalid("record.peerResult", "is required for completed consult")
            })?;
            match result.state {
                P::Discarded => {
                    require_only_effects(&self.effects, &[K::PrepareTarget, K::RunTarget])?;
                    require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                    require_effect_status(&self.effects, K::RunTarget, &[E::Succeeded])?;
                    invalid_if!(
                        self.effects.deliver_result.is_some(),
                        "record.effects.deliver-result",
                        "is forbidden for a discarded result"
                    );
                    return Ok(());
                }
                P::Delivered => {
                    require_only_effects(
                        &self.effects,
                        &[K::PrepareTarget, K::RunTarget, K::DeliverResult],
                    )?;
                    require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
                    require_effect_status(&self.effects, K::RunTarget, &[E::Succeeded])?;
                    require_effect_status(&self.effects, K::DeliverResult, &[E::Succeeded])?;
                    return Ok(());
                }
                P::Unknown if self.target.reconciliation_marker.is_some() => {
                    require_only_effects(
                        &self.effects,
                        &[K::PrepareTarget, K::RunTarget, K::DeliverResult],
                    )?;
                    return Ok(());
                }
                _ => {
                    return Err(ContractError::invalid(
                        "record.peerResult.state",
                        "is not terminal for a completed consultation",
                    ));
                }
            }
        }
        self.target.require_session()?;
        require_only_effects(
            &self.effects,
            &[
                K::PrepareTarget,
                K::ParkSource,
                K::TransferOwnership,
                K::ActivateTarget,
            ],
        )?;
        require_effect_status(&self.effects, K::PrepareTarget, &[E::Succeeded])?;
        require_effect_status(&self.effects, K::ParkSource, &[E::Succeeded])?;
        require_effect_status(&self.effects, K::TransferOwnership, &[E::Succeeded])?;
        let activation = self.effects.activate_target.as_ref();
        invalid_if!(
            !activation.is_some_and(|effect| {
                effect.status == E::Succeeded
                    || (effect.status == E::Unknown && self.target.reconciliation_marker.is_some())
            }),
            "record.effects.activate-target",
            "must be successful or explicitly reconciled"
        );
        invalid_if!(
            self.worktree
                .as_ref()
                .and_then(CollaborationWorktreeV1::managed)
                .is_some_and(|worktree| {
                    !matches!(worktree.owner, CollaborationWorktreeOwnerV1::Target { .. })
                }),
            "record.worktree.owner",
            "must belong to the target after handoff completion"
        );
        Ok(())
    }
}
