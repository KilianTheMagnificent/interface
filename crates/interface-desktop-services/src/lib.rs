//! Transport-neutral parity adapter for Interface's existing desktop services.
//!
//! The retained Electron preload converts its positional calls into strict,
//! named JSON objects. This crate validates those objects and delegates to the
//! Rust settings, workspace, and Review ports without knowing about Electron,
//! NDJSON, windows, or provider runtimes.

mod agent_shell;
mod checkpoint;
mod error;
mod moved_folder;
mod payload;
mod project;
mod service;

pub use checkpoint::CheckpointHooks;
pub use error::{DesktopEffectCertainty, DesktopServiceError};
// The host's background-worker adapter uses the same workspace path policy.
pub use interface_settings::{
    AgentStartAdmission, AgentTaskAdmission, AgentTaskRequest, CustomThemeLibrary,
    PhoneConnectionStore, RemoteTrustStore, TerminalReopenAuthority,
    TerminalReopenAuthorityCompareExchange, TerminalReopenAuthoritySlot, TerminalTitleStore,
    agent_home::agents_root, custom_themes_directory,
};
pub use interface_workspace::delegation as delegate_workspace;
pub use interface_workspace::output_preview::{
    read_output_preview, read_output_preview_with_files,
};
pub use service::{
    AGENT_WORK_PROJECTION_FAILED, AgentThreadLaunch, COVERED_OPERATIONS, DesktopService,
    DesktopServiceConfig, DesktopServiceReviewPorts, FolderTrustDecision, FolderTrustState,
    INTERNAL_DESKTOP_OPERATIONS, NATIVE_DIRECTORY_PICKER_START_PATH,
    SavedTerminalProviderSessionPersistence, TerminalReopenAuthorityState,
};

pub mod agent_policy;

pub use interface_workspace::CustomizationRoots;
