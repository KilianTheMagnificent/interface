//! Durable provider-neutral collaboration coordinator.

use chrono::{Duration, Utc};
use interface_contracts::canonical_timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::audit::{AuditError, CollaborationAuditSink};
use crate::broker::CollaborationModelAuthority;
use crate::clip;
use crate::journal::{CollaborationJournal, JournalError, JournalErrorCode};
use crate::state::{
    CollaborationApprovalDecision, CollaborationApprovalRecordV1, CollaborationEffectKind,
    CollaborationEffectRecordV1, CollaborationEffectStatus, CollaborationEngineId,
    CollaborationErrorCode, CollaborationFailureV1, CollaborationIntentInput,
    CollaborationLiveStatus, CollaborationMode, CollaborationPacketRecordV1,
    CollaborationPeerAcceptanceV1, CollaborationPeerDecision, CollaborationPeerResultState,
    CollaborationPeerResultV1, CollaborationPolicyOverrides, CollaborationReceiptV1,
    CollaborationRecordV1, CollaborationSourceV1, CollaborationState, CollaborationTargetV1,
    CollaborationTransitionPatch, CollaborationTransitionRequest, CollaborationWorktreeV1,
    ContractError, CreateCollaborationRecordOptions, HANDOFF_PREPARE_TIMEOUT_MS,
    JS_MAX_SAFE_INTEGER, UnmanagedWorktreeV1, cancellation_is_safe, create_collaboration_record,
    dedupe_key_for_intent, effect_successor, fingerprint_origin_lineage,
    normalize_collaboration_intent, payload_hash_for_intent, peer_result_id,
    receipt_for_validated_collaboration, sha256, source_guard_hash_for_approval, timestamp_millis,
    transition_collaboration_record,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    pub engine: CollaborationEngineId,
    pub session_key: String,
    pub provider_session_id: String,
    pub title: Option<String>,
    pub registry_revision: u64,
    pub runtime_epoch: String,
    pub live_handle_generation: u64,
    pub live_status: CollaborationLiveStatus,
    pub canonical_cwd: String,
    pub canonical_project_root: String,
    pub pending_approval_set_hash: String,
    pub pending_approval_count: u64,
    pub queue_revision: u64,
    pub queue_depth: u64,
    pub delivery_revision: u64,
    pub delivery_unknown: bool,
    pub worktree: Option<CollaborationWorktreeV1>,
}

impl SourceSnapshot {
    pub fn to_source(&self) -> Result<CollaborationSourceV1, CoordinatorError> {
        if self.pending_approval_count > JS_MAX_SAFE_INTEGER
            || self.queue_depth > JS_MAX_SAFE_INTEGER
        {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::StateCorrupt,
            ));
        }
        let worktree_owner_revision = self
            .worktree
            .as_ref()
            .and_then(CollaborationWorktreeV1::managed)
            .map(|worktree| worktree.owner_revision);
        let source = CollaborationSourceV1 {
            engine: self.engine,
            session_key: self.session_key.clone(),
            provider_session_id: Some(self.provider_session_id.clone()),
            title: self.title.clone(),
            registry_revision: Some(self.registry_revision),
            runtime_epoch: Some(self.runtime_epoch.clone()),
            live_handle_generation: Some(self.live_handle_generation),
            live_status: Some(self.live_status),
            canonical_cwd: Some(self.canonical_cwd.clone()),
            canonical_project_root: Some(self.canonical_project_root.clone()),
            pending_approval_set_hash: Some(self.pending_approval_set_hash.clone()),
            queue_revision: Some(self.queue_revision),
            delivery_revision: Some(self.delivery_revision),
            delivery_unknown: Some(self.delivery_unknown),
            worktree_owner_revision,
        };
        source.require_approval_bound()?;
        Ok(source)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketBuildResult {
    pub body: String,
    pub context_bytes: u64,
    pub omissions: u64,
}

