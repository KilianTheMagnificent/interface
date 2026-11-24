use thiserror::Error;

use interface_contracts::terminal::{TerminalGeneration, TerminalId, TerminalLaunchProfileId};

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{message}")]
pub struct TerminalBackendError {
    message: String,
}

impl TerminalBackendError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TerminalError {
    #[error("the terminal cwd is not a stable canonical directory")]
    UnsafeCwd,
    #[error("the terminal cwd is not an exact approved root")]
    UnauthorizedCwd,
    #[error("terminal cwd identity changed during the operation")]
    CwdChanged,
    #[error("terminal dimensions must be between 2x1 and 1000x500 cells")]
    InvalidSize,
    #[error("terminal title must be non-empty, at most 120 characters, and contain no controls")]
    InvalidTitle,
    #[error("terminal input exceeds the 65536-byte write limit")]
    InputTooLarge,
    #[error("terminal launch request version {0} is unsupported")]
    UnsupportedLaunchVersion(u8),
    #[error("terminal launch profile {0} is unavailable")]
    UnavailableLaunchProfile(TerminalLaunchProfileId),
    #[error("terminal launch session version {0} is unsupported")]
    UnsupportedLaunchSessionVersion(u8),
    #[error(
        "a terminal chat id must be 1-128 characters of letters, digits, dot, dash, or underscore"
    )]
    InvalidLaunchSessionId,
    #[error("terminal launch profile {0} cannot be pointed at a specific chat")]
    UnbindableLaunchProfile(TerminalLaunchProfileId),
    #[error("terminal launch profile {0} has no sign-in command")]
    NoSignIn(TerminalLaunchProfileId),
    /// Protocol 60: the provider account a launch named does not exist, or
    /// its environment could not be applied. Nothing was started.
    #[error("that provider account is not available: {0}")]
    AccountUnavailable(String),
    #[error("saved terminal history for {0} could not be verified safely")]
    ProviderSessionLookupFailed(TerminalLaunchProfileId),
    #[error("provider-session capture for {0} could not be prepared safely")]
    ProviderSessionCaptureUnavailable(TerminalLaunchProfileId),
    #[error("terminal pass-off requires a running source terminal")]
    HandoffSourceUnavailable,
    #[error("that agent terminal is no longer waiting to be handed a shell")]
    ShellHandoffUnavailable,
    #[error("terminal launch profile {0} cannot take a pass-off")]
    HandoffUnsupportedProfile(TerminalLaunchProfileId),
    #[error("terminal pass-off payload is malformed or oversized")]
    InvalidHandoff,
    #[error("terminal {0} was not found")]
    NotFound(TerminalId),
    #[error(
        "terminal {id} generation is stale: expected {expected:?}, current generation is {actual:?}"
    )]
    StaleGeneration {
        id: TerminalId,
        expected: TerminalGeneration,
        actual: TerminalGeneration,
    },
    #[error("terminal {0} creation conflicts with the request that already owns this id")]
    CreateConflict(TerminalId),
    #[error("terminal {0} has been dismissed and its id cannot be reused")]
    RetiredId(TerminalId),
    #[error("terminal {0} is not running")]
    NotRunning(TerminalId),
    #[error("terminal {0} is still running and cannot be dismissed")]
    StillRunning(TerminalId),
    #[error("terminal {0} is still running and cannot be restarted")]
    StillRunningForRestart(TerminalId),
    #[error("terminal {0} exhausted its generation counter")]
    GenerationExhausted(TerminalId),
    #[error("terminal limit of {0} entries has been reached")]
    LimitReached(usize),
    #[error("terminal output for {0} already has a reader")]
    OutputAlreadyClaimed(TerminalId),
    #[error("terminal output capacity and chunk sizes must be non-zero and internally consistent")]
    InvalidConfiguration,
    #[error("terminal workspace lease failed: {0}")]
    Lease(String),
    #[error("terminal backend failed: {0}")]
    Backend(String),
}
