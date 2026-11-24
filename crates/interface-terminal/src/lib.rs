//! External launch parity and the production integrated-terminal core.
//!
//! The legacy operation opens an approved workspace root in Ghostty or
//! Terminal.app. The embedded service is a distinct, long-lived PTY registry;
//! callers must choose explicitly between the two operations.

mod antigravity_names;
mod attention;
pub use attention::AttentionMonitor;
mod codex_names;
mod embedded;
mod error;
mod external;
mod fast_mode;
mod foreground_profile;
mod grok_names;
pub use fast_mode::{CODEX_FAST_MODE_KEY_CONFIG, CODEX_TERMINAL_TITLE_CONFIG};
mod launch;
pub mod native_checkpoint;
mod opencode_activity;
mod opencode_session;
pub use opencode_session::{OPENCODE_SESSION_RECEIPT_FILE, opencode_session_receipt};
mod osc_title;
pub mod pane_tools;
pub mod provider_canary;
pub use opencode_activity::{OPENCODE_ACTIVITY_TRANSPORT, TERMINAL_ACTIVITY_PREFIX};
mod proc_ancestry;
pub use proc_ancestry::ProcessListing;
mod process_probe;
pub use process_probe::PsProbe;
mod proc_cwd;
mod provider_files;
mod safety;
mod screen;
mod service;
pub mod shell_handoff;
mod startup_files;
pub use startup_files::{
    SystemTerminalStartupFiles, TerminalStartupFileProbe, TerminalStartupFiles,
};
mod task_title;

#[cfg(unix)]
pub use embedded::process_exists;
pub use embedded::{
    SpawnedTerminal, SystemTerminalPtyBackend, TerminalInput, TerminalProcessControl,
    TerminalProcessIdentity, TerminalProcessWaiter, TerminalPtyBackend, TerminalSpawnSpec,
    captured_process_group_is_live, current_terminal_process_identity,
    settle_stopped_orphan_process_group,
};
pub use error::{TerminalBackendError, TerminalError};
pub use external::{
    ExternalTerminalBackend, SystemExternalTerminalBackend, open_external_terminal,
};
pub use launch::{
    CLAUDE_ACTIVITY_TITLE_PREFIX, CLAUDE_SESSION_BINDING_TITLE_PREFIX,
    CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, CLAUDE_SESSION_HOOK_MODE,
    CODEX_SESSION_CAPTURE_TOKEN_ENV, CODEX_SESSION_HOOK_MODE, ResolvedTerminalLaunch,
    TerminalAccountLaunch, TerminalAccounts, TerminalLaunchCatalog, TerminalLaunchCatalogError,
    TerminalLaunchProgram, TerminalLaunchProgramError, install_hint, profile_label,
    sign_in_arguments,
};
pub use safety::{ApprovedTerminalRootError, ApprovedTerminalRoots};
pub use service::{
    TerminalLeaseRequest, TerminalLifecycleSink, TerminalOutputChunk, TerminalOutputRead,
    TerminalOutputReader, TerminalProviderSession, TerminalProviderSessionCaptureOwner,
    TerminalProviderSessionCaptureProcess, TerminalProviderSessionLookup,
    TerminalProviderSessionPersistence, TerminalProviderSessionTransition, TerminalService,
    TerminalServiceConfig, TerminalServiceParts, TerminalTitlePersistence, TerminalWorkspaceLease,
    TerminalWorkspaceLeaseProvider,
};

mod workspace_tools;
pub use workspace_tools::{
    SystemWorkspaceToolLauncher, WorkspaceToolError, WorkspaceToolLauncher, open_workspace_tool,
};
