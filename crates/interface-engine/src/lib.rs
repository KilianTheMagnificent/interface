//! Provider-neutral runtime contracts for engine adapters.
//!
//! Persisted and IPC-facing data shapes live in `interface-contracts`. This
//! crate owns the behavioral boundary: lifecycle gates, bounded event delivery,
//! provider-effect certainty, executable process specifications and the
//! shared `--version` probe, and the newline-delimited JSON-RPC transport
//! shared by native adapters.

pub mod adapter;
pub mod blocking;
pub mod child_output;
pub mod event_budget;
pub mod event_stream;
pub mod executable;
pub mod jsonrpc;
pub mod lifecycle;
pub mod outcome;
pub mod session_core;
pub mod shell_words;
pub mod supervised;
pub mod team_tools;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod thinking;

pub use adapter::{
    ApprovalResponse, ApprovalVerdict, EngineAdapter, ForkBoundary, HistoryRequest,
    ProviderCommandRequest, ProviderCommandResult, ReadySession, SessionHandle, prompt_digest,
};
pub use event_stream::{
    BoundedEventStream, EventSink, EventStreamError, EventStreamSendError, bounded_event_stream,
    bounded_event_stream_with_budget,
};
pub use executable::{
    CliAnswer, ExecutablePath, ExecutableResolution, ExecutableSource, FileVersion, ProcessSpec,
    ResolvedExecutable, VersionCache, augment_search_path, cli_query, cli_query_with_environment,
    cli_version, provider_search_path, resolve_executable,
};
pub use lifecycle::{LifecycleError, SessionLifecycle, SessionPhase};
pub use outcome::{DeliveryOutcome, EffectCertainty, EngineError, EngineErrorKind};
pub use thinking::ThinkingRepeats;

pub mod worker;