impl PacketBuildResult {
    fn to_packet(&self) -> Result<CollaborationPacketRecordV1, CoordinatorError> {
        let packet = CollaborationPacketRecordV1 {
            version: 2,
            hash: sha256(&self.body),
            bytes: u64::try_from(self.body.len()).unwrap_or(u64::MAX),
            context_bytes: self.context_bytes,
            omissions: self.omissions,
            body: Some(self.body.clone()),
        };
        packet.validate()?;
        Ok(packet)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalAction {
    pub collaboration_id: String,
    pub approval_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerAction {
    pub collaboration_id: String,
    pub result_id: String,
    pub result_digest: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPreview {
    pub collaboration_id: String,
    pub approval_id: String,
    pub expected_revision: u64,
    pub expires_at: String,
    pub mode: CollaborationMode,
    pub source_engine: CollaborationEngineId,
    pub source_session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_title: Option<String>,
    pub canonical_cwd: String,
    pub canonical_project_root: String,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
    pub packet_hash: String,
    pub packet_preview: String,
    pub packet_bytes: u64,
    pub context_bytes: u64,
    pub omissions: u64,
    pub managed_worktree: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingPeerState {
    AwaitingReview,
    DeliveryPending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingPeer {
    pub state: PendingPeerState,
    pub collaboration_id: String,
    pub expected_revision: u64,
    pub queue_entry_id: String,
    pub result_id: String,
    pub result_digest: String,
    pub result_bytes: u64,
    pub source_session_key: String,
    pub source_engine: CollaborationEngineId,
    pub target_session_key: String,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockedSessionRole {
    Source,
    Target,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedSession {
    pub collaboration_id: String,
    pub mode: CollaborationMode,
    pub state: CollaborationState,
    pub session_key: String,
    pub role: LockedSessionRole,
}

/// Renderer-safe participant identities. Provider handles and broker
/// capabilities never cross this projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionParticipants {
    pub source_session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_key: Option<String>,
}

pub type CollaborationApprovalAction = ApprovalAction;
pub type CollaborationPeerAction = PeerAction;

/// Host-only material needed to resolve the exact source and build the packet
/// after the durable collaboration ID has been allocated.  Keeping this as a
/// separate projection closes the legacy crash window where provider/filesystem
/// reads happened before an operation had a durable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanPreviewMaterial {
    pub collaboration_id: String,
    pub mode: CollaborationMode,
    pub source_session_key: String,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedTarget {
    pub session_key: String,
    pub provider_session_id: String,
    pub reconciliation_marker: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectSuccess {
    PreparedTarget(PreparedTarget),
    ConsultResult(String),
    Unit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectOutcome {
    Succeeded {
        value: EffectSuccess,
        provider_effect_id: Option<String>,
    },
    ProvenUnsent {
        code: CollaborationErrorCode,
    },
    Ambiguous {
        code: Option<CollaborationErrorCode>,
        provider_effect_id: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCommand {
    PrepareTarget {
        collaboration_id: String,
        effect_id: String,
        mode: CollaborationMode,
        target_collaboration_depth: u8,
        source: CollaborationSourceV1,
        target_engine: CollaborationEngineId,
    },
    RunTarget {
        collaboration_id: String,
        effect_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        packet_body: String,
        objective: String,
    },
    ParkSource {
        collaboration_id: String,
        effect_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        expected_source_guard_hash: String,
    },
    TransferOwnership {
        collaboration_id: String,
        effect_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
    },
    ActivateTarget {
        collaboration_id: String,
        effect_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        packet_body: String,
        objective: String,
    },
    DeliverResult {
        collaboration_id: String,
        effect_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        queue_entry_id: String,
        result_id: String,
        result_digest: String,
        body: String,
        expected_source_guard_hash: String,
    },
}

impl EffectCommand {
    #[must_use]
    pub const fn kind(&self) -> CollaborationEffectKind {
        match self {
            Self::PrepareTarget { .. } => CollaborationEffectKind::PrepareTarget,
            Self::RunTarget { .. } => CollaborationEffectKind::RunTarget,
            Self::ParkSource { .. } => CollaborationEffectKind::ParkSource,
            Self::TransferOwnership { .. } => CollaborationEffectKind::TransferOwnership,
            Self::ActivateTarget { .. } => CollaborationEffectKind::ActivateTarget,
            Self::DeliverResult { .. } => CollaborationEffectKind::DeliverResult,
        }
    }

    #[must_use]
    pub fn effect_id(&self) -> &str {
        match self {
            Self::PrepareTarget { effect_id, .. }
            | Self::RunTarget { effect_id, .. }
            | Self::ParkSource { effect_id, .. }
            | Self::TransferOwnership { effect_id, .. }
            | Self::ActivateTarget { effect_id, .. }
            | Self::DeliverResult { effect_id, .. } => effect_id,
        }
    }
}

#[derive(Debug, Error)]
pub enum CoordinatorError {
    #[error("collaboration request refused: {code:?}")]
    Ingress {
        code: CollaborationErrorCode,
        collaboration_id: Option<String>,
        retryable: bool,
    },
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

impl CoordinatorError {
    fn ingress(code: CollaborationErrorCode) -> Self {
        let retryable = !code.is_ambiguous();
        Self::Ingress {
            code,
            collaboration_id: None,
            retryable,
        }
    }

    fn for_record(code: CollaborationErrorCode, record: &CollaborationRecordV1) -> Self {
        let retryable = !code.is_ambiguous();
        Self::Ingress {
            code,
            collaboration_id: Some(record.collaboration_id.clone()),
            retryable,
        }
    }
}

/// A record that is missing a field its state requires.
fn corrupt(record: &CollaborationRecordV1) -> CoordinatorError {
    CoordinatorError::for_record(CollaborationErrorCode::StateCorrupt, record)
}

type Now = Box<dyn FnMut() -> String + Send>;
type NewId = Box<dyn FnMut(&str) -> String + Send>;

/// Provider-neutral durable coordinator. It returns effect commands only after
/// the corresponding `started` attempt is durably committed.
pub struct CollaborationCoordinator {
    journal: CollaborationJournal,
    audit: Option<CollaborationAuditSink>,
    limits: CollaborationPolicyOverrides,
    now: Now,
    new_id: NewId,
    accepting: bool,
    recovered: bool,
    quiesced: bool,
}

impl CollaborationCoordinator {
    #[must_use]
    pub fn new(journal: CollaborationJournal) -> Self {
        Self::with_generators(
            journal,
            None,
            CollaborationPolicyOverrides::default(),
            || canonical_timestamp(Utc::now()),
            |kind| format!("{kind}-{}", Uuid::new_v4()),
        )
    }

    pub fn with_generators(
        journal: CollaborationJournal,
        audit: Option<CollaborationAuditSink>,
        limits: CollaborationPolicyOverrides,
        now: impl FnMut() -> String + Send + 'static,
        new_id: impl FnMut(&str) -> String + Send + 'static,
    ) -> Self {
        Self {
            journal,
            audit,
            limits,
            now: Box::new(now),
            new_id: Box::new(new_id),
            accepting: false,
            recovered: false,
            quiesced: false,
        }
    }

    #[must_use]
    pub const fn accepting(&self) -> bool {
        self.accepting
    }

    /// Rebuilds active state before admission. A durable `started` effect is
    /// never replayed: without a provider-specific read-only proof it becomes
    /// an unknown, non-retryable outcome.
    pub fn recover(&mut self) -> Result<(), CoordinatorError> {
        if self.recovered {
            return Ok(());
        }
        if self.quiesced {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::BrokerUnavailable,
            ));
        }
        self.accepting = false;
        let records = self
            .journal
            .records()?
            .filter(|record| {
                !record.state.is_terminal() && record.state != CollaborationState::Unknown
            })
            .cloned()
            .collect::<Vec<_>>();
        for record in records {
            if record.state == CollaborationState::AwaitingApproval {
                let now = self.now_at_least(&record.updated_at)?;
                let expires = timestamp_millis(
                    &record
                        .approval
                        .as_ref()
                        .ok_or_else(|| corrupt(&record))?
                        .expires_at,
                    "record.approval.expiresAt",
                )?;
                if timestamp_millis(&now, "coordinator.now")? >= expires {
                    self.expire_record(record, now)?;
                }
                continue;
            }
            let started = record
                .effects
                .iter()
                .find(|effect| effect.status == CollaborationEffectStatus::Started)
                .cloned();
            if let Some(started) = started {
                self.quarantine_started(record, started)?;
            }
        }
        self.recovered = true;
        self.accepting = true;
        Ok(())
    }

    pub fn submit_human(
        &mut self,
        input: CollaborationIntentInput,
        source_snapshot: &SourceSnapshot,
        packet_build: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let started = self.begin_human(input)?;
        if started.state != CollaborationState::Requested {
            return Ok(started);
        }
        self.complete_human_preview(&started.collaboration_id, source_snapshot, packet_build)
    }

    /// Durably admits one human request and allocates its exact operation ID
    /// before any source snapshot or transcript packet is read.
    pub fn begin_human(
        &mut self,
        mut input: CollaborationIntentInput,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        input.origin = crate::state::CollaborationOrigin::Human;
        self.begin_preview(input, None, 0)
    }

    /// Returns the host-only packet/source material for a durable request.
    pub fn human_preview_material(
        &self,
        collaboration_id: &str,
    ) -> Result<HumanPreviewMaterial, CoordinatorError> {
        let record = self.record_or_throw(collaboration_id)?;
        let objective = record
            .intent
            .objective
            .clone()
            .ok_or_else(|| corrupt(&record))?;
        Ok(HumanPreviewMaterial {
            collaboration_id: record.collaboration_id.clone(),
            mode: record.intent.mode,
            source_session_key: record.intent.source_session_key.clone(),
            target_engine: record.target.engine,
            objective,
        })
    }

    /// Completes the preview for an already durable human request. Replaying
    /// after the record advanced is read-only and returns its current receipt.
    pub fn complete_human_preview(
        &mut self,
        collaboration_id: &str,
        source_snapshot: &SourceSnapshot,
        packet_build: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let created = self.record_or_throw(collaboration_id)?;
        if created.state != CollaborationState::Requested {
            return Ok(receipt_for_validated_collaboration(&created));
        }
        self.complete_preview(created, source_snapshot, packet_build)
    }

    /// Records a preview-time refusal without authorizing a provider effect.
    pub fn fail_human_preview(
        &mut self,
        collaboration_id: &str,
        code: CollaborationErrorCode,
        retryable: bool,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        if record.state != CollaborationState::Requested {
            return Ok(receipt_for_validated_collaboration(&record));
        }
        self.fail_receipt(&record, code, retryable)
    }

    /// Commits a known host-side pre-effect failure for an active human
    /// operation. Terminal or unknown records are returned unchanged.
    pub fn fail_human_operation(
        &mut self,
        collaboration_id: &str,
        code: CollaborationErrorCode,
        retryable: bool,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        if record.state.is_terminal() || record.state == CollaborationState::Unknown {
            return Ok(receipt_for_validated_collaboration(&record));
        }
        self.fail_receipt(&record, code, retryable)
    }

    pub fn submit_model(
        &mut self,
        input: CollaborationIntentInput,
        authority: &CollaborationModelAuthority,
        source_snapshot: &SourceSnapshot,
        packet_build: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let started = self.begin_model(input, authority)?;
        if started.state != CollaborationState::Requested {
            return Ok(started);
        }
        self.complete_human_preview(&started.collaboration_id, source_snapshot, packet_build)
    }

    /// Durably admits one model request after validating the broker-resolved
    /// source authority. The caller can then resolve provider/filesystem
    /// preview material without a crash window before the collaboration ID is
    /// allocated.
    pub fn begin_model(
        &mut self,
        mut input: CollaborationIntentInput,
        authority: &CollaborationModelAuthority,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        if authority.depth >= 1 {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::LimitExceeded,
            ));
        }
        let source_key = if input.source_session_key.is_empty() {
            if authority.source_session_keys.len() != 1 {
                return Err(CoordinatorError::ingress(
                    CollaborationErrorCode::SourceNotFound,
                ));
            }
            authority.source_session_keys[0].clone()
        } else {
            input.source_session_key.clone()
        };
        let lineage = authority
            .source_lineage_ids
            .get(&source_key)
            .ok_or_else(|| CoordinatorError::ingress(CollaborationErrorCode::SourceNotFound))?;
        input.source_session_key = source_key;
        input.origin = crate::state::CollaborationOrigin::Model;
        self.begin_preview(input, Some(lineage.as_str()), authority.depth)
    }

    fn begin_preview(
        &mut self,
        input: CollaborationIntentInput,
        lineage_id: Option<&str>,
        depth: u8,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let intent = normalize_collaboration_intent(&input, lineage_id)?;
        let dedupe_key = dedupe_key_for_intent(&intent)?;
        if let Some(replay) = self.journal.find_by_dedupe_key(&dedupe_key)? {
            if replay.payload_hash != payload_hash_for_intent(&intent)? {
                return Err(CoordinatorError::for_record(
                    CollaborationErrorCode::IdempotencyConflict,
                    &replay,
                ));
            }
            return Ok(receipt_for_validated_collaboration(&replay));
        }
        self.expire_pending_approvals()?;
        if self
            .locked_sessions()?
            .iter()
            .any(|session| session.session_key == intent.source_session_key)
        {
            return Err(CoordinatorError::Ingress {
                code: CollaborationErrorCode::SourceDeliveryUnknown,
                collaboration_id: None,
                retryable: false,
            });
        }
        self.assert_concurrency_limits(intent.mode, &intent.source_session_key)?;
        let created_at = self.now_value()?;
        let collaboration_id = (self.new_id)("collaboration");
        let record = create_collaboration_record(CreateCollaborationRecordOptions {
            collaboration_id,
            intent,
            created_at,
            depth,
            limits: self.limits.clone(),
        })?;
        let created = match self.journal.create(record) {
            Ok(result) if result.replayed => {
                return Ok(receipt_for_validated_collaboration(&result.record));
            }
            Ok(result) => result.record,
            Err(error) if error.code == JournalErrorCode::IdempotencyConflict => {
                return Err(CoordinatorError::ingress(
                    CollaborationErrorCode::IdempotencyConflict,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        Ok(receipt_for_validated_collaboration(&created))
    }

    fn complete_preview(
        &mut self,
        created: CollaborationRecordV1,
        source_snapshot: &SourceSnapshot,
        packet_build: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let source = match source_snapshot.to_source() {
            Ok(source) if source.session_key == created.intent.source_session_key => source,
            Ok(_) | Err(_) => {
                return self.fail_receipt(&created, CollaborationErrorCode::StateCorrupt, true);
            }
        };
        let Ok(packet) = packet_build.to_packet() else {
            return self.fail_receipt(&created, CollaborationErrorCode::BrokerUnavailable, true);
        };
        if source.live_status == Some(CollaborationLiveStatus::Failed) {
            return self.fail_receipt(&created, CollaborationErrorCode::SourceNotLive, true);
        }
        if created.intent.mode == CollaborationMode::Handoff
            && source_snapshot
                .worktree
                .as_ref()
                .and_then(CollaborationWorktreeV1::managed)
                .is_some()
        {
            return self.fail_receipt(
                &created,
                CollaborationErrorCode::ManagedWorktreeTransferUnavailable,
                false,
            );
        }
        let at = self.now_at_least(&created.updated_at)?;
        let approval = self.pending_approval(&created, &source, &packet, &at)?;
        let worktree = Some(snapshot_worktree(source_snapshot.worktree.as_ref()));
        self.commit_receipt(
            &created,
            CollaborationState::AwaitingApproval,
            CollaborationTransitionPatch {
                source: Some(source),
                packet: Some(packet),
                approval: Some(approval),
                worktree,
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    pub fn human_status(
        &self,
        collaboration_id: &str,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        Ok(receipt_for_validated_collaboration(
            self.record_ref(collaboration_id)?,
        ))
    }

    /// Exact owner-private record for the host effect runner. This must never
    /// be serialized directly to a renderer or model-facing transport.
    pub fn runtime_record(
        &self,
        collaboration_id: &str,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        self.record_or_throw(collaboration_id)
    }

    pub fn all_receipts(&self) -> Result<Vec<CollaborationReceiptV1>, CoordinatorError> {
        Ok(self
            .journal
            .records()?
            .map(receipt_for_validated_collaboration)
            .collect())
    }

    pub fn model_status(
        &self,
        collaboration_id: &str,
        authority: &CollaborationModelAuthority,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let record = self.authorized_model_record(collaboration_id, authority)?;
        Ok(receipt_for_validated_collaboration(&record))
    }

    pub fn human_participants(
        &self,
        collaboration_id: &str,
    ) -> Result<SessionParticipants, CoordinatorError> {
        let record = self.record_ref(collaboration_id)?;
        Ok(SessionParticipants {
            source_session_key: record.source.session_key.clone(),
            target_session_key: record.target.session_key.clone(),
        })
    }

    /// Whether any record awaits approval, from memory alone. An unhealthy
    /// journal answers `true` so the caller still reaches the real check and
    /// its error.
    #[must_use]
    pub fn has_awaiting_approval(&self) -> bool {
        self.journal.records().map_or(true, |mut records| {
            records.any(|record| record.state == CollaborationState::AwaitingApproval)
        })
    }

    pub fn active_receipts(&self) -> Result<Vec<CollaborationReceiptV1>, CoordinatorError> {
        Ok(self
            .journal
            .records()?
            .filter(|record| !record.state.is_terminal())
            .map(receipt_for_validated_collaboration)
            .collect())
    }

    pub fn approval_preview(
        &mut self,
        collaboration_id: &str,
    ) -> Result<Option<ApprovalPreview>, CoordinatorError> {
        let Some(record) = self.journal.record(collaboration_id)? else {
            return Ok(None);
        };
        if record.state != CollaborationState::AwaitingApproval {
            return Ok(None);
        }
        let now = checked_now(&mut self.now)?;
        if timestamp_millis(&now, "coordinator.now")?
            >= timestamp_millis(
                &record.approval.as_ref().expect("validated").expires_at,
                "record.approval.expiresAt",
            )?
        {
            return Ok(None);
        }
        Ok(Some(project_approval_preview(record)?))
    }

    pub fn pending_approvals(&mut self) -> Result<Vec<ApprovalPreview>, CoordinatorError> {
        let now = self.now_value()?;
        let now = timestamp_millis(&now, "coordinator.now")?;
        self.journal
            .records()?
            .filter(|record| {
                record.state == CollaborationState::AwaitingApproval
                    && record.approval.as_ref().is_some_and(|approval| {
                        timestamp_millis(&approval.expires_at, "record.approval.expiresAt")
                            .is_ok_and(|expires| now < expires)
                    })
            })
            .map(project_approval_preview)
            .collect()
    }

    pub fn pending_peers(&self) -> Result<Vec<PendingPeer>, CoordinatorError> {
        self.journal
            .records()?
            .filter_map(|record| pending_peer_for(record).transpose())
            .collect()
    }

    /// The held peer result for one collaboration, as [`Self::pending_peers`]
    /// would list it, without projecting every other held result.
    pub fn pending_peer(
        &self,
        collaboration_id: &str,
    ) -> Result<Option<PendingPeer>, CoordinatorError> {
        self.journal
            .record(collaboration_id)?
            .map_or(Ok(None), pending_peer_for)
    }

    pub fn locked_sessions(&self) -> Result<Vec<LockedSession>, CoordinatorError> {
        let mut locked = Vec::new();
        for record in self
            .journal
            .records()?
            .filter(|record| !record.state.is_terminal())
        {
            let uncertain_kind = (record.state == CollaborationState::Unknown)
                .then(|| {
                    record
                        .effects
                        .iter()
                        .find(|effect| effect.status == CollaborationEffectStatus::Unknown)
                        .map(|effect| effect.kind)
                })
                .flatten();
            let source_locked = match record.intent.mode {
                CollaborationMode::Handoff => {
                    matches!(
                        record.state,
                        CollaborationState::ParkingSource
                            | CollaborationState::OwnershipTransfer
                            | CollaborationState::ActivatingTarget
                    ) || (record.state == CollaborationState::Unknown
                        && matches!(
                            uncertain_kind,
                            Some(
                                CollaborationEffectKind::ParkSource
                                    | CollaborationEffectKind::TransferOwnership
                                    | CollaborationEffectKind::ActivateTarget
                            )
                        ))
                }
                CollaborationMode::Consult => {
                    record.state == CollaborationState::Unknown
                        && uncertain_kind == Some(CollaborationEffectKind::DeliverResult)
                }
            };
            if source_locked {
                locked.push(LockedSession {
                    collaboration_id: record.collaboration_id.clone(),
                    mode: record.intent.mode,
                    state: record.state,
                    session_key: record.source.session_key.clone(),
                    role: LockedSessionRole::Source,
                });
            }
            if let Some(session_key) = record.target.session_key.clone() {
                locked.push(LockedSession {
                    collaboration_id: record.collaboration_id.clone(),
                    mode: record.intent.mode,
                    state: record.state,
                    session_key,
                    role: LockedSessionRole::Target,
                });
            }
        }
        Ok(locked)
    }

    pub fn approve_human(
        &mut self,
        action: &ApprovalAction,
        current_source: &SourceSnapshot,
        current_packet: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let current = self.record_or_throw(&action.collaboration_id)?;
        if current.approval.as_ref().is_some_and(|approval| {
            approval.approval_id == action.approval_id
                && approval.decision == CollaborationApprovalDecision::Approved
        }) {
            return Ok(receipt_for_validated_collaboration(&current));
        }
        self.assert_pending_approval(&current, action)?;
        let now = self.now_at_least(&current.updated_at)?;
        if timestamp_millis(&now, "coordinator.now")?
            >= timestamp_millis(
                &current.approval.as_ref().expect("checked").expires_at,
                "record.approval.expiresAt",
            )?
        {
            let expired = self.expire_record(current, now)?;
            return Ok(receipt_for_validated_collaboration(&expired));
        }
        match source_drift(&current, current_source, current_packet)? {
            SourceDrift::Refused(code) => return self.fail_receipt(&current, code, true),
            SourceDrift::OtherSession => {
                return self.fail_receipt(&current, CollaborationErrorCode::SourceNotFound, true);
            }
            SourceDrift::Changed(binding) => return self.refresh_approval(&current, *binding),
            SourceDrift::Unchanged => {}
        }
        self.assert_source_ready(&current, current_source)?;
        self.assert_handoff_consultations_settled(&current)?;
        self.assert_target_capacity(&current)?;
        let approved_at = self.now_at_least(&current.updated_at)?;
        let mut approval = current.approval.clone().expect("checked");
        approval.decision = CollaborationApprovalDecision::Approved;
        approval.decided_at = Some(approved_at.clone());
        self.commit_receipt(
            &current,
            CollaborationState::Approved,
            CollaborationTransitionPatch {
                approval: Some(approval),
                ..CollaborationTransitionPatch::default()
            },
            approved_at,
        )
    }

    pub fn deny_human(
        &mut self,
        action: &ApprovalAction,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let current = self.record_or_throw(&action.collaboration_id)?;
        if current.approval.as_ref().is_some_and(|approval| {
            approval.approval_id == action.approval_id
                && approval.decision == CollaborationApprovalDecision::Denied
        }) {
            return Ok(receipt_for_validated_collaboration(&current));
        }
        self.assert_pending_approval(&current, action)?;
        let now = self.now_at_least(&current.updated_at)?;
        if timestamp_millis(&now, "coordinator.now")?
            >= timestamp_millis(
                &current.approval.as_ref().expect("checked").expires_at,
                "record.approval.expiresAt",
            )?
        {
            let expired = self.expire_record(current, now)?;
            return Ok(receipt_for_validated_collaboration(&expired));
        }
        let mut approval = current.approval.clone().expect("checked");
        approval.decision = CollaborationApprovalDecision::Denied;
        approval.decided_at = Some(now.clone());
        self.commit_receipt(
            &current,
            CollaborationState::Denied,
            CollaborationTransitionPatch {
                approval: Some(approval),
                failure: Some(failure(CollaborationErrorCode::Denied, &now, false)),
                ..CollaborationTransitionPatch::default()
            },
            now,
        )
    }

    pub fn expire_pending_approvals(&mut self) -> Result<u64, CoordinatorError> {
        self.assert_admission()?;
        let now = self.now_value()?;
        let mut expired_records = Vec::new();
        for record in self.journal.records()? {
            if record.state != CollaborationState::AwaitingApproval {
                continue;
            }
            let expires_at = timestamp_millis(
                &record.approval.as_ref().expect("validated").expires_at,
                "record.approval.expiresAt",
            )?;
            if timestamp_millis(&now, "coordinator.now")? < expires_at {
                continue;
            }
            let at = later_timestamp(
                &now,
                &record.approval.as_ref().expect("validated").expires_at,
            )?;
            expired_records.push((record.clone(), at));
        }
        let mut count = 0_u64;
        for (record, at) in expired_records {
            match self.expire_record(record, at) {
                Ok(_) => count = count.saturating_add(1),
                Err(CoordinatorError::Journal(error))
                    if error.code == JournalErrorCode::CasConflict => {}
                Err(error) => return Err(error),
            }
        }
        Ok(count)
    }

    pub fn cancel_human(
        &mut self,
        collaboration_id: &str,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        self.cancel_record(self.record_or_throw(collaboration_id)?)
    }

    pub fn cancel_model(
        &mut self,
        collaboration_id: &str,
        authority: &CollaborationModelAuthority,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.authorized_model_record(collaboration_id, authority)?;
        self.cancel_record(record)
    }

    fn cancel_record(
        &mut self,
        record: CollaborationRecordV1,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        if record.state.is_terminal() || !cancellation_is_safe(&record) {
            return Ok(receipt_for_validated_collaboration(&record));
        }
        let at = self.now_at_least(&record.updated_at)?;
        self.commit_receipt(
            &record,
            CollaborationState::Cancelled,
            CollaborationTransitionPatch::default(),
            at,
        )
    }

    /// Revalidates the approval-bound source/packet and durably reserves target
    /// preparation. No provider call is authorized by this method.
    pub fn reserve_prepare(
        &mut self,
        collaboration_id: &str,
        fresh_source: &SourceSnapshot,
        fresh_packet: &PacketBuildResult,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let current = self.record_or_throw(collaboration_id)?;
        if current.state != CollaborationState::Approved
            && current.state != CollaborationState::Preflight
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::ApprovalStale,
                &current,
            ));
        }
        match source_drift(&current, fresh_source, fresh_packet)? {
            SourceDrift::Refused(code) => return self.fail_receipt(&current, code, true),
            SourceDrift::OtherSession => {
                return self.fail_receipt(&current, CollaborationErrorCode::SourceNotFound, true);
            }
            SourceDrift::Changed(_) if current.state == CollaborationState::Preflight => {
                return self.fail_receipt(&current, CollaborationErrorCode::ApprovalStale, true);
            }
            SourceDrift::Changed(binding) => return self.refresh_approval(&current, *binding),
            SourceDrift::Unchanged => {}
        }
        self.assert_source_ready(&current, fresh_source)?;
        self.assert_handoff_consultations_settled(&current)?;
        self.assert_target_capacity(&current)?;
        let preflight = if current.state == CollaborationState::Preflight {
            current
        } else {
            let preflight_at = self.now_at_least(&current.updated_at)?;
            self.commit(
                &current,
                CollaborationState::Preflight,
                CollaborationTransitionPatch::default(),
                preflight_at,
                None,
            )?
        };
        let reserve_at = self.now_at_least(&preflight.updated_at)?;
        let prepare = reserve_effect(
            &preflight,
            CollaborationEffectKind::PrepareTarget,
            &reserve_at,
            effect_timeout(&preflight),
        )?;
        let mut effects = preflight.effects.clone();
        effects.set(prepare);
        self.commit_receipt(
            &preflight,
            CollaborationState::PreparingTarget,
            CollaborationTransitionPatch {
                effects: Some(effects),
                ..CollaborationTransitionPatch::default()
            },
            reserve_at,
        )
    }

    /// Consumes the one permitted attempt for the state's reserved effect and
    /// only then returns a provider command. A `started` attempt is never
    /// returned again.
    pub fn start_reserved_effect(
        &mut self,
        collaboration_id: &str,
        fresh_source: Option<&SourceSnapshot>,
        fresh_packet: Option<&PacketBuildResult>,
    ) -> Result<Option<EffectCommand>, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        let kind = crate::state::maintenance_effect_for_state(record.state, record.intent.mode)
            .ok_or_else(|| {
                CoordinatorError::for_record(CollaborationErrorCode::RecoveryRequired, &record)
            })?;
        let effect = record.effects.get(kind).ok_or_else(|| corrupt(&record))?;
        if effect.status == CollaborationEffectStatus::Started {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::RecoveryRequired,
                &record,
            ));
        }
        if effect.status != CollaborationEffectStatus::Reserved {
            return Err(corrupt(&record));
        }
        let now = self.now_at_least(&record.updated_at)?;
        if timestamp_millis(&now, "coordinator.now")?
            >= timestamp_millis(&effect.deadline_at, "record.effect.deadlineAt")?
        {
            self.fail_record(&record, CollaborationErrorCode::BrokerUnavailable, true)?;
            return Ok(None);
        }
        match kind {
            CollaborationEffectKind::PrepareTarget | CollaborationEffectKind::ParkSource => {
                let source = fresh_source.ok_or_else(|| {
                    CoordinatorError::for_record(CollaborationErrorCode::ApprovalStale, &record)
                })?;
                let packet = fresh_packet.ok_or_else(|| {
                    CoordinatorError::for_record(CollaborationErrorCode::ApprovalStale, &record)
                })?;
                let refused = match source_drift(&record, source, packet)? {
                    SourceDrift::Refused(code) => Some(code),
                    // The session key is part of the approval guard, so a
                    // different session is a moved guard here.
                    SourceDrift::OtherSession | SourceDrift::Changed(_) => {
                        Some(CollaborationErrorCode::ApprovalStale)
                    }
                    SourceDrift::Unchanged => None,
                };
                if let Some(code) = refused {
                    self.fail_record(&record, code, true)?;
                    return Ok(None);
                }
                self.assert_source_ready(&record, source)?;
                if kind == CollaborationEffectKind::ParkSource {
                    self.assert_source_dispatch_ready(&record, source)?;
                }
            }
            CollaborationEffectKind::DeliverResult => {
                let source = fresh_source.ok_or_else(|| {
                    CoordinatorError::for_record(CollaborationErrorCode::SourceNotLive, &record)
                })?;
                self.assert_source_dispatch_ready(&record, source)?;
                let guard = source_dispatch_guard_hash(source)?;
                let peer = record
                    .peer_result
                    .as_ref()
                    .ok_or_else(|| corrupt(&record))?;
                let receipt = peer.acceptance.as_ref().ok_or_else(|| {
                    CoordinatorError::for_record(CollaborationErrorCode::ApprovalStale, &record)
                })?;
                let expected_receipt = deterministic_id(
                    "acceptance",
                    &[&record.collaboration_id, &peer.result_id, &guard],
                );
                if receipt.receipt_id != expected_receipt {
                    // A restarted source needs explicit human acceptance rebind.
                    return Ok(None);
                }
            }
            CollaborationEffectKind::RunTarget
            | CollaborationEffectKind::TransferOwnership
            | CollaborationEffectKind::ActivateTarget => {}
        }
        let mut effects = record.effects.clone();
        let mut started = effect.clone();
        started.status = CollaborationEffectStatus::Started;
        started.attempts = 1;
        started.started_at = Some(now.clone());
        effects.set(started);
        let started_record = self.commit(
            &record,
            record.state,
            CollaborationTransitionPatch {
                effects: Some(effects),
                ..CollaborationTransitionPatch::default()
            },
            now,
            Some(kind),
        )?;
        Ok(Some(self.effect_command(
            &started_record,
            kind,
            fresh_source,
        )?))
    }

    /// Settles exactly the durable started attempt identified by `effect_id`.
    /// Ambiguous outcomes enter `unknown` and are never automatically retried.
    pub fn settle_effect(
        &mut self,
        collaboration_id: &str,
        effect_id: &str,
        outcome: EffectOutcome,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        let effect = record
            .effects
            .iter()
            .find(|effect| effect.effect_id == effect_id)
            .cloned()
            .ok_or_else(|| corrupt(&record))?;
        if effect.status != CollaborationEffectStatus::Started || effect.attempts != 1 {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::RecoveryRequired,
                &record,
            ));
        }
        match outcome {
            EffectOutcome::Succeeded {
                value,
                provider_effect_id,
            } => self.settle_success(record, effect, value, provider_effect_id),
            EffectOutcome::ProvenUnsent { code } => {
                let code = if code.is_ambiguous() {
                    CollaborationErrorCode::BrokerUnavailable
                } else {
                    code
                };
                self.settle_failure(record, effect, code, None, false)
            }
            EffectOutcome::Ambiguous {
                code: _,
                provider_effect_id,
            } => self.settle_failure(
                record,
                effect.clone(),
                ambiguous_code(effect.kind),
                provider_effect_id,
                true,
            ),
        }
    }

    fn settle_success(
        &mut self,
        record: CollaborationRecordV1,
        effect: CollaborationEffectRecordV1,
        value: EffectSuccess,
        provider_effect_id: Option<String>,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let at = self.settlement_at(&record, &effect)?;
        let mut settled = effect.clone();
        settled.status = CollaborationEffectStatus::Succeeded;
        settled.settled_at = Some(at.clone());
        settled.provider_effect_id = provider_effect_id;
        settled.error_code = None;
        settled.validate()?;
        let mut effects = record.effects.clone();
        effects.set(settled);
        // Only the payload differs per effect; the successor state and the
        // effect it reserves come from the ladder table in `state/transition.rs`.
        let mut patch = CollaborationTransitionPatch::default();
        match (effect.kind, value) {
            (CollaborationEffectKind::PrepareTarget, EffectSuccess::PreparedTarget(prepared)) => {
                let target = CollaborationTargetV1 {
                    engine: record.target.engine,
                    session_key: Some(prepared.session_key),
                    provider_session_id: Some(prepared.provider_session_id),
                    reconciliation_marker: prepared.reconciliation_marker,
                };
                target.validate()?;
                patch.target = Some(target);
            }
            (CollaborationEffectKind::RunTarget, EffectSuccess::ConsultResult(text)) => {
                let body = bound_provider_body(&text, record.limits.max_result_bytes);
                let digest = sha256(&body);
                patch.peer_result = Some(CollaborationPeerResultV1 {
                    version: 1,
                    result_id: peer_result_id(&record.collaboration_id, &digest)?,
                    digest,
                    bytes: u64::try_from(body.len()).unwrap_or(u64::MAX),
                    state: CollaborationPeerResultState::Ready,
                    created_at: at.clone(),
                    target_session_key: record
                        .target
                        .session_key
                        .clone()
                        .ok_or_else(|| corrupt(&record))?,
                    queue_entry_id: None,
                    acceptance: None,
                    body: Some(body),
                });
            }
            (CollaborationEffectKind::DeliverResult, EffectSuccess::Unit) => {
                let mut peer_result = record.peer_result.clone().ok_or_else(|| corrupt(&record))?;
                peer_result.state = CollaborationPeerResultState::Delivered;
                patch.peer_result = Some(peer_result);
            }
            (
                CollaborationEffectKind::ParkSource
                | CollaborationEffectKind::TransferOwnership
                | CollaborationEffectKind::ActivateTarget,
                EffectSuccess::Unit,
            ) => {}
            _ => return Err(corrupt(&record)),
        }
        let (next_state, next_effect) = effect_successor(effect.kind, record.intent.mode);
        if let Some(next) = next_effect {
            effects.set(reserve_effect(&record, next, &at, effect_timeout(&record))?);
        }
        patch.effects = Some(effects);
        self.commit_receipt(&record, next_state, patch, at)
    }

    fn settle_failure(
        &mut self,
        record: CollaborationRecordV1,
        effect: CollaborationEffectRecordV1,
        code: CollaborationErrorCode,
        provider_effect_id: Option<String>,
        ambiguous: bool,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let at = self.settlement_at(&record, &effect)?;
        let mut settled = effect.clone();
        settled.status = if ambiguous {
            CollaborationEffectStatus::Unknown
        } else {
            CollaborationEffectStatus::Failed
        };
        settled.settled_at = Some(at.clone());
        settled.provider_effect_id = provider_effect_id;
        settled.error_code = Some(code);
        settled.validate()?;
        let mut effects = record.effects.clone();
        effects.set(settled);
        let mut patch = CollaborationTransitionPatch {
            effects: Some(effects),
            failure: Some(failure(code, &at, !ambiguous)),
            ..CollaborationTransitionPatch::default()
        };
        if ambiguous && effect.kind == CollaborationEffectKind::DeliverResult {
            let mut peer_result = record.peer_result.clone().ok_or_else(|| corrupt(&record))?;
            peer_result.state = CollaborationPeerResultState::Unknown;
            patch.peer_result = Some(peer_result);
        }
        self.commit_receipt(
            &record,
            if ambiguous {
                CollaborationState::Unknown
            } else {
                CollaborationState::Failed
            },
            patch,
            at,
        )
    }

    pub fn hold_peer_result(
        &mut self,
        collaboration_id: &str,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        if record.state == CollaborationState::AwaitingPeerReview {
            return Ok(receipt_for_validated_collaboration(&record));
        }
        if record.state != CollaborationState::ResultReady {
            return Err(corrupt(&record));
        }
        let mut result = record.peer_result.clone().ok_or_else(|| corrupt(&record))?;
        result.state = CollaborationPeerResultState::Held;
        result.queue_entry_id = Some(deterministic_id(
            "queue",
            &[&record.collaboration_id, &result.result_id],
        ));
        let at = self.now_at_least(&record.updated_at)?;
        self.commit_receipt(
            &record,
            CollaborationState::AwaitingPeerReview,
            CollaborationTransitionPatch {
                peer_result: Some(result),
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    pub fn accept_peer_human(
        &mut self,
        action: &PeerAction,
        source_snapshot: &SourceSnapshot,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let current = self.record_or_throw(&action.collaboration_id)?;
        let rebinding = current.state == CollaborationState::DeliveryPending
            && current.effects.has_status(
                CollaborationEffectKind::DeliverResult,
                CollaborationEffectStatus::Reserved,
            )
            && current.peer_result.as_ref().is_some_and(|result| {
                result.result_id == action.result_id
                    && result.digest == action.result_digest
                    && result.acceptance.as_ref().is_some_and(|acceptance| {
                        acceptance.decision == CollaborationPeerDecision::Accepted
                    })
            });
        if rebinding {
            if current.revision != action.expected_revision {
                return Err(CoordinatorError::for_record(
                    CollaborationErrorCode::ApprovalStale,
                    &current,
                ));
            }
        } else {
            self.assert_held_peer(&current, action)?;
        }
        self.assert_source_dispatch_ready(&current, source_snapshot)?;
        let delivery_guard = source_dispatch_guard_hash(source_snapshot)?;
        let at = self.now_at_least(&current.updated_at)?;
        let receipt_id = deterministic_id(
            "acceptance",
            &[
                &current.collaboration_id,
                &action.result_id,
                &delivery_guard,
            ],
        );
        if rebinding {
            let mut result = current.peer_result.clone().expect("checked");
            let acceptance = result.acceptance.as_mut().expect("checked");
            if acceptance.receipt_id == receipt_id {
                return Ok(receipt_for_validated_collaboration(&current));
            }
            acceptance.receipt_id = receipt_id;
            return self.commit_receipt(
                &current,
                CollaborationState::DeliveryPending,
                CollaborationTransitionPatch {
                    peer_result: Some(result),
                    ..CollaborationTransitionPatch::default()
                },
                at,
            );
        }
        let mut result = current.peer_result.clone().expect("checked");
        result.state = CollaborationPeerResultState::DeliveryPending;
        result.acceptance = Some(CollaborationPeerAcceptanceV1 {
            version: 1,
            receipt_id,
            result_id: action.result_id.clone(),
            result_digest: action.result_digest.clone(),
            decision: CollaborationPeerDecision::Accepted,
            decided_at: at.clone(),
        });
        let mut effects = current.effects.clone();
        effects.set(reserve_effect(
            &current,
            CollaborationEffectKind::DeliverResult,
            &at,
            current.limits.consultation_timeout_ms.unwrap_or_default(),
        )?);
        self.commit_receipt(
            &current,
            CollaborationState::DeliveryPending,
            CollaborationTransitionPatch {
                peer_result: Some(result),
                effects: Some(effects),
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    pub fn discard_peer_human(
        &mut self,
        action: &PeerAction,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let current = self.record_or_throw(&action.collaboration_id)?;
        if current.peer_result.as_ref().is_some_and(|result| {
            result.result_id == action.result_id
                && result.digest == action.result_digest
                && result.acceptance.as_ref().is_some_and(|acceptance| {
                    acceptance.decision == CollaborationPeerDecision::Discarded
                })
        }) {
            return Ok(receipt_for_validated_collaboration(&current));
        }
        self.assert_held_peer(&current, action)?;
        let at = self.now_at_least(&current.updated_at)?;
        let mut result = current.peer_result.clone().expect("checked");
        result.state = CollaborationPeerResultState::Discarded;
        result.acceptance = Some(CollaborationPeerAcceptanceV1 {
            version: 1,
            receipt_id: (self.new_id)("acceptance"),
            result_id: action.result_id.clone(),
            result_digest: action.result_digest.clone(),
            decision: CollaborationPeerDecision::Discarded,
            decided_at: at.clone(),
        });
        self.commit_receipt(
            &current,
            CollaborationState::Completed,
            CollaborationTransitionPatch {
                peer_result: Some(result),
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    /// Reconciles an unknown effect to a proven terminal failure. This records
    /// a marker but never dispatches the effect again.
    pub fn reconcile_unknown_failed(
        &mut self,
        collaboration_id: &str,
        reconciliation_marker: &str,
        code: CollaborationErrorCode,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        self.assert_admission()?;
        let record = self.record_or_throw(collaboration_id)?;
        if record.state != CollaborationState::Unknown || code.is_ambiguous() {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::RecoveryRequired,
                &record,
            ));
        }
        let mut target = record.target.clone();
        target.reconciliation_marker = Some(reconciliation_marker.to_owned());
        let at = self.now_at_least(&record.updated_at)?;
        self.commit_receipt(
            &record,
            CollaborationState::Failed,
            CollaborationTransitionPatch {
                target: Some(target),
                failure: Some(failure(code, &at, false)),
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    pub fn quiesce(&mut self) {
        self.accepting = false;
        self.quiesced = true;
    }

    pub fn stop(&mut self) -> Result<(), CoordinatorError> {
        self.quiesce();
        self.journal.close()?;
        // Audit is a best-effort projection (as in the 0.1.169 oracle): a
        // failed close cannot fail the journal close that already succeeded.
        if let Some(audit) = self.audit.as_mut() {
            let _ = audit.close();
        }
        Ok(())
    }

    fn commit(
        &mut self,
        current: &CollaborationRecordV1,
        to: CollaborationState,
        patch: CollaborationTransitionPatch,
        at: String,
        maintenance_effect: Option<CollaborationEffectKind>,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        let event_id = deterministic_id(
            "event",
            &[
                &current.collaboration_id,
                &current.revision.saturating_add(1).to_string(),
            ],
        );
        let transitioned = transition_collaboration_record(
            current,
            CollaborationTransitionRequest {
                expected_revision: current.revision,
                expected_state: current.state,
                to,
                maintenance_effect,
                at,
                event_id: Some(event_id),
                patch,
            },
        )?;
        let committed = self.journal.compare_and_set(
            &current.collaboration_id,
            current.revision,
            current.state,
            transitioned.record,
        )?;
        // The state transition already committed. Audit is a projection, so a
        // failure (which poisons the sink for later records) cannot compensate
        // or make the record appear uncommitted.
        if let Some(audit) = self.audit.as_mut() {
            let _ = audit.record(transitioned.audit);
        }
        Ok(committed)
    }

    /// [`Self::commit`] for a logical transition, answered with the
    /// committed record's receipt.
    fn commit_receipt(
        &mut self,
        current: &CollaborationRecordV1,
        to: CollaborationState,
        patch: CollaborationTransitionPatch,
        at: String,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let committed = self.commit(current, to, patch, at, None)?;
        Ok(receipt_for_validated_collaboration(&committed))
    }

    fn pending_approval(
        &mut self,
        record: &CollaborationRecordV1,
        source: &CollaborationSourceV1,
        packet: &CollaborationPacketRecordV1,
        at: &str,
    ) -> Result<CollaborationApprovalRecordV1, CoordinatorError> {
        Ok(CollaborationApprovalRecordV1 {
            version: 1,
            approval_id: (self.new_id)("approval"),
            collaboration_id: record.collaboration_id.clone(),
            packet_hash: packet.hash.clone(),
            source_registry_revision: source.registry_revision.ok_or_else(|| corrupt(record))?,
            source_guard_hash: source_guard_hash_for_approval(source)?,
            target_engine: record.target.engine,
            created_at: at.to_owned(),
            expires_at: add_milliseconds(at, record.limits.approval_timeout_ms)?,
            decision: CollaborationApprovalDecision::Pending,
            decided_at: None,
        })
    }

    /// Replaces a drifted approval binding with a fresh single-use approval.
    fn refresh_approval(
        &mut self,
        current: &CollaborationRecordV1,
        binding: ApprovalBinding,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let ApprovalBinding {
            source,
            packet,
            worktree,
        } = binding;
        let at = self.now_at_least(&current.updated_at)?;
        let approval = self.pending_approval(current, &source, &packet, &at)?;
        self.commit_receipt(
            current,
            CollaborationState::AwaitingApproval,
            CollaborationTransitionPatch {
                source: Some(source),
                packet: Some(packet),
                approval: Some(approval),
                worktree: Some(worktree),
                ..CollaborationTransitionPatch::default()
            },
            at,
        )
    }

    fn expire_record(
        &mut self,
        current: CollaborationRecordV1,
        now: String,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        let at = later_timestamp(
            &now,
            &current.approval.as_ref().expect("validated").expires_at,
        )?;
        let mut approval = current.approval.clone().expect("validated");
        approval.decision = CollaborationApprovalDecision::Expired;
        approval.decided_at = Some(at.clone());
        self.commit(
            &current,
            CollaborationState::Cancelled,
            CollaborationTransitionPatch {
                approval: Some(approval),
                failure: Some(failure(CollaborationErrorCode::ApprovalExpired, &at, true)),
                ..CollaborationTransitionPatch::default()
            },
            at,
            None,
        )
    }

    fn fail_receipt(
        &mut self,
        record: &CollaborationRecordV1,
        code: CollaborationErrorCode,
        retryable: bool,
    ) -> Result<CollaborationReceiptV1, CoordinatorError> {
        let failed = self.fail_record(record, code, retryable)?;
        Ok(receipt_for_validated_collaboration(&failed))
    }

    fn fail_record(
        &mut self,
        record: &CollaborationRecordV1,
        code: CollaborationErrorCode,
        retryable: bool,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        let at = self.now_at_least(&record.updated_at)?;
        self.commit(
            record,
            CollaborationState::Failed,
            CollaborationTransitionPatch {
                failure: Some(failure(code, &at, retryable)),
                ..CollaborationTransitionPatch::default()
            },
            at,
            None,
        )
    }

    fn quarantine_started(
        &mut self,
        record: CollaborationRecordV1,
        effect: CollaborationEffectRecordV1,
    ) -> Result<(), CoordinatorError> {
        self.settle_failure(
            record,
            effect.clone(),
            ambiguous_code(effect.kind),
            None,
            true,
        )?;
        Ok(())
    }

    fn assert_pending_approval(
        &self,
        record: &CollaborationRecordV1,
        action: &ApprovalAction,
    ) -> Result<(), CoordinatorError> {
        if record.state != CollaborationState::AwaitingApproval
            || record.revision != action.expected_revision
            || record.approval.as_ref().is_none_or(|approval| {
                approval.approval_id != action.approval_id
                    || approval.decision != CollaborationApprovalDecision::Pending
            })
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::ApprovalStale,
                record,
            ));
        }
        Ok(())
    }

    fn assert_held_peer(
        &self,
        record: &CollaborationRecordV1,
        action: &PeerAction,
    ) -> Result<(), CoordinatorError> {
        if record.state != CollaborationState::AwaitingPeerReview
            || record.revision != action.expected_revision
            || record.peer_result.as_ref().is_none_or(|peer| {
                peer.state != CollaborationPeerResultState::Held
                    || peer.result_id != action.result_id
                    || peer.digest != action.result_digest
            })
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::ApprovalStale,
                record,
            ));
        }
        Ok(())
    }

    fn assert_source_ready(
        &self,
        record: &CollaborationRecordV1,
        snapshot: &SourceSnapshot,
    ) -> Result<(), CoordinatorError> {
        if snapshot.live_status == CollaborationLiveStatus::Failed {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceNotLive,
                record,
            ));
        }
        if record.intent.mode == CollaborationMode::Handoff
            && snapshot.live_status != CollaborationLiveStatus::Idle
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceNotIdle,
                record,
            ));
        }
        Ok(())
    }

    fn assert_source_dispatch_ready(
        &self,
        record: &CollaborationRecordV1,
        snapshot: &SourceSnapshot,
    ) -> Result<(), CoordinatorError> {
        if snapshot.session_key != record.source.session_key
            || snapshot.live_status == CollaborationLiveStatus::Failed
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceNotLive,
                record,
            ));
        }
        if snapshot.live_status != CollaborationLiveStatus::Idle {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceNotIdle,
                record,
            ));
        }
        if snapshot.delivery_unknown {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceDeliveryUnknown,
                record,
            ));
        }
        if snapshot.pending_approval_count > 0 {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::PendingSourceApproval,
                record,
            ));
        }
        if snapshot.queue_depth > 0 {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::SourceQueueNotEmpty,
                record,
            ));
        }
        Ok(())
    }

    fn assert_concurrency_limits(
        &self,
        mode: CollaborationMode,
        source_session_key: &str,
    ) -> Result<(), CoordinatorError> {
        let mut has_active_consult = false;
        let mut same_mode = 0usize;
        let mut target_capacity = 0usize;
        for record in self.journal.records()? {
            if mode == CollaborationMode::Handoff
                && record.source.session_key == source_session_key
                && record.intent.mode == CollaborationMode::Consult
                && !record.state.is_terminal()
            {
                has_active_consult = true;
            }
            if !record.state.is_terminal()
                && record.state != CollaborationState::Unknown
                && record.source.session_key == source_session_key
                && record.intent.mode == mode
            {
                same_mode += 1;
            }
            if target_capacity_record(record) {
                target_capacity += 1;
            }
        }
        if has_active_consult {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::LimitExceeded,
            ));
        }
        let limit = match mode {
            CollaborationMode::Consult => self
                .limits
                .max_active_consultations_per_source
                .unwrap_or(crate::state::MAX_ACTIVE_CONSULTATIONS_PER_SOURCE),
            CollaborationMode::Handoff => self
                .limits
                .max_active_handoffs_per_source
                .unwrap_or(crate::state::MAX_ACTIVE_HANDOFFS_PER_SOURCE),
        };
        if u64::try_from(same_mode).unwrap_or(u64::MAX) >= limit {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::LimitExceeded,
            ));
        }
        let global_limit = self
            .limits
            .max_running_targets
            .unwrap_or(crate::state::MAX_RUNNING_TARGETS);
        if u64::try_from(target_capacity).unwrap_or(u64::MAX) >= global_limit {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::LimitExceeded,
            ));
        }
        Ok(())
    }

    fn assert_handoff_consultations_settled(
        &self,
        record: &CollaborationRecordV1,
    ) -> Result<(), CoordinatorError> {
        if record.intent.mode == CollaborationMode::Handoff
            && self.journal.records()?.any(|candidate| {
                candidate.collaboration_id != record.collaboration_id
                    && candidate.source.session_key == record.source.session_key
                    && candidate.intent.mode == CollaborationMode::Consult
                    && !candidate.state.is_terminal()
            })
        {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::LimitExceeded,
                record,
            ));
        }
        Ok(())
    }

    fn assert_target_capacity(
        &self,
        record: &CollaborationRecordV1,
    ) -> Result<(), CoordinatorError> {
        let used = self
            .journal
            .records()?
            .filter(|candidate| {
                candidate.collaboration_id != record.collaboration_id
                    && target_capacity_record(candidate)
            })
            .count();
        if u64::try_from(used).unwrap_or(u64::MAX) >= record.limits.max_running_targets {
            return Err(CoordinatorError::for_record(
                CollaborationErrorCode::LimitExceeded,
                record,
            ));
        }
        Ok(())
    }

    fn authorized_model_record(
        &self,
        collaboration_id: &str,
        authority: &CollaborationModelAuthority,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        let record = self.record_or_throw(collaboration_id)?;
        let lineage = authority
            .source_lineage_ids
            .get(&record.source.session_key)
            .ok_or_else(|| CoordinatorError::ingress(CollaborationErrorCode::SourceNotFound))?;
        let fingerprint = fingerprint_origin_lineage(lineage)?;
        if record.intent.origin != crate::state::CollaborationOrigin::Model
            || record.intent.origin_lineage_fingerprint.as_deref() != Some(&fingerprint)
        {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::SourceNotFound,
            ));
        }
        Ok(record)
    }

    fn record_or_throw(
        &self,
        collaboration_id: &str,
    ) -> Result<CollaborationRecordV1, CoordinatorError> {
        self.record_ref(collaboration_id).cloned()
    }

    /// Borrowed form of `record_or_throw` for read-only projections.
    fn record_ref(
        &self,
        collaboration_id: &str,
    ) -> Result<&CollaborationRecordV1, CoordinatorError> {
        self.journal
            .record(collaboration_id)?
            .ok_or_else(|| CoordinatorError::ingress(CollaborationErrorCode::SourceNotFound))
    }

    fn effect_command(
        &self,
        record: &CollaborationRecordV1,
        kind: CollaborationEffectKind,
        fresh_source: Option<&SourceSnapshot>,
    ) -> Result<EffectCommand, CoordinatorError> {
        let effect_id = record
            .effects
            .get(kind)
            .ok_or_else(|| corrupt(record))?
            .effect_id
            .clone();
        let common_id = record.collaboration_id.clone();
        let command = match kind {
            CollaborationEffectKind::PrepareTarget => EffectCommand::PrepareTarget {
                collaboration_id: common_id,
                effect_id,
                mode: record.intent.mode,
                target_collaboration_depth: u8::from(
                    record.intent.mode == CollaborationMode::Consult,
                ),
                source: record.source.clone(),
                target_engine: record.target.engine,
            },
            CollaborationEffectKind::RunTarget => {
                let (packet_body, objective) = transient_inputs(record)?;
                EffectCommand::RunTarget {
                    collaboration_id: common_id,
                    effect_id,
                    source: record.source.clone(),
                    target: record.target.clone(),
                    packet_body,
                    objective,
                }
            }
            CollaborationEffectKind::ParkSource => EffectCommand::ParkSource {
                collaboration_id: common_id,
                effect_id,
                source: record.source.clone(),
                target: record.target.clone(),
                expected_source_guard_hash: source_guard_hash_for_approval(&record.source)?,
            },
            CollaborationEffectKind::TransferOwnership => EffectCommand::TransferOwnership {
                collaboration_id: common_id,
                effect_id,
                source: record.source.clone(),
                target: record.target.clone(),
            },
            CollaborationEffectKind::ActivateTarget => {
                let (packet_body, objective) = transient_inputs(record)?;
                EffectCommand::ActivateTarget {
                    collaboration_id: common_id,
                    effect_id,
                    source: record.source.clone(),
                    target: record.target.clone(),
                    packet_body,
                    objective,
                }
            }
            CollaborationEffectKind::DeliverResult => {
                let peer = record.peer_result.as_ref().ok_or_else(|| corrupt(record))?;
                let source = fresh_source.ok_or_else(|| {
                    CoordinatorError::for_record(CollaborationErrorCode::SourceNotLive, record)
                })?;
                EffectCommand::DeliverResult {
                    collaboration_id: common_id,
                    effect_id,
                    source: source.to_source()?,
                    target: record.target.clone(),
                    queue_entry_id: peer.queue_entry_id.clone().ok_or_else(|| corrupt(record))?,
                    result_id: peer.result_id.clone(),
                    result_digest: peer.digest.clone(),
                    body: peer.body.clone().ok_or_else(|| corrupt(record))?,
                    expected_source_guard_hash: source_dispatch_guard_hash(source)?,
                }
            }
        };
        Ok(command)
    }

    fn settlement_at(
        &mut self,
        record: &CollaborationRecordV1,
        effect: &CollaborationEffectRecordV1,
    ) -> Result<String, CoordinatorError> {
        let now = self.now_at_least(&record.updated_at)?;
        if timestamp_millis(&now, "coordinator.now")?
            > timestamp_millis(&effect.deadline_at, "record.effect.deadlineAt")?
        {
            return Ok(effect.deadline_at.clone());
        }
        Ok(now)
    }

    fn now_value(&mut self) -> Result<String, CoordinatorError> {
        checked_now(&mut self.now)
    }

    fn now_at_least(&mut self, previous: &str) -> Result<String, CoordinatorError> {
        let now = self.now_value()?;
        later_timestamp(&now, previous)
    }

    fn assert_admission(&self) -> Result<(), CoordinatorError> {
        if !self.accepting || self.quiesced {
            return Err(CoordinatorError::ingress(
                CollaborationErrorCode::BrokerUnavailable,
            ));
        }
        Ok(())
    }
}

