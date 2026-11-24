//! Contract constants, enums and the wire/persisted collaboration structs.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::CollaborationEngineId;

pub const COLLABORATION_CONTRACT_VERSION: u8 = 1;
pub const COLLABORATION_PACKET_VERSION: u8 = 2;
/// Largest integer that round-trips through the legacy JavaScript wire format.
pub const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub const OBJECTIVE_CHARACTERS_MAX: usize = 4_000;
pub const OBJECTIVE_BYTES_MAX: usize = 12 * 1_024;
pub const SOURCE_SESSION_KEY_BYTES_MAX: usize = 1_024;
pub const PROVIDER_SESSION_ID_BYTES_MAX: usize = 512;
pub const SOURCE_TITLE_BYTES_MAX: usize = 1_024;
pub const PATH_BYTES_MAX: usize = 4 * 1_024;
pub const PACKET_BYTES_MAX: u64 = 32 * 1_024;
pub const CONTEXT_BYTES_MAX: u64 = 24 * 1_024;
pub const RESULT_BYTES_MAX: u64 = 24 * 1_024;
pub const CLIENT_REQUEST_ID_BYTES_MAX: usize = 256;
pub const ORIGIN_CAPABILITY_BYTES_MAX: usize = 512;
pub const IDENTIFIER_BYTES_MAX: usize = 512;
pub const SAFE_NEXT_ACTION_BYTES_MAX: usize = 1_024;
pub const MAX_DEPTH: u8 = 1;
pub const MAX_ACTIVE_CONSULTATIONS_PER_SOURCE: u64 = 2;
pub const MAX_ACTIVE_HANDOFFS_PER_SOURCE: u64 = 1;
pub const MAX_ACTIVE_HANDOFFS_PER_WORKTREE: u64 = 1;
pub const MAX_RUNNING_TARGETS: u64 = 4;
pub const APPROVAL_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
pub const REQUEST_TOOL_TIMEOUT_MS: u64 = 2_000;
pub const CONSULTATION_DEFAULT_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
pub const CONSULTATION_HARD_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
pub const HANDOFF_PREPARE_TIMEOUT_MS: u64 = 60 * 1_000;
pub const MAX_TERMINAL_RECEIPTS: u64 = 1_000;
pub const TERMINAL_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
pub const TRANSIENT_BODY_CRASH_BACKSTOP_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationMode {
    Consult,
    Handoff,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationOrigin {
    Human,
    Model,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationState {
    Requested,
    AwaitingApproval,
    Approved,
    Preflight,
    PreparingTarget,
    RunningTarget,
    ResultReady,
    AwaitingPeerReview,
    DeliveryPending,
    ParkingSource,
    OwnershipTransfer,
    ActivatingTarget,
    Completed,
    Denied,
    Cancelled,
    Failed,
    Unknown,
}

impl CollaborationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Approved => "approved",
            Self::Preflight => "preflight",
            Self::PreparingTarget => "preparing_target",
            Self::RunningTarget => "running_target",
            Self::ResultReady => "result_ready",
            Self::AwaitingPeerReview => "awaiting_peer_review",
            Self::DeliveryPending => "delivery_pending",
            Self::ParkingSource => "parking_source",
            Self::OwnershipTransfer => "ownership_transfer",
            Self::ActivatingTarget => "activating_target",
            Self::Completed => "completed",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Denied | Self::Cancelled | Self::Failed
        )
    }
}

