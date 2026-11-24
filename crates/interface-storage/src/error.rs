use std::io;
use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, StorageError>;

/// Fail-closed errors surfaced by the storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("invalid storage path {path}: {reason}")]
    InvalidPath { path: PathBuf, reason: &'static str },

    #[error("invalid storage entry name {name:?}")]
    InvalidName { name: String },

    #[error("unsafe storage at {path}: {reason}")]
    UnsafeStorage { path: PathBuf, reason: String },

    #[error("{path} exceeds the {max_bytes}-byte limit (observed {actual_bytes} bytes)")]
    BoundsExceeded {
        path: PathBuf,
        max_bytes: usize,
        actual_bytes: u64,
    },

    #[error("corrupt state at {path}: {reason}")]
    Corrupt { path: PathBuf, reason: String },

    #[error("invalid state proposed for {path}: {reason}")]
    InvalidData { path: PathBuf, reason: String },

    #[error("authoritative state at {path} changed outside this writer")]
    ExternalDrift { path: PathBuf },

    #[error("stale revision: expected {expected}, current revision is {current}")]
    CasConflict { expected: u64, current: u64 },

    #[error("invalid revision transition from {current} to {next}")]
    InvalidRevision { current: u64, next: u64 },

    #[error("another writer holds {path}")]
    LockHeld { path: PathBuf },

    #[error("the writer lease at {path} is no longer trustworthy")]
    LockLost { path: PathBuf },

    #[error("storage entry was not found at {path}")]
    NotFound { path: PathBuf },

    #[error("atomic write to {path} failed before rename: {reason}")]
    WriteFailed { path: PathBuf, reason: String },

    #[error("atomic write to {path} may have committed and must be reconciled: {reason}")]
    WriteUncertain { path: PathBuf, reason: String },

    #[error("could not {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl StorageError {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn unsafe_at(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::UnsafeStorage {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
