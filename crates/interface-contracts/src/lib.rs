//! Versioned, provider-neutral contracts shared by every Interface surface.

pub mod agents;
pub mod appearance;
pub mod away;
pub mod delegation;
pub mod diagnostics;
pub mod draft_stash;
pub mod engine;
pub mod ipc;
pub mod orchestration;
pub mod orchestration_control;
pub mod prompt_images;
pub mod provider_accounts;
pub mod provider_commands;
pub mod questions;
pub mod retention;
pub mod terminal;
pub mod terminal_controls;
pub mod thread_journal;
pub mod thread_secrets;
pub mod threads;
pub mod update_sessions;
pub mod updater;
mod validation;
pub mod voice;
pub mod voice_control;

pub use validation::{
    CANONICAL_TIMESTAMP_FORMAT, ValidationError, canonical_timestamp, parse_canonical_timestamp,
};

pub mod delegate_validation;

pub mod workspace_tools;

pub mod remote;

pub mod remote_approval;
pub mod remote_connection;
pub mod remote_threads;
pub mod remote_trust;

pub mod command_catalog;

pub mod harness;

pub mod agent_work;
pub mod output_preview;
pub mod projects;

pub mod background;
pub mod customization;
pub mod output_review;
pub mod workflows;

pub mod recovery;
