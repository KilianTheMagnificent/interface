//! Bounded, credential-aware workspace reads for the desktop and CLI hosts.

pub mod delegation;
mod editor;
mod fsid;
mod inventory;
pub mod output_preview;
mod search;

pub use editor::{
    DirectoryEntry, EDITOR_DIRECTORY_ENTRY_LIMIT, EDITOR_FILE_BYTE_LIMIT, EditorHooks,
    WorkspaceAuthority, WorkspaceEditorError,
};
pub use inventory::providers::{
    CustomizationRoots, check_mcp, provider_inventory, revalidate_customization_source,
};
pub use inventory::{
    CUSTOMIZATION_LIMITS, CustomizationLimits, default_customization_user_root,
    read_customization_inventory, read_frontmatter, rules_summary,
};
pub use search::{
    PROJECT_LISTING_DEADLINE, PROJECT_SEARCH_DEADLINE, PROJECT_SEARCH_LIMITS, ProjectSearchLimits,
    ProjectWalkHooks, ProjectWalkOptions, WorkspaceReadError, is_credential_path,
    is_likely_binary_path, is_search_excluded_path, list_project_files, list_project_files_bounded,
    search_project, search_project_bounded,
};
