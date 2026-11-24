use std::fmt;

use serde::{Deserialize, Serialize};

/// Whether a failed operation could have produced a provider-side effect.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectCertainty {
    /// Validation or lifecycle gating refused the operation before dispatch.
    DefinitelyUnsent,
    /// The provider returned a structured rejection for the exact request.
    DefinitelyRejected,
    /// Dispatch began, but no authoritative provider result was observed.
    OutcomeUnknown,
}

/// Stable categories suitable for UI and control-plane decisions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineErrorKind {
    InvalidInput,
    Unsupported,
    Unavailable,
    Lifecycle,
    Capacity,
    Timeout,
    Transport,
    Protocol,
    Provider,
    Internal,
}

/// A sanitized engine failure with an explicit provider-effect boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineError {
    pub kind: EngineErrorKind,
    pub message: String,
    pub retryable: bool,
    pub certainty: EffectCertainty,
}

impl EngineError {
    #[must_use]
    pub fn definitely_unsent(
        kind: EngineErrorKind,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            certainty: EffectCertainty::DefinitelyUnsent,
        }
    }

    #[must_use]
    pub fn definitely_rejected(
        kind: EngineErrorKind,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            certainty: EffectCertainty::DefinitelyRejected,
        }
    }

    #[must_use]
    pub fn outcome_unknown(
        kind: EngineErrorKind,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            certainty: EffectCertainty::OutcomeUnknown,
        }
    }

    /// Blind replay is safe only when the original frame was never sent.
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(self.certainty, EffectCertainty::DefinitelyUnsent)
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

/// The three-state result of a provider-effecting operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome<T> {
    Confirmed(T),
    DefinitelyUnsent(EngineError),
    DefinitelyRejected(EngineError),
    OutcomeUnknown(EngineError),
}

impl<T> DeliveryOutcome<T> {
    #[must_use]
    pub fn from_error(error: EngineError) -> Self {
        match error.certainty {
            EffectCertainty::DefinitelyUnsent => Self::DefinitelyUnsent(error),
            EffectCertainty::DefinitelyRejected => Self::DefinitelyRejected(error),
            EffectCertainty::OutcomeUnknown => Self::OutcomeUnknown(error),
        }
    }

    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }

    /// Returns true only when replay cannot duplicate a provider effect.
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(self, Self::DefinitelyUnsent(_))
    }

    pub fn map<U>(self, mapper: impl FnOnce(T) -> U) -> DeliveryOutcome<U> {
        match self {
            Self::Confirmed(value) => DeliveryOutcome::Confirmed(mapper(value)),
            Self::DefinitelyUnsent(error) => DeliveryOutcome::DefinitelyUnsent(error),
            Self::DefinitelyRejected(error) => DeliveryOutcome::DefinitelyRejected(error),
            Self::OutcomeUnknown(error) => DeliveryOutcome::OutcomeUnknown(error),
        }
    }

    /// The confirmed value, or the error. Only the error's own
    /// [`EngineError::certainty`] survives, so a caller that must tell an
    /// unsent effect from an unknown one reads it there.
    ///
    /// # Errors
    ///
    /// Returns the error of every outcome but [`DeliveryOutcome::Confirmed`].
    pub fn into_result(self) -> Result<T, EngineError> {
        match self {
            Self::Confirmed(value) => Ok(value),
            Self::DefinitelyUnsent(error)
            | Self::DefinitelyRejected(error)
            | Self::OutcomeUnknown(error) => Err(error),
        }
    }

    /// The error of every outcome but [`DeliveryOutcome::Confirmed`].
    #[must_use]
    pub const fn error(&self) -> Option<&EngineError> {
        match self {
            Self::Confirmed(_) => None,
            Self::DefinitelyUnsent(error)
            | Self::DefinitelyRejected(error)
            | Self::OutcomeUnknown(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_never_dispatched_effects_are_blindly_replayable() {
        let unsent = EngineError::definitely_unsent(
            EngineErrorKind::Transport,
            "writer rejected the frame",
            true,
        );
        let unknown = EngineError::outcome_unknown(
            EngineErrorKind::Transport,
            "connection ended after dispatch",
            true,
        );
        assert!(unsent.replay_is_safe());
        assert!(!unknown.replay_is_safe());
        assert!(DeliveryOutcome::<()>::from_error(unsent).replay_is_safe());
        assert!(!DeliveryOutcome::<()>::from_error(unknown).replay_is_safe());
    }

    #[test]
    fn into_result_and_error_keep_each_outcome_certainty() {
        assert_eq!(DeliveryOutcome::Confirmed(7).into_result(), Ok(7));
        assert_eq!(DeliveryOutcome::Confirmed(7).error(), None);
        for error in [
            EngineError::definitely_unsent(EngineErrorKind::Transport, "unsent", true),
            EngineError::definitely_rejected(EngineErrorKind::Provider, "rejected", false),
            EngineError::outcome_unknown(EngineErrorKind::Transport, "unknown", false),
        ] {
            let outcome = DeliveryOutcome::<u8>::from_error(error.clone());
            assert_eq!(outcome.error(), Some(&error));
            let returned = outcome.clone().into_result().unwrap_err();
            assert_eq!(returned, error);
            assert_eq!(DeliveryOutcome::<u8>::from_error(returned), outcome);
        }
    }
}