/// An absent worktree means the same thing as an explicitly unmanaged one.
/// The record persists the normalized value, so every drift comparison must
/// normalize both sides or an absent snapshot worktree can never settle.
fn snapshot_worktree(worktree: Option<&CollaborationWorktreeV1>) -> CollaborationWorktreeV1 {
    worktree
        .cloned()
        .unwrap_or(CollaborationWorktreeV1::Unmanaged(UnmanagedWorktreeV1 {
            version: 1,
            managed: false,
        }))
}

/// The approval-bound values a drifted snapshot carries: what an approval
/// refresh records in place of the record's own.
struct ApprovalBinding {
    source: CollaborationSourceV1,
    packet: CollaborationPacketRecordV1,
    worktree: CollaborationWorktreeV1,
}

/// What a fresh source snapshot and packet mean for an approval-bound record.
enum SourceDrift {
    /// The snapshot or packet is unusable: the caller commits this failure.
    Refused(CollaborationErrorCode),
    /// The snapshot is of a different source session.
    OtherSession,
    /// The approval no longer binds the snapshot, packet or worktree.
    Changed(Box<ApprovalBinding>),
    /// The approval still binds exactly this source, packet and worktree.
    Unchanged,
}

/// The one approval-drift check approval, target reservation and dispatch
/// share. The snapshot worktree is normalized as `complete_preview` persists
/// it, both for the comparison and for the value a refresh records.
fn source_drift(
    record: &CollaborationRecordV1,
    snapshot: &SourceSnapshot,
    packet: &PacketBuildResult,
) -> Result<SourceDrift, CoordinatorError> {
    if snapshot.live_status == CollaborationLiveStatus::Failed {
        return Ok(SourceDrift::Refused(CollaborationErrorCode::SourceNotLive));
    }
    let Ok(source) = snapshot.to_source() else {
        return Ok(SourceDrift::Refused(CollaborationErrorCode::StateCorrupt));
    };
    let Ok(packet) = packet.to_packet() else {
        return Ok(SourceDrift::Refused(
            CollaborationErrorCode::BrokerUnavailable,
        ));
    };
    if source.session_key != record.source.session_key {
        return Ok(SourceDrift::OtherSession);
    }
    let worktree = snapshot_worktree(snapshot.worktree.as_ref());
    let changed = source_guard_hash_for_approval(&source)?
        != source_guard_hash_for_approval(&record.source)?
        || record.packet.as_ref().map(|value| &value.hash) != Some(&packet.hash)
        || worktree != snapshot_worktree(record.worktree.as_ref());
    Ok(if changed {
        SourceDrift::Changed(Box::new(ApprovalBinding {
            source,
            packet,
            worktree,
        }))
    } else {
        SourceDrift::Unchanged
    })
}

