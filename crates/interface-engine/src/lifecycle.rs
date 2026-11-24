use std::fmt;

use interface_contracts::engine::{EngineEvent, SessionStatus, next_status};

use crate::{EngineError, EngineErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPhase {
    Opening,
    Ready,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleError {
    operation: &'static str,
    phase: SessionPhase,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot {} while the session is {:?}",
            self.operation, self.phase
        )
    }
}

impl std::error::Error for LifecycleError {}

impl From<LifecycleError> for EngineError {
    fn from(error: LifecycleError) -> Self {
        Self::definitely_unsent(EngineErrorKind::Lifecycle, error.to_string(), false)
    }
}

/// Provider-neutral handle lifecycle, separate from the visible activity dot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionLifecycle {
    phase: SessionPhase,
    status: SessionStatus,
}

impl Default for SessionLifecycle {
    fn default() -> Self {
        Self {
            phase: SessionPhase::Opening,
            status: SessionStatus::Idle,
        }
    }
}

impl SessionLifecycle {
    /// Move an opening handle to ready.
    ///
    /// # Errors
    ///
    /// Returns an error unless the handle is opening.
    pub fn mark_ready(&mut self) -> Result<(), LifecycleError> {
        self.require_phase("mark ready", SessionPhase::Opening)?;
        self.phase = SessionPhase::Ready;
        Ok(())
    }

    /// Apply the legacy status hold rule without changing handle readiness.
    ///
    /// A failed turn is not a failed session: a non-retryable provider error
    /// moves the visible status to `Failed` and leaves the handle sendable,
    /// as the 0.1.169 adapters did. Only [`Self::mark_failed`] and the close
    /// transitions end a handle.
    pub fn apply_event(&mut self, event: &EngineEvent) {
        self.status = next_status(self.status, event);
    }

    /// Prove that a new provider effect may be dispatched.
    ///
    /// # Errors
    ///
    /// Returns an error unless provider readiness has completed and close has not begun.
    pub fn require_sendable(&self) -> Result<(), LifecycleError> {
        self.require_phase("send", SessionPhase::Ready)
    }

    pub fn begin_close(&mut self) {
        if matches!(self.phase, SessionPhase::Closed | SessionPhase::Closing) {
            return;
        }
        self.phase = SessionPhase::Closing;
    }

    /// Finish a close transition.
    ///
    /// # Errors
    ///
    /// Returns an error unless close has begun.
    pub fn mark_closed(&mut self) -> Result<(), LifecycleError> {
        self.require_phase("mark closed", SessionPhase::Closing)?;
        self.phase = SessionPhase::Closed;
        Ok(())
    }

    pub fn mark_failed(&mut self) {
        self.phase = SessionPhase::Failed;
        self.status = SessionStatus::Failed;
    }

    fn require_phase(
        &self,
        operation: &'static str,
        expected: SessionPhase,
    ) -> Result<(), LifecycleError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(LifecycleError {
                operation,
                phase: self.phase,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::{EngineEventKind, NoticeTone};

    use super::*;

    #[test]
    fn opening_and_closed_handles_refuse_sends_before_dispatch() {
        let mut lifecycle = SessionLifecycle::default();
        let opening: EngineError = lifecycle.require_sendable().unwrap_err().into();
        assert!(opening.replay_is_safe());
        lifecycle.mark_ready().unwrap();
        lifecycle.begin_close();
        lifecycle.mark_closed().unwrap();
        let closed: EngineError = lifecycle.require_sendable().unwrap_err().into();
        assert!(closed.replay_is_safe());
    }

    #[test]
    fn notice_does_not_move_status_and_a_failed_turn_keeps_the_handle_sendable() {
        let mut lifecycle = SessionLifecycle::default();
        lifecycle.mark_ready().unwrap();
        lifecycle.apply_event(&EngineEvent {
            at: None,
            event: EngineEventKind::Notice {
                message: "heads up".into(),
                tone: NoticeTone::Warning,
            },
        });
        assert_eq!(lifecycle.status, SessionStatus::Idle);
        lifecycle.apply_event(&EngineEvent {
            at: None,
            event: EngineEventKind::Error {
                cause: None,
                message: "turn failed".into(),
                retryable: false,
            },
        });
        assert_eq!(lifecycle.status, SessionStatus::Failed);
        assert_eq!(lifecycle.phase, SessionPhase::Ready);
        lifecycle.require_sendable().unwrap();
        lifecycle.apply_event(&EngineEvent {
            at: None,
            event: EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            },
        });
        lifecycle.require_sendable().unwrap();
    }

    #[test]
    fn only_an_explicit_failure_ends_the_handle() {
        let mut lifecycle = SessionLifecycle::default();
        lifecycle.mark_ready().unwrap();
        lifecycle.mark_failed();
        let failed: EngineError = lifecycle.require_sendable().unwrap_err().into();
        assert!(failed.replay_is_safe());
        assert_eq!(lifecycle.phase, SessionPhase::Failed);
    }
}
