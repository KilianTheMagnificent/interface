use interface_storage::StorageError;
use thiserror::Error;

/// Stable error codes shared by reducer, scheduler, projection, and store callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OrchestrationErrorCode {
    InvalidMutation,
    NotFound,
    AlreadyExists,
    InvalidTransition,
    DependencyBlocked,
    EvidenceRequired,
    LimitExceeded,
    IdempotencyConflict,
    InvalidState,
    UnsafeStorage,
    StateCorrupt,
    CasConflict,
    WriteFailed,
    WriteUncertain,
    WriterLeaseLost,
    StoreClosed,
    StorePoisoned,
    PublicBudget,
}

/// Fail-closed orchestration error with the current revision when known.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct OrchestrationError {
    pub code: OrchestrationErrorCode,
    pub message: String,
    pub current_revision: Option<u64>,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl OrchestrationError {
    pub(crate) fn new(code: OrchestrationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            current_revision: None,
            source: None,
        }
    }

    pub(crate) fn revision(mut self, revision: u64) -> Self {
        self.current_revision = Some(revision);
        self
    }
}

impl From<interface_contracts::ValidationError> for OrchestrationError {
    fn from(error: interface_contracts::ValidationError) -> Self {
        Self::new(OrchestrationErrorCode::InvalidState, error.to_string())
    }
}

impl From<StorageError> for OrchestrationError {
    fn from(error: StorageError) -> Self {
        let code = match &error {
            StorageError::InvalidPath { .. }
            | StorageError::InvalidName { .. }
            | StorageError::UnsafeStorage { .. } => OrchestrationErrorCode::UnsafeStorage,
            StorageError::CasConflict { .. } | StorageError::ExternalDrift { .. } => {
                OrchestrationErrorCode::CasConflict
            }
            StorageError::WriteUncertain { .. } => OrchestrationErrorCode::WriteUncertain,
            StorageError::Corrupt { .. } | StorageError::BoundsExceeded { .. } => {
                OrchestrationErrorCode::StateCorrupt
            }
            StorageError::LockHeld { .. } | StorageError::LockLost { .. } => {
                OrchestrationErrorCode::WriterLeaseLost
            }
            _ => OrchestrationErrorCode::WriteFailed,
        };
        Self {
            code,
            message: error.to_string(),
            current_revision: None,
            source: Some(Box::new(error)),
        }
    }
}

pub type Result<T> = std::result::Result<T, OrchestrationError>;
