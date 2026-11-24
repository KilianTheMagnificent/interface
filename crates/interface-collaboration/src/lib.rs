//! Durable, fail-closed collaboration state and coordination.
//!
//! It owns the wire contract, authority checks, durable journal/audit trail,
//! idempotency, effect-attempt state machine, and the owner-private MCP broker
//! transport consumed by provider adapters. Provider process launches remain
//! in their engine crates.

// The public surface intentionally mirrors the legacy TypeScript contract.
// Keeping its fallible value-oriented entry points and explicit state-machine
// methods is more important here than applying API-shape preferences from
// Clippy's pedantic group.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unused_self
)]

pub mod audit;
pub mod away;
pub mod away_cycle;
pub mod broker;
pub mod broker_runtime;
mod clip;
pub mod coordinator;
pub mod delegates;
#[cfg(test)]
mod golden;
pub mod journal;
pub mod mcp;
pub mod packet;
pub mod project_away;
mod snapshot_file;
pub mod state;
pub mod thread_handoff;
pub mod workers;

/// The identifier rule project Away and the workers broker share: 1 to 80
/// bytes of ASCII letters, digits, `-` and `_`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

pub use audit::{
    AuditCounters, AuditDistribution, AuditError, CollaborationAuditSink, aggregate_audit_counters,
};
pub use broker::{
    BrokerDepthFence, BrokerError, BrokerRuntime, BrokerSessionBinding, BrokerSourceRegistration,
    CollaborationModelAuthority, collaboration_source_lineage_id,
};
pub use broker_runtime::{
    CollaborationBrokerRuntime, CollaborationBrokerRuntimeError, CollaborationBrokerRuntimeOptions,
};
pub use coordinator::{
    ApprovalAction, ApprovalPreview, CollaborationApprovalAction, CollaborationCoordinator,
    CollaborationPeerAction, CoordinatorError, EffectCommand, EffectOutcome, EffectSuccess,
    LockedSession, LockedSessionRole, PacketBuildResult, PeerAction, PendingPeer, PendingPeerState,
    PreparedTarget, SessionParticipants, SourceSnapshot, source_dispatch_guard_hash,
};
pub use journal::{CollaborationJournal, JournalCreateResult, JournalError, JournalErrorCode};
pub use mcp::{
    CollaborationMcpAccessRequest, CollaborationMcpIntent, CollaborationMcpRequestIngress,
    CollaborationMcpSocket, CollaborationMcpSocketOptions, CollaborationModelIngress,
    IngressCancellation, McpIngressError, McpIngressErrorCode, collaboration_mcp_tools,
};
pub use state::*;

pub mod delegate_quality;

#[cfg(test)]
mod tests {
    use super::valid_id;

    #[test]
    fn ids_are_short_ascii_words() {
        assert!(valid_id("away-run_1"));
        assert!(valid_id(&"a".repeat(80)));
        for invalid in ["", "has space", "slash/id", "é", &"a".repeat(81)] {
            assert!(!valid_id(invalid), "{invalid:?}");
        }
    }
}