fn target_capacity_record(record: &CollaborationRecordV1) -> bool {
    !record.state.is_terminal()
        && (record
            .approval
            .as_ref()
            .is_some_and(|approval| approval.decision == CollaborationApprovalDecision::Approved)
            || record.target.session_key.is_some()
            || record.effects.prepare_target.is_some())
}

/// Reads the coordinator clock and rejects a malformed timestamp. A free
/// function so a caller can hold a journal borrow while reading the clock.
fn checked_now(now: &mut Now) -> Result<String, CoordinatorError> {
    let now = now();
    timestamp_millis(&now, "coordinator.now")?;
    Ok(now)
}

/// The held peer result a record carries, if it is awaiting peer review or
/// has a reserved delivery. A missing field is `StateCorrupt` for that record.
fn pending_peer_for(
    record: &CollaborationRecordV1,
) -> Result<Option<PendingPeer>, CoordinatorError> {
    if !(record.state == CollaborationState::AwaitingPeerReview
        || (record.state == CollaborationState::DeliveryPending
            && record.effects.has_status(
                CollaborationEffectKind::DeliverResult,
                CollaborationEffectStatus::Reserved,
            )))
    {
        return Ok(None);
    }
    let result = record.peer_result.as_ref().ok_or_else(|| corrupt(record))?;
    Ok(Some(PendingPeer {
        state: if record.state == CollaborationState::AwaitingPeerReview {
            PendingPeerState::AwaitingReview
        } else {
            PendingPeerState::DeliveryPending
        },
        collaboration_id: record.collaboration_id.clone(),
        expected_revision: record.revision,
        queue_entry_id: result
            .queue_entry_id
            .clone()
            .ok_or_else(|| corrupt(record))?,
        result_id: result.result_id.clone(),
        result_digest: result.digest.clone(),
        result_bytes: result.bytes,
        source_session_key: record.source.session_key.clone(),
        source_engine: record.source.engine,
        target_session_key: record
            .target
            .session_key
            .clone()
            .ok_or_else(|| corrupt(record))?,
        target_engine: record.target.engine,
        objective: record
            .intent
            .objective
            .clone()
            .ok_or_else(|| corrupt(record))?,
        body: result.body.clone().ok_or_else(|| corrupt(record))?,
    }))
}

