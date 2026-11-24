//! Bounded owner-private orchestration control-wire contracts.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::orchestration::{
    AgentCapabilityV1, AgentNodeState, AgentNodeV1, AgentScopeV1, MailboxMessageV1,
    MessageDeliveryState, MessageOrigin, ModelCatalogueMode, OrchestrationCapabilityFacet,
    OrchestrationEventV1, OrchestratorAccessCeiling, OrchestratorAutonomy, OrchestratorIntegration,
    OrchestratorPolicyV1, RoleClaimV1, WorktreeLeaseV1,
};

pub const ORCHESTRATION_CONTROL_VERSION: u8 = 1;
pub const ORCHESTRATION_CONTROL_MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OrchestrationControlMethod {
    #[serde(rename = "host.ping")]
    HostPing,
    #[serde(rename = "orchestration.snapshot")]
    OrchestrationSnapshot,
    #[serde(rename = "orchestration.apply")]
    OrchestrationApply,
    #[serde(rename = "agent.get")]
    AgentGet,
    #[serde(rename = "agent.list")]
    AgentList,
    #[serde(rename = "agent.create")]
    AgentCreate,
    #[serde(rename = "agent.send")]
    AgentSend,
    #[serde(rename = "agent.reply")]
    AgentReply,
    #[serde(rename = "agent.transcript")]
    AgentTranscript,
    #[serde(rename = "agent.inbox")]
    AgentInbox,
    #[serde(rename = "agent.stop")]
    AgentStop,
    #[serde(rename = "agent.steer")]
    AgentSteer,
    #[serde(rename = "orchestrator.policy.set")]
    OrchestratorPolicySet,
    #[serde(rename = "orchestrator.policy.clear")]
    OrchestratorPolicyClear,
    #[serde(rename = "agent.bind-parent")]
    AgentBindParent,
    #[serde(rename = "agent.register-session")]
    AgentRegisterSession,
    #[serde(rename = "role.claim")]
    RoleClaim,
    #[serde(rename = "role.relinquish")]
    RoleRelinquish,
    #[serde(rename = "worktree.escrow")]
    WorktreeEscrow,
    #[serde(rename = "worker.submit")]
    WorkerSubmit,
    #[serde(rename = "worker.integrate")]
    WorkerIntegrate,
    #[serde(rename = "worker.abandon")]
    WorkerAbandon,
    #[serde(rename = "worker.ack")]
    WorkerAck,
    #[serde(rename = "coordination.status")]
    CoordinationStatus,
    #[serde(rename = "orchestrator.models")]
    OrchestratorModels,
    #[serde(rename = "adversarial-review.start")]
    AdversarialReviewStart,
    #[serde(rename = "events.since")]
    EventsSince,
    /// Protocol 62 (T3 section 7): `iface trigger` runs a thread's scheduled
    /// prompt now. Served only by a desktop host that owns `host.sock`.
    #[serde(rename = "thread.schedule.trigger")]
    ThreadScheduleTrigger,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationControlErrorCode {
    InvalidRequest,
    Unavailable,
    Conflict,
    NotFound,
    Blocked,
    UnknownEffect,
    UnsafeStorage,
    Timeout,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationControlRequest {
    pub version: u8,
    pub id: String,
    pub method: OrchestrationControlMethod,
    pub params: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationControlErrorBody {
    pub code: OrchestrationControlErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OrchestrationControlResponsePayload {
    Result {
        result: Value,
    },
    Error {
        error: OrchestrationControlErrorBody,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OrchestrationControlResponse {
    pub version: u8,
    pub id: String,
    #[serde(flatten)]
    pub payload: OrchestrationControlResponsePayload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentCreateRequestV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub engine_id: String,
    pub name: String,
    pub prompt: String,
    pub canonical_cwd: String,
    pub access_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_capabilities: Option<Vec<OrchestrationCapabilityFacet>>,
    pub client_request_id: String,
}

/// Starts one bounded cross-model builder/reviewer workflow.
///
/// The host currently accepts only the Claude/Codex pair in either role. The
/// client request identity is durable: replaying the same payload returns the
/// same agents, while reusing it for a changed payload is a conflict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdversarialReviewStartRequestV1 {
    pub builder_engine_id: String,
    pub reviewer_engine_id: String,
    pub canonical_cwd: String,
    pub objective: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdversarialReviewStartV1 {
    pub version: u8,
    pub builder: OrchestrationControlAgentV1,
    pub reviewer: OrchestrationControlAgentV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentSendRequestV1 {
    pub recipient_agent_id: String,
    pub body: String,
    pub expect_reply: bool,
    pub client_request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorPolicySetRequestV1 {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<OrchestratorAutonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_access_mode: Option<OrchestratorAccessCeiling>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_workers: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delegations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<OrchestratorIntegration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordination_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers_write: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_unverified_fence: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalogue: Option<ModelCatalogueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_allowlist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_notes: Option<String>,
    pub client_request_id: String,
}

macro_rules! id_request {
    ($name:ident, $field:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name {
            pub $field: String,
            pub client_request_id: String,
        }
    };
}

id_request!(OrchestratorPolicyClearRequestV1, agent_id);
id_request!(WorkerSubmitRequestV1, agent_id);
id_request!(WorkerAbandonRequestV1, agent_id);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentBindParentRequestV1 {
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentRegisterSessionRequestV1 {
    pub session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationControlAgentV1 {
    pub version: u8,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub engine_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    pub runtime_epoch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub depth: u64,
    pub state: AgentNodeState,
    pub capabilities: Vec<AgentCapabilityV1>,
    pub scope: AgentScopeV1,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&AgentNodeV1> for OrchestrationControlAgentV1 {
    fn from(agent: &AgentNodeV1) -> Self {
        Self {
            version: agent.version,
            agent_id: agent.agent_id.clone(),
            name: agent.name.clone(),
            engine_id: agent.engine_id.clone(),
            session_key: agent.session_key.clone(),
            runtime_epoch: agent.runtime_epoch.clone(),
            parent_agent_id: agent.parent_agent_id.clone(),
            role: agent.role.clone(),
            depth: agent.depth,
            state: agent.state,
            capabilities: agent.capabilities.clone(),
            scope: agent.scope.clone(),
            created_at: agent.created_at.clone(),
            updated_at: agent.updated_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationControlMessageSummaryV1 {
    pub version: u8,
    pub message_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
    pub origin: MessageOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent_id: Option<String>,
    pub recipient_agent_id: String,
    pub expect_reply: bool,
    pub delivery_state: MessageDeliveryState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationProjectionOmittedV1 {
    pub agents: u64,
    pub messages: u64,
    pub role_claims: u64,
    pub worktree_leases: u64,
    pub orchestrator_policies: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationSnapshotProjectionV1 {
    pub version: u8,
    pub truncated: bool,
    pub event_floor_sequence: u64,
    pub omitted: OrchestrationProjectionOmittedV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationControlSnapshotV1 {
    pub version: u8,
    pub host_id: String,
    pub runtime_epoch: String,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub agents: Vec<OrchestrationControlAgentV1>,
    pub messages: Vec<OrchestrationControlMessageSummaryV1>,
    pub role_claims: Vec<RoleClaimV1>,
    pub worktree_leases: Vec<WorktreeLeaseV1>,
    pub orchestrator_policies: Vec<OrchestratorPolicyV1>,
    pub projection: OrchestrationSnapshotProjectionV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationInboxPageV1 {
    pub version: u8,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before_message_id: Option<String>,
    pub has_more: bool,
    pub messages: Vec<MailboxMessageV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationControlEventV1 {
    pub version: u8,
    pub sequence: u64,
    pub kind: String,
    pub entity_kind: crate::orchestration::OrchestrationEntityKind,
    pub entity_id: String,
    pub at: String,
    pub summary: String,
}

impl From<&OrchestrationEventV1> for OrchestrationControlEventV1 {
    fn from(event: &OrchestrationEventV1) -> Self {
        Self {
            version: event.version,
            sequence: event.sequence,
            kind: event.kind.clone(),
            entity_kind: event.entity_kind,
            entity_id: event.entity_id.clone(),
            at: event.at.clone(),
            summary: event.summary.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationEventPageV1 {
    pub version: u8,
    pub requested_sequence: u64,
    pub floor_sequence: u64,
    pub next_sequence: u64,
    pub truncated: bool,
    pub has_more: bool,
    pub events: Vec<OrchestrationControlEventV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentPageV1 {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after_agent_id: Option<String>,
    pub has_more: bool,
    pub agents: Vec<OrchestrationControlAgentV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationTranscriptPageV1 {
    pub version: u8,
    pub agent_id: String,
    pub requested_lines: u64,
    pub truncated: bool,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorSubmitResultV1 {
    pub submission_id: String,
    pub changes: u64,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerIntegrateRequestV1 {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerAckRequestV1 {
    pub agent_id: String,
    pub keep: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationStatusRequestV1 {
    pub canonical_project_root: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationStatusV1 {
    pub registered: bool,
    pub active_tasks: u64,
    pub published_uncommitted_paths: Vec<String>,
    pub required_checks: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorModelRowV1 {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorEngineModelsV1 {
    pub engine_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub models: Vec<OrchestratorModelRowV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorSyntheticModelsV1 {
    pub ultracode: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrchestratorCatalogueState {
    Live,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorModelsV1 {
    pub engines: Vec<OrchestratorEngineModelsV1>,
    pub synthetic: OrchestratorSyntheticModelsV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowlist: Option<Vec<String>>,
    pub catalogue: OrchestratorCatalogueState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrchestrationControlError {
    pub code: OrchestrationControlErrorCode,
    pub message: String,
    pub data: Option<Value>,
}

impl fmt::Display for OrchestrationControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OrchestrationControlError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_wire_bytes_stay_frozen() {
        // The host decodes this shape straight off the socket
        // (apps/interface-host/src/orchestration_transport.rs), so the field
        // order, the camelCase params and the method token are frozen.
        let request = OrchestrationControlRequest {
            version: 1,
            id: "request-1".into(),
            method: OrchestrationControlMethod::AgentBindParent,
            params: Map::from_iter([("agentId".into(), json!("agent-1"))]),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"version":1,"id":"request-1","method":"agent.bind-parent","params":{"agentId":"agent-1"}}"#
        );
        assert_eq!(
            serde_json::from_str::<OrchestrationControlRequest>(
                r#"{"version":1,"id":"request-1","method":"agent.bind-parent","params":{"agentId":"agent-1"}}"#
            )
            .unwrap(),
            request
        );
    }

    #[test]
    fn retired_teammate_methods_are_not_in_the_protocol() {
        assert!(
            serde_json::from_value::<OrchestrationControlMethod>(json!("teammate.create")).is_err()
        );
    }
}
