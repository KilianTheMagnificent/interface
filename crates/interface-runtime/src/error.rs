use interface_engine::{EffectCertainty, EngineError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeErrorKind {
    InvalidInput,
    DuplicateEngine,
    EngineNotRegistered,
    SessionNotActive,
    Lifecycle,
    Provider,
    Registry,
    History,
    Internal,
}

/// Sanitized runtime failure suitable for IPC and CLI presentation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub retryable: bool,
    pub certainty: EffectCertainty,
}

impl RuntimeError {
    #[must_use]
    pub fn new(
        kind: RuntimeErrorKind,
        message: impl Into<String>,
        retryable: bool,
        certainty: EffectCertainty,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            certainty,
        }
    }

    #[must_use]
    pub fn definitely_unsent(
        kind: RuntimeErrorKind,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self::new(kind, message, retryable, EffectCertainty::DefinitelyUnsent)
    }

    #[must_use]
    pub fn from_engine(error: &EngineError) -> Self {
        Self::new(
            RuntimeErrorKind::Provider,
            error.message.clone(),
            error.retryable,
            error.certainty,
        )
    }

    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(self.certainty, EffectCertainty::DefinitelyUnsent)
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeError {}