/// The packet body and objective a target run or activation carries; both
/// are pruned at terminal cleanup, so an active record without them is
/// corrupt.
fn transient_inputs(record: &CollaborationRecordV1) -> Result<(String, String), CoordinatorError> {
    let packet_body = record
        .packet
        .as_ref()
        .and_then(|packet| packet.body.clone())
        .ok_or_else(|| corrupt(record))?;
    let objective = record
        .intent
        .objective
        .clone()
        .ok_or_else(|| corrupt(record))?;
    Ok((packet_body, objective))
}

fn project_approval_preview(
    record: &CollaborationRecordV1,
) -> Result<ApprovalPreview, CoordinatorError> {
    let approval = record.approval.as_ref().ok_or_else(|| corrupt(record))?;
    let packet = record.packet.as_ref().ok_or_else(|| corrupt(record))?;
    Ok(ApprovalPreview {
        collaboration_id: record.collaboration_id.clone(),
        approval_id: approval.approval_id.clone(),
        expected_revision: record.revision,
        expires_at: approval.expires_at.clone(),
        mode: record.intent.mode,
        source_engine: record.source.engine,
        source_session_key: record.source.session_key.clone(),
        source_title: record.source.title.clone(),
        canonical_cwd: record
            .source
            .canonical_cwd
            .clone()
            .ok_or_else(|| corrupt(record))?,
        canonical_project_root: record
            .source
            .canonical_project_root
            .clone()
            .ok_or_else(|| corrupt(record))?,
        target_engine: record.target.engine,
        objective: record
            .intent
            .objective
            .clone()
            .ok_or_else(|| corrupt(record))?,
        packet_hash: packet.hash.clone(),
        packet_preview: packet.body.clone().ok_or_else(|| corrupt(record))?,
        packet_bytes: packet.bytes,
        context_bytes: packet.context_bytes,
        omissions: packet.omissions,
        managed_worktree: record
            .worktree
            .as_ref()
            .and_then(CollaborationWorktreeV1::managed)
            .is_some(),
    })
}

