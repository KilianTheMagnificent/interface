//! Native service integration for Interface engine adapters.
//!
//! This crate owns the one-consumer provider event streams, persists them
//! before publishing them to desktop or CLI consumers, and retains the exact
//! delivery certainty returned by each provider. Provider-specific wire
//! protocols remain in their engine crates; durable plan and orchestration
//! models remain in their canonical crates.

pub mod accounts;
mod bridge;
mod claims;
mod error;
mod history;
mod management;
mod project_root;
mod runtime;

pub use bridge::{
    BridgeApprovalResponse, BridgeApprovalVerdict, RuntimeBridgeCommand, RuntimeBridgeResponse,
    RuntimeDeliveryDisposition, RuntimeDeliveryReceipt, RuntimeSendLiveResponse,
};
pub use claims::{HANDOFF_HOLD, SessionClaim, SessionClaims, TerminalSessionBindings};
pub use error::{RuntimeError, RuntimeErrorKind};
pub use history::{
    EffectAuditDisposition, EngineHistoryStore, HistoryEffect, HistoryEvent, HistoryLimits,
    HistorySession, HistorySessionState, RuntimeDeliveryState, RuntimeEffectKind,
    RuntimeHistorySnapshot, RuntimeStreamState,
};
pub use management::{
    IMPORT_HISTORY_LIMIT, ImportProgress, ImportProgressSink, ImportReport, RuntimeSendLiveOutcome,
};
pub use runtime::{
    NativeAdapterConfiguration, ProviderReadiness, ProviderSessionRef, RuntimeClock,
    RuntimeConfiguration, RuntimeDelivery, RuntimeDeliveryAudit, RuntimeEvent, RuntimeEventFrame,
    RuntimeEventTermination, RuntimeServices, RuntimeSession, SessionOpenOutcome, SessionRuntime,
    SessionRuntimeBuilder, SystemRuntimeClock,
};
