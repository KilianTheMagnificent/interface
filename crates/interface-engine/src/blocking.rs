//! Blocking provider-store work run off the async runtime.

use crate::{EngineError, EngineErrorKind};

/// Run `work` on the blocking pool. A task that panics or is cancelled before
/// returning is definitely-unsent for a read, which changed nothing, and
/// outcome-unknown for a mutation, which may have partly run.
///
/// # Errors
///
/// Returns [`join_error`] with `failure` when the task does not return.
pub async fn run<T: Send + 'static>(
    mutating: bool,
    failure: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, EngineError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| join_error(mutating, failure))
}

/// The error for blocking store work that did not return; see [`run`].
#[must_use]
pub fn join_error(mutating: bool, failure: &'static str) -> EngineError {
    if mutating {
        EngineError::outcome_unknown(EngineErrorKind::Internal, failure, true)
    } else {
        EngineError::definitely_unsent(EngineErrorKind::Internal, failure, true)
    }
}