fn effect_timeout(record: &CollaborationRecordV1) -> u64 {
    match record.intent.mode {
        CollaborationMode::Consult => record
            .limits
            .consultation_timeout_ms
            .unwrap_or(crate::state::CONSULTATION_DEFAULT_TIMEOUT_MS),
        CollaborationMode::Handoff => record
            .limits
            .handoff_prepare_timeout_ms
            .unwrap_or(HANDOFF_PREPARE_TIMEOUT_MS),
    }
}

fn reserve_effect(
    record: &CollaborationRecordV1,
    kind: CollaborationEffectKind,
    at: &str,
    timeout_ms: u64,
) -> Result<CollaborationEffectRecordV1, CoordinatorError> {
    let effect = CollaborationEffectRecordV1 {
        version: 1,
        effect_id: deterministic_id("effect", &[&record.collaboration_id, kind.as_str()]),
        kind,
        status: CollaborationEffectStatus::Reserved,
        attempts: 0,
        created_at: at.to_owned(),
        deadline_at: add_milliseconds(at, timeout_ms)?,
        started_at: None,
        settled_at: None,
        provider_effect_id: None,
        error_code: None,
    };
    effect.validate()?;
    Ok(effect)
}

fn failure(code: CollaborationErrorCode, at: &str, retryable: bool) -> CollaborationFailureV1 {
    CollaborationFailureV1 {
        version: 1,
        code,
        at: at.to_owned(),
        retryable: retryable && !code.is_ambiguous(),
        ambiguous: code.is_ambiguous(),
    }
}

