//! The one way host owners reach the session registry behind a
//! `SessionRuntime`: take the lock, map a poisoned lock to a definitely-unsent
//! registry error, and after a write release the lock before scheduling the
//! cache save. Each owner keeps its own mapping of a registry write failure.

use interface_contracts::engine::{EngineEvent, SessionSummary};
use interface_registry::{RegistryError, SessionRegistry, sort_listed};
use interface_runtime::{
    RuntimeClock, RuntimeError, RuntimeErrorKind, SessionRuntime, SystemRuntimeClock,
};

fn poisoned() -> RuntimeError {
    RuntimeError::definitely_unsent(
        RuntimeErrorKind::Registry,
        "session registry lock was poisoned",
        false,
    )
}

/// Runs `read` under the registry lock.
///
/// # Errors
///
/// Returns a definitely-unsent registry error when the lock is poisoned.
pub(crate) fn read<T>(
    runtime: &SessionRuntime,
    read: impl FnOnce(&SessionRegistry) -> T,
) -> Result<T, RuntimeError> {
    let registry = runtime.services().session_registry();
    let registry = registry.lock().map_err(|_| poisoned())?;
    Ok(read(&registry))
}

/// Runs `write` under the registry lock with the registry's current revision.
/// When it reports a change (`Some`), the lock is released and then the cache
/// save is scheduled; `None` changed nothing and schedules nothing.
///
/// # Errors
///
/// Returns a definitely-unsent registry error when the lock is poisoned, or
/// the error `write` returned.
pub(crate) fn write<T>(
    runtime: &SessionRuntime,
    write: impl FnOnce(&mut SessionRegistry, u64) -> Result<Option<T>, RuntimeError>,
) -> Result<Option<T>, RuntimeError> {
    let services = runtime.services();
    let registry = services.session_registry();
    let mut registry = registry.lock().map_err(|_| poisoned())?;
    let revision = registry.revision();
    let written = write(&mut registry, revision)?;
    drop(registry);
    if written.is_some() {
        services.schedule_registry_cache_save();
    }
    Ok(written)
}

/// The visible sessions in list order, cloned under the lock every provider
/// event needs and sorted after it: the same order `SessionRegistry::list`
/// gives, since the sort is stable over the same rows.
///
/// # Errors
///
/// Returns a definitely-unsent registry error when the lock is poisoned.
pub(crate) fn sorted_visible(
    runtime: &SessionRuntime,
) -> Result<Vec<SessionSummary>, RuntimeError> {
    let mut sessions = read(runtime, SessionRegistry::list_unsorted)?;
    sort_listed(&mut sessions);
    Ok(sessions)
}

/// The registry's content revision for a two-rate registry coalescer. A
/// poisoned lock reads as unknown, which only restores the 50 ms cadence.
pub(crate) fn content_revision(runtime: &SessionRuntime) -> Option<u64> {
    read(runtime, SessionRegistry::content_revision).ok()
}

/// Projects one session event onto the registry, stamped now. The ports that
/// project onto the registry (`RuntimeCommandHandler` and
/// `OrchestrationRuntimePort`) both delegate here: two copies of this write
/// ordering drifted once already, classifying the same `apply_event` failure
/// with opposite retryability.
///
/// # Errors
///
/// Returns a definitely-unsent registry error when the lock is poisoned, or a
/// retryable one when the registry refuses the event.
pub(crate) fn apply_event(
    runtime: &SessionRuntime,
    session_key: &str,
    event: &EngineEvent,
) -> Result<(), RuntimeError> {
    write(runtime, |registry, revision| {
        let at = SystemRuntimeClock.now();
        registry
            .apply_event(revision, session_key, event, at)
            .map(Some)
            .map_err(|error| runtime_registry_error(&error))
    })
    .map(|_| ())
}

/// A registry write the registry refused: definitely unsent, retryable.
pub(crate) fn runtime_registry_error(error: &RegistryError) -> RuntimeError {
    RuntimeError::definitely_unsent(RuntimeErrorKind::Registry, error.to_string(), true)
}
