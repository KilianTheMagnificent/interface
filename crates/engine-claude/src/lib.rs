//! Direct, provider-authenticated Claude Code transport.
//!
//! The user-owned `claude` executable remains the authentication boundary. This
//! crate never opens credential files, never shells out through a command
//! interpreter, and never puts environment values in diagnostics. It speaks the
//! CLI's streaming JSON and control protocols over a supervised child process.

mod adapter;
pub mod auth;
mod executable;
mod launch;
mod mapper;
mod protocol;
mod store;
mod transport;

pub use adapter::{
    ClaudeAdapter, FolderTrust, VOICE_MCP_SERVER_NAME, VOICE_PROMPT_APPEND, VOICE_READ_ONLY_TOOLS,
    VoiceMcpProvider,
};
pub use executable::{CLAUDE_EXECUTABLE_ENV, resolve_claude_executable};
pub use launch::McpServer;
pub use store::{session_exists, session_id_from_transcript_path};

// Crate-internal seams: only interface-host and interface-runtime use this
// crate, through the items above, so the rest stays visible to dead_code.
pub(crate) use launch::{
    Initialization, LaunchOptions, PermissionMode, SessionTarget, SettingSource,
};
pub(crate) use protocol::HostRequest;
pub(crate) use transport::{
    ClaudeEvent, ClaudeSession, DeliveryCertainty, DeliveryError, HostResponse, LaunchError,
    ReceiveError, RequestError,
};

pub mod ship_text;
pub mod title;
pub mod worker;