const fn ambiguous_code(kind: CollaborationEffectKind) -> CollaborationErrorCode {
    match kind {
        CollaborationEffectKind::ParkSource => CollaborationErrorCode::SourceParkUnknown,
        CollaborationEffectKind::DeliverResult => CollaborationErrorCode::DeliveryUnknown,
        _ => CollaborationErrorCode::ProviderOutcomeUnknown,
    }
}

fn add_milliseconds(at: &str, milliseconds: u64) -> Result<String, CoordinatorError> {
    let at = timestamp_millis(at, "timestamp")?;
    let milliseconds = i64::try_from(milliseconds)
        .map_err(|_| CoordinatorError::ingress(CollaborationErrorCode::StateCorrupt))?;
    let result = at
        .checked_add_signed(Duration::milliseconds(milliseconds))
        .ok_or_else(|| CoordinatorError::ingress(CollaborationErrorCode::StateCorrupt))?;
    Ok(canonical_timestamp(result))
}

fn later_timestamp(left: &str, right: &str) -> Result<String, CoordinatorError> {
    let left_time = timestamp_millis(left, "timestamp")?;
    let right_time = timestamp_millis(right, "timestamp")?;
    Ok(if left_time < right_time {
        right.to_owned()
    } else {
        left.to_owned()
    })
}

