use std::path::PathBuf;

use interface_storage::StorageError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("corrupt registry state at {path}: {reason}")]
    Corrupt { path: PathBuf, reason: String },

    #[error("invalid registry state: {0}")]
    Invalid(String),

    #[error("worktree operation failed: {0}")]
    Worktree(String),

    #[error("checkpoint operation failed at {path}: {reason}")]
    Checkpoint { path: PathBuf, reason: String },

    #[error("could not {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl RegistryError {
    pub(crate) fn corrupt(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
