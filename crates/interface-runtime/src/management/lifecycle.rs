//! Per-session lifecycle leases: one operation changes a session at a time,
//! and an identical concurrent request joins the running one's result.

use std::sync::Arc;

use crate::{RuntimeError, RuntimeErrorKind};

use super::{SessionManagementState, SettleOnce, local_error, remove_entry_if_current};

impl SessionManagementState {
    /// Registers `operation` on `key`: `Ok(entry)` when this call now owns the
    /// session's lifecycle, `Err(current)` when an identical coalescing
    /// operation is already running. A differing operation is refused, and an
    /// operation without an identity never joins another one.
    fn claim(
        &self,
        key: &str,
        operation: &'static str,
        identity: Option<LifecycleIdentity>,
    ) -> Result<Result<Arc<LifecycleEntry>, Arc<LifecycleEntry>>, RuntimeError> {
        let mut active = self.lifecycle.lock().map_err(|_| {
            local_error(
                RuntimeErrorKind::Internal,
                "session lifecycle mutex is poisoned",
                false,
            )
        })?;
        if let Some(current) = active.get(key) {
            if identity.is_some() && current.identity == identity {
                return Ok(Err(Arc::clone(current)));
            }
            return Err(local_error(
                RuntimeErrorKind::Lifecycle,
                format!(
                    "That session is already changing. Wait for {} to finish before {operation}.",
                    current.operation
                ),
                true,
            ));
        }
        let entry = Arc::new(LifecycleEntry::new(operation, identity));
        active.insert(key.to_owned(), Arc::clone(&entry));
        Ok(Ok(entry))
    }

    /// Holds `key`'s lifecycle for a non-coalescing operation (fork): any
    /// concurrent operation on the same session is refused.
    pub(super) fn begin(
        self: &Arc<Self>,
        key: &str,
        operation: &'static str,
    ) -> Result<LifecycleGuard, RuntimeError> {
        match self.claim(key, operation, None)? {
            Ok(entry) => Ok(LifecycleGuard {
                state: Arc::clone(self),
                key: key.to_owned(),
                entry,
            }),
            Err(_) => unreachable!("an operation without an identity never joins another"),
        }
    }

    /// Runs one coalesced lifecycle unit. The owner runs `work` and settles
    /// the unit with its result; an identical concurrent request waits for and
    /// returns that result without ever polling its own `work`.
    pub(super) async fn coalesced_unit(
        self: &Arc<Self>,
        key: &str,
        operation: &'static str,
        identity: LifecycleIdentity,
        work: impl Future<Output = Result<(), RuntimeError>>,
    ) -> Result<(), RuntimeError> {
        let entry = match self.claim(key, operation, Some(identity))? {
            Ok(entry) => entry,
            Err(current) => return current.unit.wait().await,
        };
        let owner = LifecycleUnitOwner {
            state: Arc::clone(self),
            key: key.to_owned(),
            entry,
            complete: false,
        };
        owner.finish(work.await)
    }

    pub(super) fn is_changing(&self, key: &str) -> bool {
        self.lifecycle
            .lock()
            .map_or(true, |active| active.contains_key(key))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LifecycleIdentity {
    Forget,
    LocalArchive(bool),
    Rename(String),
    Delete,
    Adopt(bool),
    ProviderArchive(bool),
}

pub(super) struct LifecycleEntry {
    operation: &'static str,
    identity: Option<LifecycleIdentity>,
    unit: SettleOnce<Result<(), RuntimeError>>,
}

impl LifecycleEntry {
    fn new(operation: &'static str, identity: Option<LifecycleIdentity>) -> Self {
        Self {
            operation,
            identity,
            unit: SettleOnce::new(),
        }
    }
}

struct LifecycleUnitOwner {
    state: Arc<SessionManagementState>,
    key: String,
    entry: Arc<LifecycleEntry>,
    complete: bool,
}

impl LifecycleUnitOwner {
    fn finish(mut self, result: Result<(), RuntimeError>) -> Result<(), RuntimeError> {
        self.entry.unit.complete(result.clone());
        self.remove();
        self.complete = true;
        result
    }

    fn remove(&self) {
        remove_entry_if_current(&self.state.lifecycle, &self.key, &self.entry);
    }
}

impl Drop for LifecycleUnitOwner {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        self.entry.unit.complete(Err(local_error(
            RuntimeErrorKind::Lifecycle,
            "The session operation ended before a result was available.",
            true,
        )));
        self.remove();
    }
}

pub(super) struct LifecycleGuard {
    state: Arc<SessionManagementState>,
    key: String,
    entry: Arc<LifecycleEntry>,
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        remove_entry_if_current(&self.state.lifecycle, &self.key, &self.entry);
    }
}