fn deterministic_id(domain: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            digest.update([0]);
        }
        digest.update(part.as_bytes());
    }
    format!("{domain}-{:x}", digest.finalize())
}

fn bound_provider_body(value: &str, max_bytes: u64) -> String {
    let mut safe = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .map(|character| {
            let code = u32::from(character);
            if code <= 0x08
                || matches!(code, 0x0b | 0x0c)
                || (0x0e..=0x1f).contains(&code)
                || (0x7f..=0x9f).contains(&code)
            {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>()
        .nfc()
        .collect::<String>();
    let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if safe.len() <= max {
        return safe;
    }
    let marker = "\n[truncated]";
    if max <= marker.len() {
        return clip::head(marker, max).to_owned();
    }
    safe.truncate(clip::head(&safe, max - marker.len()).len());
    safe.push_str(marker);
    safe
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DispatchGuard<'a> {
    engine: CollaborationEngineId,
    session_key: &'a str,
    provider_session_id: &'a str,
    registry_revision: u64,
    runtime_epoch: &'a str,
    live_handle_generation: u64,
    canonical_cwd: &'a str,
    canonical_project_root: &'a str,
    pending_approval_set_hash: &'a str,
    pending_approval_count: u64,
    queue_revision: u64,
    queue_depth: u64,
    delivery_revision: u64,
    delivery_unknown: bool,
    worktree_owner_revision: Option<u64>,
}

pub fn source_dispatch_guard_hash(snapshot: &SourceSnapshot) -> Result<String, CoordinatorError> {
    // `to_source` validates every approval-bound field and derives the
    // managed worktree's owner revision; the rest comes from the snapshot.
    let source = snapshot.to_source()?;
    let guard = DispatchGuard {
        engine: snapshot.engine,
        session_key: &snapshot.session_key,
        provider_session_id: &snapshot.provider_session_id,
        registry_revision: snapshot.registry_revision,
        runtime_epoch: &snapshot.runtime_epoch,
        live_handle_generation: snapshot.live_handle_generation,
        canonical_cwd: &snapshot.canonical_cwd,
        canonical_project_root: &snapshot.canonical_project_root,
        pending_approval_set_hash: &snapshot.pending_approval_set_hash,
        pending_approval_count: snapshot.pending_approval_count,
        queue_revision: snapshot.queue_revision,
        queue_depth: snapshot.queue_depth,
        delivery_revision: snapshot.delivery_revision,
        delivery_unknown: snapshot.delivery_unknown,
        worktree_owner_revision: source.worktree_owner_revision,
    };
    let json = serde_json::to_string(&guard)
        .map_err(|_| CoordinatorError::ingress(CollaborationErrorCode::StateCorrupt))?;
    Ok(sha256(&json))
}

#[cfg(test)]
mod tests {
    use super::{UnicodeNormalization, bound_provider_body, clip};

    #[test]
    fn provider_body_matches_legacy_nfc_normalization_before_byte_bounding() {
        assert_eq!(bound_provider_body("Cafe\u{301}", 64), "Caf\u{e9}");
        assert_eq!(bound_provider_body("ok\u{0001}body", 64), "ok\u{fffd}body");
    }

    /// `bound_provider_body` as it was before byte clipping moved to
    /// `crate::clip`: it rebuilt the kept prefix one character at a time.
    fn legacy_bound_provider_body(value: &str, max_bytes: u64) -> String {
        let safe = value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .map(|character| {
                let code = u32::from(character);
                if code <= 0x08
                    || matches!(code, 0x0b | 0x0c)
                    || (0x0e..=0x1f).contains(&code)
                    || (0x7f..=0x9f).contains(&code)
                {
                    '\u{fffd}'
                } else {
                    character
                }
            })
            .collect::<String>()
            .nfc()
            .collect::<String>();
        let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        if safe.len() <= max {
            return safe;
        }
        let marker = "\n[truncated]";
        if max <= marker.len() {
            return marker.chars().take(max).collect();
        }
        let budget = max - marker.len();
        let mut output = String::new();
        for character in safe.chars() {
            if output.len().saturating_add(character.len_utf8()) > budget {
                break;
            }
            output.push(character);
        }
        output.push_str(marker);
        output
    }

    #[test]
    fn provider_body_bytes_match_the_char_by_char_clipper_for_every_budget() {
        for text in clip::sample_texts() {
            // Normalization can grow the text (a C1 control becomes U+FFFD).
            let ceiling = u64::try_from(text.len() * 3 + 16).unwrap();
            for max_bytes in (0..=ceiling).chain([u64::MAX]) {
                assert_eq!(
                    bound_provider_body(&text, max_bytes),
                    legacy_bound_provider_body(&text, max_bytes),
                    "{text:?} {max_bytes}"
                );
            }
        }
    }
}
