//! Owner-private desktop preferences and bounded provider configuration.

mod acp_profiles;
mod agent_decisions;
pub mod agent_home;
mod agent_permissions;
mod agent_starts;
mod agents;
pub use agent_decisions::AgentDecisionLog;
pub use agent_starts::AgentStartAdmission;
mod appearance;
mod custom_themes;
mod theme_import;
pub use agents::{
    AGENT_TASK_RUN_PREFIX, AgentBinding, AgentStore, AgentTaskAdmission, AgentTaskRequest,
};
mod default_directory;
pub mod folder_trust;
#[cfg(test)]
mod golden;
mod leased;
mod private_path;
mod terminal_reopen_authority;
mod terminal_titles;
mod terminal_workspaces;
pub use terminal_titles::TerminalTitleStore;

pub use acp_profiles::{
    ACP_PROFILE_LIMITS, AcpProfileLimits, AcpProfileStore, validate_acp_argument_bounds,
    validate_acp_display_name, validate_acp_executable_path, validate_acp_profile,
    validate_acp_profiles, validate_acp_slug,
};
pub use appearance::AppearanceStore;
pub use custom_themes::{
    CUSTOM_THEME_LOOK_PROPERTY, CUSTOM_THEME_MAX_BYTES, CUSTOM_THEME_MAX_COUNT,
    CUSTOM_THEME_PREVIEW_PROPERTIES, CustomThemeFile, CustomThemeLibrary, custom_themes_directory,
};
pub use default_directory::{
    directory_picker_start_path, directory_picker_start_path_with, is_file_provider_managed,
    resolve_default_directory,
};
pub use private_path::SettingsError;
pub use terminal_reopen_authority::{
    TerminalReopenAuthority, TerminalReopenAuthorityCompareExchange, TerminalReopenAuthoritySlot,
    TerminalReopenAuthorityStore, terminal_identity_matches, terminal_reopen_authority_path,
};
pub use terminal_workspaces::{
    TERMINAL_WORKSPACE_LIMITS, TerminalWorkspaceLimits, TerminalWorkspaceStore,
    validate_terminal_workspace, validate_terminal_workspaces,
};
pub use theme_import::{
    ImportedScheme, SCHEME_IMPORT_MAX_BYTES, SchemeFormat, SchemeImportError, convert_scheme,
};

mod phone_connection;
mod remote_trust;
pub use phone_connection::PhoneConnectionStore;
pub use remote_trust::RemoteTrustStore;

mod harness;
pub use harness::HarnessStore;

mod agent_work;
pub use agent_work::{AgentWorkStore, validate_agent_work_spec};

mod agent_calendar;
pub use agent_calendar::{calendar_occurrences, validate_calendar};

pub mod draft_stash;
pub mod provider_accounts;
pub mod thread_journal;
pub mod thread_reference;
pub mod thread_schedules;
pub mod thread_secrets;
pub mod thread_sides;
pub mod thread_team;
pub mod thread_visuals;
pub mod threads;
pub use threads::{ThreadStore, ThreadStoreError, ThreadsDocument};

mod session_access;
pub use session_access::SessionAccessStore;

mod provider_canary;
pub use provider_canary::{ProviderCanaryChange, ProviderCanarySeen, ProviderCanaryStore};

mod trusted_folders;
pub use trusted_folders::{
    FolderAnswer, TRUSTED_FOLDERS_FILE_NAME, TRUSTED_FOLDERS_MAX_ENTRIES,
    TRUSTED_FOLDERS_MAX_PATH_BYTES, TrustedFoldersStore, valid_trust_path,
};

mod projects_folder;
pub use projects_folder::{
    DEFAULT_PROJECTS_FOLDER, ProjectsFolderOrigin, ProjectsFolderStore, ResolvedProjectsFolder,
    resolve_projects_folder_with,
};

mod background;
pub mod workflows;
pub use background::BackgroundStore;
mod output_reviews;
pub use output_reviews::{
    OUTPUT_REVIEW_SNAPSHOTS, OUTPUT_REVIEWS_FILE_NAME, OUTPUT_REVIEWS_V1_FILE_NAME,
    OutputReviewStore,
};

mod prompt_recovery;
pub use prompt_recovery::PromptRecoveryStore;
