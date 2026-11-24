use thiserror::Error;

/// Internal certainty; never serialized into the frozen desktop payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopEffectCertainty {
    DefinitelyUnsent,
    OutcomeUnknown,
    /// General failures inherit the operation's conservative effect policy.
    OperationDefault,
}

/// Stable error returned to a host dispatcher. `message` is the same prose the
/// Electron preload exposed after removing Electron's invoke wrapper.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{message}")]
pub struct DesktopServiceError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub certainty: DesktopEffectCertainty,
}

impl DesktopServiceError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        let code = code.into();
        let certainty = match code.as_str() {
            "invalid-payload"
            | "invalid-acp-profiles"
            | "request-refused"
            | "appearance-unavailable" => DesktopEffectCertainty::DefinitelyUnsent,
            _ => DesktopEffectCertainty::OperationDefault,
        };
        Self {
            code,
            message: message.into(),
            retryable,
            certainty,
        }
    }

    #[must_use]
    pub fn outcome_unknown(mut self) -> Self {
        self.certainty = DesktopEffectCertainty::OutcomeUnknown;
        self
    }

    #[must_use]
    pub(crate) fn invalid_payload(operation: &str) -> Self {
        Self::new(
            "invalid-payload",
            format!("The {operation} request payload was invalid."),
            false,
        )
    }

    #[must_use]
    pub(crate) fn refused(lead: &str, detail: &str) -> Self {
        Self::new("request-refused", format!("{lead}\n{detail}"), false)
    }

    #[must_use]
    pub(crate) fn state_unavailable() -> Self {
        Self::new(
            "desktop-state-unavailable",
            "Interface could not access its private desktop state.",
            false,
        )
    }

    #[must_use]
    pub(crate) fn appearance_unavailable() -> Self {
        Self::new(
            "appearance-unavailable",
            "The appearance file is unavailable; the theme was not saved.",
            false,
        )
    }
}
