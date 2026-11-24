//! Codex app-server foundations.
//!
//! The provider process is launched with an exact argv vector and spoken to
//! over newline-delimited JSON-RPC. This crate deliberately exposes transport
//! and state-machine seams so tests can use fake servers without contacting a
//! live or paid provider.

pub mod adapter;
pub mod auth;
pub mod executable;
pub mod mapper;
pub mod protocol;
pub mod transport;

pub use adapter::{CodexAdapter, CodexAdapterConfig};

// Root re-exports cover what other crates use (the host, and the host's live
// agent gate); every other seam stays reachable through its module.
pub use executable::{
    CODEX_EXECUTABLE_ENV, GATED_DISABLED_FEATURES, codex_app_server_process_with_collaboration,
    codex_gated_feature_args, resolve_codex_executable,
};
pub use protocol::{CodexMethod, InitializeParams, ThreadStartParams};
pub use transport::{CodexTransport, CodexTransportLimits};

pub mod worker;
