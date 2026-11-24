//! Bounded, transport-neutral bridge between the Electron shell and Rust core.

pub mod agents;
pub mod antigravity_usage;
pub mod bootstrap;
mod bounded_process;
mod capture_generation;
mod capture_hook;
pub mod claude_manual_wrapper;
pub mod claude_terminal_hook;
pub mod codex_terminal_hook;
pub mod collaboration;
pub mod delegation_hook;
pub mod diagnostics;
pub mod dispatcher;
pub mod folder_trust;
pub mod host_log;
pub mod instance_lock;
pub mod mcp_transport;
pub mod native;
pub mod opencode_terminal_session;
pub mod opencode_usage;
pub mod orchestration;
pub mod orchestration_transport;
pub mod outside_agents;
mod pane_delegates;
pub mod pane_mcp;
mod payload;
pub mod project_away;
pub mod prompt_context;
pub mod protocol;
pub mod provider_accounts;
pub mod provider_canary;
pub mod provider_capture;
pub mod provider_refresh;
mod registry_access;
mod registry_push;
pub mod report_workers;
pub mod router;
pub mod server;
pub mod session_access;
pub mod session_management;
pub mod terminal;
pub mod terminal_transport;
pub mod theme_watch;
pub mod threads;
pub mod updater;
pub mod voice;
pub mod worktree;
pub mod worktree_release;

pub use bootstrap::OpenedHostServices;
pub use claude_manual_wrapper::{
    manual_claude_wrapper_requested, manual_codex_wrapper_requested, run_manual_claude_wrapper,
    run_manual_codex_wrapper,
};
pub use claude_terminal_hook::run_claude_terminal_session_hook;
pub use codex_terminal_hook::run_codex_terminal_session_hook;
pub use collaboration::{
    COLLABORATION_OPERATIONS, CollaborationHostDispatcher, CollaborationSessionHooks,
    DeferredCollaborationModelIngress, DesktopCollaborationProjection, DurableCollaborationService,
    ProjectedSendOutcome, SessionRuntimeCollaborationPort,
};
pub use dispatcher::{
    DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE, OPERATION_NOT_MIGRATED_CODE,
    RUNTIME_BRIDGE_OPERATIONS, RegistryRoundObserver, RuntimeCommandHandler, RuntimeHostDispatcher,
    SendHooks, SendSettlement, SendTicket, SessionEventObserver, SharedRegistryPushes,
};
pub use instance_lock::{CliInstanceLock, acquire_cli_instance_lock};
pub use provider_capture::native::run as run_native_terminal_checkpoint_hook;
mod native_checkpoint_setup;
pub use native::{NATIVE_SERVICE_OPERATIONS, NativeHostDispatcher, NativeRuntimeView};
pub use orchestration::{
    ExternalOutcome, ORCHESTRATION_OPERATIONS, OrchestrationCoordinationPort,
    OrchestrationHostDispatcher, OrchestrationRuntimePort, RuntimeAgentDescription,
    RuntimeLaunchRequest, RuntimeLaunchSuccess, UnavailableCoordination,
};
pub use orchestration_transport::{
    OrchestrationHostControl, OrchestrationHostPaths, PreparedOrchestrationHost, ScheduleTrigger,
    prepare_interactive as prepare_orchestration_host,
    prepare_one_shot as prepare_one_shot_orchestration_host, trigger_thread_schedule,
};
pub use pane_mcp::{PANE_MCP_SOCKET_ENV, PaneMcpServer};
pub use prompt_context::{
    HostPromptContextRuntimeView, PROMPT_CONTEXT_OPERATIONS, PromptContextHostDispatcher,
};
pub use protocol::{
    BridgeError, BridgeRequest, BridgeResponse, HostFrame, HostReady, PROTOCOL_VERSION, PushFrame,
};
pub use router::{
    COMPOSITE_OWNED_OPERATION_COUNT, DesktopHostDispatcher, DuplicateOperationOwner,
    OperationOwnerRouter,
};
pub use server::{HOST_MAX_FRAME_BYTES, HostServer, HostServerConfig, HostServerError};
pub use session_management::{
    ADOPT_ALL_OPERATION, IMPORT_PROGRESS_OPERATION, SEND_LIVE_OPERATION,
    SESSION_MANAGEMENT_OPERATIONS, SessionManagementHostDispatcher, SessionManagementRuntimePort,
    SupplementalUsageReader,
};
pub use terminal::{TERMINAL_OPERATIONS, TerminalHostDispatcher};
pub use threads::{
    THREAD_OPERATIONS, ThreadHost, ThreadHostConfig, ThreadJournalConfig, ThreadPorts,
    ThreadRegistrySignal, ThreadRunTap, ThreadTeamConfig,
};
pub use updater::{
    INTERNAL_UPDATER_OPERATIONS, NATIVE_UPDATER_CONFIGURE, NATIVE_UPDATER_EVENT,
    UPDATER_OPERATIONS, UpdaterHostDispatcher, spawn_update_state_forwarder,
};
pub use voice::{DeferredVoiceMcp, VOICE_OPERATIONS, VoiceDeps, VoiceEventTap, VoiceService};
pub use worktree::{
    ManagedWorktreeInventory, SystemWorktreeService, WorktreeService, WorktreeServiceError,
};
pub use worktree_release::AutomaticWorktreeReleaseService;

pub use pane_delegates::CliDelegate;

pub mod remote;

mod agent_consultation;
pub mod agent_tools;
pub mod team_tools;