impl fmt::Display for CollaborationState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub const COLLABORATION_STATES: &[CollaborationState] = &[
    CollaborationState::Requested,
    CollaborationState::AwaitingApproval,
    CollaborationState::Approved,
    CollaborationState::Preflight,
    CollaborationState::PreparingTarget,
    CollaborationState::RunningTarget,
    CollaborationState::ResultReady,
    CollaborationState::AwaitingPeerReview,
    CollaborationState::DeliveryPending,
    CollaborationState::ParkingSource,
    CollaborationState::OwnershipTransfer,
    CollaborationState::ActivatingTarget,
    CollaborationState::Completed,
    CollaborationState::Denied,
    CollaborationState::Cancelled,
    CollaborationState::Failed,
    CollaborationState::Unknown,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CollaborationErrorCode {
    SourceNotFound,
    SourceNotLive,
    SourceNotIdle,
    TargetNotSupported,
    TargetUnhealthy,
    ApprovalRequired,
    ApprovalExpired,
    ApprovalStale,
    Denied,
    PendingSourceApproval,
    SourceQueueNotEmpty,
    SourceDeliveryUnknown,
    ManagedWorktreeTransferUnavailable,
    WorktreeOwnerChanged,
    IdempotencyConflict,
    LimitExceeded,
    BrokerUnavailable,
    ProviderOutcomeUnknown,
    SourceParkUnknown,
    DeliveryUnknown,
    StateCorrupt,
    RecoveryRequired,
}

impl CollaborationErrorCode {
    #[must_use]
    pub const fn safe_next_action(self) -> &'static str {
        match self {
            Self::SourceNotFound => "Choose an existing source session and submit a new request.",
            Self::SourceNotLive => "Open the source session and submit a new request.",
            Self::SourceNotIdle => {
                "Wait for the source session to become idle, then submit a new request."
            }
            Self::TargetNotSupported => {
                "Choose a target provider that supports this collaboration mode."
            }
            Self::TargetUnhealthy => "Restore the target provider, then submit a new request.",
            Self::ApprovalRequired => "Review the collaboration preview in Interface.",
            Self::ApprovalExpired => "Create and review a fresh collaboration preview.",
            Self::ApprovalStale => "Review a fresh preview bound to the current source state.",
            Self::Denied => "Submit a new request only if the collaboration is still wanted.",
            Self::PendingSourceApproval => {
                "Resolve the source session approval before trying again."
            }
            Self::SourceQueueNotEmpty => "Resolve the source delivery queue before trying again.",
            Self::SourceDeliveryUnknown | Self::DeliveryUnknown => {
                "Inspect the source session and reconcile delivery in Interface."
            }
            Self::ManagedWorktreeTransferUnavailable => {
                "Use an ordinary checkout or wait for managed ownership transfer support."
            }
            Self::WorktreeOwnerChanged => {
                "Review the current worktree owner and create a fresh request."
            }
            Self::IdempotencyConflict => {
                "Reuse the request ID only with its original normalized payload."
            }
            Self::LimitExceeded => {
                "Wait for an active collaboration to finish or reduce the request size."
            }
            Self::BrokerUnavailable => {
                "Restore the Interface collaboration broker, then try again."
            }
            Self::ProviderOutcomeUnknown => {
                "Inspect the target provider and reconcile the outcome in Interface."
            }
            Self::SourceParkUnknown => {
                "Inspect the source session and reconcile parking in Interface."
            }
            Self::StateCorrupt => {
                "Preserve the record and repair collaboration state before continuing."
            }
            Self::RecoveryRequired => {
                "Inspect local collaboration state and reconcile it in Interface."
            }
        }
    }

    #[must_use]
    pub const fn is_ambiguous(self) -> bool {
        matches!(
            self,
            Self::ProviderOutcomeUnknown
                | Self::SourceParkUnknown
                | Self::DeliveryUnknown
                | Self::RecoveryRequired
        )
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceNotFound => "SOURCE_NOT_FOUND",
            Self::SourceNotLive => "SOURCE_NOT_LIVE",
            Self::SourceNotIdle => "SOURCE_NOT_IDLE",
            Self::TargetNotSupported => "TARGET_NOT_SUPPORTED",
            Self::TargetUnhealthy => "TARGET_UNHEALTHY",
            Self::ApprovalRequired => "APPROVAL_REQUIRED",
            Self::ApprovalExpired => "APPROVAL_EXPIRED",
            Self::ApprovalStale => "APPROVAL_STALE",
            Self::Denied => "DENIED",
            Self::PendingSourceApproval => "PENDING_SOURCE_APPROVAL",
            Self::SourceQueueNotEmpty => "SOURCE_QUEUE_NOT_EMPTY",
            Self::SourceDeliveryUnknown => "SOURCE_DELIVERY_UNKNOWN",
            Self::ManagedWorktreeTransferUnavailable => "MANAGED_WORKTREE_TRANSFER_UNAVAILABLE",
            Self::WorktreeOwnerChanged => "WORKTREE_OWNER_CHANGED",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::LimitExceeded => "LIMIT_EXCEEDED",
            Self::BrokerUnavailable => "BROKER_UNAVAILABLE",
            Self::ProviderOutcomeUnknown => "PROVIDER_OUTCOME_UNKNOWN",
            Self::SourceParkUnknown => "SOURCE_PARK_UNKNOWN",
            Self::DeliveryUnknown => "DELIVERY_UNKNOWN",
            Self::StateCorrupt => "STATE_CORRUPT",
            Self::RecoveryRequired => "RECOVERY_REQUIRED",
        }
    }
}

