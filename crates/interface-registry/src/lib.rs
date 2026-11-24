//! Durable session, worktree, and review-checkpoint registries.

mod checkpoint;
pub mod delegate_worktree;
mod error;
mod session;
mod worktree;

pub use checkpoint::{
    CheckpointBlobReader, CheckpointDraft, CheckpointFile, CheckpointInputFile, CheckpointKind,
    CheckpointLimits, CheckpointManifest, CheckpointOmission, CheckpointOmissionReason,
    CheckpointStore, CheckpointSummary, TURN_CHECKPOINT_RETENTION, bounded_unsafe_path,
    inspect_checkpoint_read_only, is_utf8_text, list_checkpoints_read_only,
    read_checkpoint_blob_read_only,
};
pub use error::{RegistryError, Result};
pub use session::{
    REGISTRY_MAX_BYTES, RegistrySave, SessionRegistry, read_registry_file, sort_listed,
};
pub use worktree::{
    CreateWorktreeOptions, GitResult, GitRunner, ManagedWorktreeRecord,
    ManagedWorktreeRegistration, ManagedWorktreeStore, RemoveKept, RemoveResult, SPAWN_FAILED,
    SystemGit, VerifiedWorktreeMetadata, WORKTREE_STORE_MAX_BYTES, Worktree, WorktreeError,
    WorktreeFailure, create_worktree, create_worktree_with_suffix, default_base_for,
    launch_recovery_scope, load_managed_worktrees_read_only, managed_worktree_registration,
    remove_registered_worktree, remove_worktree, slugify, verify_managed_worktree,
    verify_worktree_for_lease,
};
