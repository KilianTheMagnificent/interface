//! Agent Client Protocol foundations shared by Grok, Qwen, and custom profiles.
//!
//! This crate contains provider-neutral ACP v1 framing and strict argv profile
//! validation plus the frozen built-in Grok/Qwen bindings. Qwen's local MLX
//! backend is owner-managed and never applies to custom ACP profiles.

pub mod adapter;
mod grok_store;
pub mod mapper;
pub mod opencode_usage;
pub mod opencode_worker;
pub mod opencode_worker_metrics;
pub mod profile;
pub mod protocol;
pub mod qwen;
pub mod transport;

pub use adapter::{AcpAdapter, AcpAdapterConfig, AcpProviderFlavor};
pub use profile::{AcpProfile, BuiltinAcpProvider, resolve_builtin_executable, validate_profile};