pub const COLLABORATION_ERROR_CODES: &[CollaborationErrorCode] = &[
    CollaborationErrorCode::SourceNotFound,
    CollaborationErrorCode::SourceNotLive,
    CollaborationErrorCode::SourceNotIdle,
    CollaborationErrorCode::TargetNotSupported,
    CollaborationErrorCode::TargetUnhealthy,
    CollaborationErrorCode::ApprovalRequired,
    CollaborationErrorCode::ApprovalExpired,
    CollaborationErrorCode::ApprovalStale,
    CollaborationErrorCode::Denied,
    CollaborationErrorCode::PendingSourceApproval,
    CollaborationErrorCode::SourceQueueNotEmpty,
    CollaborationErrorCode::SourceDeliveryUnknown,
    CollaborationErrorCode::ManagedWorktreeTransferUnavailable,
    CollaborationErrorCode::WorktreeOwnerChanged,
    CollaborationErrorCode::IdempotencyConflict,
    CollaborationErrorCode::LimitExceeded,
    CollaborationErrorCode::BrokerUnavailable,
    CollaborationErrorCode::ProviderOutcomeUnknown,
    CollaborationErrorCode::SourceParkUnknown,
    CollaborationErrorCode::DeliveryUnknown,
    CollaborationErrorCode::StateCorrupt,
    CollaborationErrorCode::RecoveryRequired,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractErrorReason {
    InvalidContract,
    StaleRevision,
    StaleState,
    IllegalTransition,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{message}")]
pub struct ContractError {
    pub reason: ContractErrorReason,
    message: String,
}

impl ContractError {
    pub(super) fn invalid(path: impl fmt::Display, rule: &str) -> Self {
        Self {
            reason: ContractErrorReason::InvalidContract,
            message: format!("Invalid {path}: {rule}."),
        }
    }

    pub(super) fn transition(reason: ContractErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

pub type ContractResult<T> = Result<T, ContractError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationIntentInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    pub mode: CollaborationMode,
    pub source_session_key: String,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
    pub client_request_id: String,
    pub origin: CollaborationOrigin,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationIntentV1 {
    pub version: u8,
    pub mode: CollaborationMode,
    pub source_session_key: String,
    pub target_engine: CollaborationEngineId,
    pub objective_digest: String,
    pub objective_bytes: u64,
    pub objective_characters: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    pub client_request_id: String,
    pub origin: CollaborationOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_lineage_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSourceV1 {
    pub engine: CollaborationEngineId,
    pub session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_handle_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_status: Option<CollaborationLiveStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval_set_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_unknown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_owner_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationLiveStatus {
    Working,
    Waiting,
    Idle,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationTargetV1 {
    pub engine: CollaborationEngineId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_marker: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPacketRecordV1 {
    pub version: u8,
    pub hash: String,
    pub bytes: u64,
    pub context_bytes: u64,
    pub omissions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationApprovalDecision {
    Pending,
    Approved,
    Denied,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalRecordV1 {
    pub version: u8,
    pub approval_id: String,
    pub collaboration_id: String,
    pub packet_hash: String,
    pub source_registry_revision: u64,
    pub source_guard_hash: String,
    pub target_engine: CollaborationEngineId,
    pub created_at: String,
    pub expires_at: String,
    pub decision: CollaborationApprovalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollaborationEffectKind {
    PrepareTarget,
    RunTarget,
    ParkSource,
    TransferOwnership,
    ActivateTarget,
    DeliverResult,
}

impl CollaborationEffectKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrepareTarget => "prepare-target",
            Self::RunTarget => "run-target",
            Self::ParkSource => "park-source",
            Self::TransferOwnership => "transfer-ownership",
            Self::ActivateTarget => "activate-target",
            Self::DeliverResult => "deliver-result",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationEffectStatus {
    Reserved,
    Started,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationEffectRecordV1 {
    pub version: u8,
    pub effect_id: String,
    pub kind: CollaborationEffectKind,
    pub status: CollaborationEffectStatus,
    pub attempts: u8,
    pub created_at: String,
    pub deadline_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_effect_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<CollaborationErrorCode>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationEffectMapV1 {
    #[serde(
        rename = "prepare-target",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub prepare_target: Option<CollaborationEffectRecordV1>,
    #[serde(
        rename = "run-target",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub run_target: Option<CollaborationEffectRecordV1>,
    #[serde(
        rename = "park-source",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub park_source: Option<CollaborationEffectRecordV1>,
    #[serde(
        rename = "transfer-ownership",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub transfer_ownership: Option<CollaborationEffectRecordV1>,
    #[serde(
        rename = "activate-target",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub activate_target: Option<CollaborationEffectRecordV1>,
    #[serde(
        rename = "deliver-result",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub deliver_result: Option<CollaborationEffectRecordV1>,
}

impl CollaborationEffectMapV1 {
    #[must_use]
    pub fn get(&self, kind: CollaborationEffectKind) -> Option<&CollaborationEffectRecordV1> {
        match kind {
            CollaborationEffectKind::PrepareTarget => self.prepare_target.as_ref(),
            CollaborationEffectKind::RunTarget => self.run_target.as_ref(),
            CollaborationEffectKind::ParkSource => self.park_source.as_ref(),
            CollaborationEffectKind::TransferOwnership => self.transfer_ownership.as_ref(),
            CollaborationEffectKind::ActivateTarget => self.activate_target.as_ref(),
            CollaborationEffectKind::DeliverResult => self.deliver_result.as_ref(),
        }
    }

    pub fn set(&mut self, effect: CollaborationEffectRecordV1) {
        match effect.kind {
            CollaborationEffectKind::PrepareTarget => self.prepare_target = Some(effect),
            CollaborationEffectKind::RunTarget => self.run_target = Some(effect),
            CollaborationEffectKind::ParkSource => self.park_source = Some(effect),
            CollaborationEffectKind::TransferOwnership => self.transfer_ownership = Some(effect),
            CollaborationEffectKind::ActivateTarget => self.activate_target = Some(effect),
            CollaborationEffectKind::DeliverResult => self.deliver_result = Some(effect),
        }
    }

    #[must_use]
    pub fn has_status(
        &self,
        kind: CollaborationEffectKind,
        status: CollaborationEffectStatus,
    ) -> bool {
        self.get(kind).is_some_and(|effect| effect.status == status)
    }

    pub fn iter(&self) -> impl Iterator<Item = &CollaborationEffectRecordV1> {
        [
            self.prepare_target.as_ref(),
            self.run_target.as_ref(),
            self.park_source.as_ref(),
            self.transfer_ownership.as_ref(),
            self.activate_target.as_ref(),
            self.deliver_result.as_ref(),
        ]
        .into_iter()
        .flatten()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPeerAcceptanceV1 {
    pub version: u8,
    pub receipt_id: String,
    pub result_id: String,
    pub result_digest: String,
    pub decision: CollaborationPeerDecision,
    pub decided_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationPeerDecision {
    Accepted,
    Discarded,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollaborationPeerResultState {
    Ready,
    Held,
    Accepted,
    Discarded,
    DeliveryPending,
    Delivered,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPeerResultV1 {
    pub version: u8,
    pub result_id: String,
    pub digest: String,
    pub bytes: u64,
    pub state: CollaborationPeerResultState,
    pub created_at: String,
    pub target_session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<CollaborationPeerAcceptanceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum CollaborationWorktreeOwnerV1 {
    Source {
        #[serde(rename = "sessionKey")]
        session_key: String,
    },
    Escrow {
        #[serde(rename = "collaborationId")]
        collaboration_id: String,
    },
    Target {
        #[serde(rename = "sessionKey")]
        session_key: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CollaborationWorktreeV1 {
    Unmanaged(UnmanagedWorktreeV1),
    Managed(ManagedWorktreeV1),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnmanagedWorktreeV1 {
    pub version: u8,
    pub managed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedWorktreeV1 {
    pub version: u8,
    pub managed: bool,
    pub canonical_path: String,
    pub branch: String,
    pub owner_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_id: Option<String>,
    pub release_in_progress: bool,
    pub owner: CollaborationWorktreeOwnerV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPolicyLimitsV1 {
    pub version: u8,
    pub depth: u8,
    pub max_objective_characters: u64,
    pub max_objective_bytes: u64,
    pub max_packet_bytes: u64,
    pub max_context_bytes: u64,
    pub max_result_bytes: u64,
    pub max_active_consultations_per_source: u64,
    pub max_active_handoffs_per_source: u64,
    pub max_active_handoffs_per_worktree: u64,
    pub max_running_targets: u64,
    pub approval_timeout_ms: u64,
    pub request_tool_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consultation_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_prepare_timeout_ms: Option<u64>,
}

impl CollaborationPolicyLimitsV1 {
    #[must_use]
    pub const fn defaults(mode: CollaborationMode, depth: u8) -> Self {
        Self {
            version: 1,
            depth,
            max_objective_characters: OBJECTIVE_CHARACTERS_MAX as u64,
            max_objective_bytes: OBJECTIVE_BYTES_MAX as u64,
            max_packet_bytes: PACKET_BYTES_MAX,
            max_context_bytes: CONTEXT_BYTES_MAX,
            max_result_bytes: RESULT_BYTES_MAX,
            max_active_consultations_per_source: MAX_ACTIVE_CONSULTATIONS_PER_SOURCE,
            max_active_handoffs_per_source: MAX_ACTIVE_HANDOFFS_PER_SOURCE,
            max_active_handoffs_per_worktree: MAX_ACTIVE_HANDOFFS_PER_WORKTREE,
            max_running_targets: MAX_RUNNING_TARGETS,
            approval_timeout_ms: APPROVAL_TIMEOUT_MS,
            request_tool_timeout_ms: REQUEST_TOOL_TIMEOUT_MS,
            consultation_timeout_ms: match mode {
                CollaborationMode::Consult => Some(CONSULTATION_DEFAULT_TIMEOUT_MS),
                CollaborationMode::Handoff => None,
            },
            handoff_prepare_timeout_ms: match mode {
                CollaborationMode::Consult => None,
                CollaborationMode::Handoff => Some(HANDOFF_PREPARE_TIMEOUT_MS),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationFailureV1 {
    pub version: u8,
    pub code: CollaborationErrorCode,
    pub at: String,
    pub retryable: bool,
    pub ambiguous: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationRecordV1 {
    pub version: u8,
    pub collaboration_id: String,
    pub revision: u64,
    pub state: CollaborationState,
    pub intent: CollaborationIntentV1,
    pub dedupe_key: String,
    pub payload_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub source: CollaborationSourceV1,
    pub target: CollaborationTargetV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet: Option<CollaborationPacketRecordV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<CollaborationApprovalRecordV1>,
    pub effects: CollaborationEffectMapV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_result: Option<CollaborationPeerResultV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<CollaborationWorktreeV1>,
    pub limits: CollaborationPolicyLimitsV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CollaborationFailureV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationErrorV1 {
    pub version: u8,
    pub code: CollaborationErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_id: Option<String>,
    pub safe_next_action: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationReceiptV1 {
    pub version: u8,
    pub collaboration_id: String,
    pub revision: u64,
    pub state: CollaborationState,
    pub mode: CollaborationMode,
    pub source_engine: CollaborationEngineId,
    pub target_engine: CollaborationEngineId,
    pub approval_required: bool,
    pub cancellation_safe: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CollaborationErrorV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationAuditEventV1 {
    pub version: u8,
    pub event_id: String,
    pub collaboration_id: String,
    pub revision: u64,
    pub at: String,
    pub mode: CollaborationMode,
    pub source_engine: CollaborationEngineId,
    pub target_engine: CollaborationEngineId,
    pub from: CollaborationState,
    pub to: CollaborationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<CollaborationErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_decision: Option<AuditApprovalDecision>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditApprovalDecision {
    Approved,
    Denied,
    Expired,
}

#[derive(Clone, Debug, Default)]
pub struct CollaborationPolicyOverrides {
    pub max_objective_characters: Option<u64>,
    pub max_objective_bytes: Option<u64>,
    pub max_packet_bytes: Option<u64>,
    pub max_context_bytes: Option<u64>,
    pub max_result_bytes: Option<u64>,
    pub max_active_consultations_per_source: Option<u64>,
    pub max_active_handoffs_per_source: Option<u64>,
    pub max_active_handoffs_per_worktree: Option<u64>,
    pub max_running_targets: Option<u64>,
    pub approval_timeout_ms: Option<u64>,
    pub request_tool_timeout_ms: Option<u64>,
    pub consultation_timeout_ms: Option<u64>,
    pub handoff_prepare_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct CreateCollaborationRecordOptions {
    pub collaboration_id: String,
    pub intent: CollaborationIntentV1,
    pub created_at: String,
    pub depth: u8,
    pub limits: CollaborationPolicyOverrides,
}

#[derive(Clone, Debug, Default)]
pub struct CollaborationTransitionPatch {
    pub source: Option<CollaborationSourceV1>,
    pub target: Option<CollaborationTargetV1>,
    pub packet: Option<CollaborationPacketRecordV1>,
    pub approval: Option<CollaborationApprovalRecordV1>,
    pub effects: Option<CollaborationEffectMapV1>,
    pub peer_result: Option<CollaborationPeerResultV1>,
    pub worktree: Option<CollaborationWorktreeV1>,
    pub failure: Option<CollaborationFailureV1>,
    pub clear_failure: bool,
}

#[derive(Clone, Debug)]
pub struct CollaborationTransitionRequest {
    pub expected_revision: u64,
    pub expected_state: CollaborationState,
    pub to: CollaborationState,
    pub maintenance_effect: Option<CollaborationEffectKind>,
    pub at: String,
    pub event_id: Option<String>,
    pub patch: CollaborationTransitionPatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollaborationTransitionResult {
    pub record: CollaborationRecordV1,
    pub audit: CollaborationAuditEventV1,
}
