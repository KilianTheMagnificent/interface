use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use interface_registry::{
    CreateWorktreeOptions, GitRunner, ManagedWorktreeRecord, ManagedWorktreeRegistration,
    ManagedWorktreeStore, RegistryError, RemoveResult, SystemGit, Worktree, create_worktree,
    managed_worktree_registration, remove_worktree, verify_worktree_for_lease,
};
use interface_storage::StorageError;

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct WorktreeServiceError {
    pub message: String,
    pub replay_safe: bool,
}

impl WorktreeServiceError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            replay_safe: true,
        }
    }

    #[must_use]
    pub fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            replay_safe: false,
        }
    }
}

#[async_trait]
pub trait WorktreeService: Send + Sync + 'static {
    /// Creates a checkout before any provider process is launched.
    async fn create(
        &self,
        cwd: String,
        name: Option<String>,
    ) -> Result<Worktree, WorktreeServiceError>;

    /// Creates an orchestration checkout, initializing a fresh repository when
    /// the selected project directory has not been initialized yet.
    async fn create_initialized(
        &self,
        cwd: String,
        name: Option<String>,
    ) -> Result<Worktree, WorktreeServiceError>;

    /// Durably assigns an already-created checkout to a definitive session key.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the managed-worktree ledger cannot be updated.
    async fn persist(&self, key: &str, worktree: &Worktree) -> Result<(), WorktreeServiceError>;

    /// Removes an unassigned checkout after a launch that never became ready.
    async fn discard(&self, worktree: Worktree) -> Option<String>;
}

/// Read/mutate access to the same durable ledger used by session creation.
///
/// This is deliberately separate from [`WorktreeService`] so tests can inject
/// the explicit-management slice without pretending to create provider
/// sessions. Production coerces one [`SystemWorktreeService`] into both traits.
#[async_trait]
pub trait ManagedWorktreeInventory: Send + Sync + 'static {
    /// Returns a defensive snapshot in stable persisted order.
    ///
    /// # Errors
    ///
    /// Returns an error when the one-writer ledger cannot be read.
    fn records(&self) -> Result<Vec<ManagedWorktreeRecord>, WorktreeServiceError>;

    /// Re-proves that Git still identifies this exact path and branch.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded verification task cannot complete.
    async fn verify(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<ManagedWorktreeRegistration, WorktreeServiceError>;

    /// Removes without force, then durably forgets only this exact generation.
    ///
    /// # Errors
    ///
    /// Returns an explicit unknown-outcome error when removal or persistence
    /// cannot be proven complete.
    async fn remove(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<RemoveResult, WorktreeServiceError>;
}

pub struct SystemWorktreeService {
    git: Arc<dyn GitRunner>,
    ledger: Arc<Ledger>,
    /// Serializes every ledger mutation. A durable write takes its owned
    /// guard onto the blocking thread, so a caller that stops waiting cannot
    /// let the next writer start from the mirror before this write lands.
    mutation_gate: Arc<tokio::sync::Mutex<()>>,
}

/// The durable ledger and the in-memory records mirroring it. Writes run on
/// the blocking pool and replace the mirror there once they commit, so
/// readers never wait on the fsync: they see the previous records until then.
struct Ledger {
    store: Option<Mutex<ManagedWorktreeStore>>,
    records: Mutex<Vec<ManagedWorktreeRecord>>,
    uncertain: AtomicBool,
    #[cfg(test)]
    replace_failure: Mutex<Option<StorageError>>,
    /// Test seam: runs once inside the next durable write.
    #[cfg(test)]
    replace_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Why a durable ledger replacement did not commit.
enum LedgerWriteFailure {
    StorePoisoned,
    Write(WorktreeServiceError),
    WorkerFailed(String),
}

impl SystemWorktreeService {
    /// Opens the canonical managed-worktree ledger under its one-writer lease.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe storage, corruption, or writer contention.
    pub fn open(runtime_root: impl AsRef<Path>) -> Result<Self, WorktreeServiceError> {
        let store = ManagedWorktreeStore::open(runtime_root)
            .map_err(|error| WorktreeServiceError::new(error.to_string()))?;
        Ok(Self::with_ledger(
            Arc::new(SystemGit),
            Ledger::durable(store),
        ))
    }

    /// Keeps non-inventory Git helpers constructible when the recovery ledger
    /// could not be loaded. Durable inventory operations remain unavailable:
    /// an unreadable ledger must never be mistaken for proof that no managed
    /// worktrees exist.
    #[must_use]
    pub fn degraded() -> Self {
        Self::with_ledger(
            Arc::new(SystemGit),
            Ledger {
                store: None,
                records: Mutex::new(Vec::new()),
                uncertain: AtomicBool::new(true),
                #[cfg(test)]
                replace_failure: Mutex::new(None),
                #[cfg(test)]
                replace_hook: Mutex::new(None),
            },
        )
    }

    fn with_ledger(git: Arc<dyn GitRunner>, ledger: Ledger) -> Self {
        Self {
            git,
            ledger: Arc::new(ledger),
            mutation_gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[cfg(test)]
    fn with_git(store: ManagedWorktreeStore, git: Arc<dyn GitRunner>) -> Self {
        Self::with_ledger(git, Ledger::durable(store))
    }

    async fn create_with_initialization(
        &self,
        cwd: String,
        name: Option<String>,
        init_if_needed: bool,
    ) -> Result<Worktree, WorktreeServiceError> {
        let git = Arc::clone(&self.git);
        tokio::task::spawn_blocking(move || {
            let mut options = CreateWorktreeOptions::new(cwd);
            options.name = name;
            options.init_if_needed = init_if_needed;
            create_worktree(&options, git.as_ref()).map_err(|error| {
                WorktreeServiceError::new(format!("{} {}", error.lead, error.detail))
            })
        })
        .await
        .map_err(|error| {
            WorktreeServiceError::new(format!("worktree creation task failed: {error}"))
        })?
    }

    fn ensure_ledger_certain(&self) -> Result<(), WorktreeServiceError> {
        self.ledger.ensure_certain()
    }

    /// Durably replaces the ledger with `next`, then the in-memory records,
    /// both on the blocking pool while `mutation` is held there.
    async fn commit_records(
        &self,
        mutation: tokio::sync::OwnedMutexGuard<()>,
        next: Vec<ManagedWorktreeRecord>,
        prior_effect_already_happened: bool,
    ) -> Result<(), LedgerWriteFailure> {
        let ledger = Arc::clone(&self.ledger);
        let written = tokio::task::spawn_blocking(move || {
            let _mutation = mutation;
            ledger.commit(next, prior_effect_already_happened)
        })
        .await;
        written.unwrap_or_else(|error| {
            // The write may have committed before the worker failed.
            self.ledger.uncertain.store(true, Ordering::Release);
            Err(LedgerWriteFailure::WorkerFailed(error.to_string()))
        })
    }
}

impl Ledger {
    fn durable(store: ManagedWorktreeStore) -> Self {
        let records = store.records().to_vec();
        Self {
            store: Some(Mutex::new(store)),
            records: Mutex::new(records),
            uncertain: AtomicBool::new(false),
            #[cfg(test)]
            replace_failure: Mutex::new(None),
            #[cfg(test)]
            replace_hook: Mutex::new(None),
        }
    }

    fn write_error(
        &self,
        error: &RegistryError,
        mutation_may_have_committed: bool,
        prior_effect_already_happened: bool,
    ) -> WorktreeServiceError {
        let ledger_unavailable = matches!(
            error,
            RegistryError::Storage(
                StorageError::WriteUncertain { .. }
                    | StorageError::ExternalDrift { .. }
                    | StorageError::LockLost { .. }
            )
        ) || mutation_may_have_committed;
        if ledger_unavailable {
            self.uncertain.store(true, Ordering::Release);
        }
        if ledger_unavailable || prior_effect_already_happened {
            WorktreeServiceError::outcome_unknown(error.to_string())
        } else {
            WorktreeServiceError::new(error.to_string())
        }
    }

    fn ensure_certain(&self) -> Result<(), WorktreeServiceError> {
        if self.uncertain.load(Ordering::Acquire) {
            Err(WorktreeServiceError::outcome_unknown(
                "managed worktree metadata is unavailable after an uncertain durable write",
            ))
        } else {
            Ok(())
        }
    }

    #[allow(clippy::unused_self)]
    fn replace_records(
        &self,
        store: &mut ManagedWorktreeStore,
        records: Vec<ManagedWorktreeRecord>,
    ) -> Result<u64, RegistryError> {
        #[cfg(test)]
        if let Some(error) = self
            .replace_failure
            .lock()
            .expect("replace failure injection lock")
            .take()
        {
            return Err(error.into());
        }
        #[cfg(test)]
        if let Some(hook) = self
            .replace_hook
            .lock()
            .expect("replace hook injection lock")
            .take()
        {
            hook();
        }
        store.replace(store.revision(), records)
    }

    /// The durable half of a mutation. The records mirror changes only after
    /// the store committed `next`; the caller holds the mutation gate.
    fn commit(
        &self,
        next: Vec<ManagedWorktreeRecord>,
        prior_effect_already_happened: bool,
    ) -> Result<(), LedgerWriteFailure> {
        let Some(store) = &self.store else {
            // Callers keep a degraded ledger's records in memory themselves.
            *self
                .records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            return Ok(());
        };
        let mut store = store.lock().map_err(|_| {
            self.uncertain.store(true, Ordering::Release);
            LedgerWriteFailure::StorePoisoned
        })?;
        if let Err(error) = self.replace_records(&mut store, next.clone()) {
            let mutation_may_have_committed = store.records() == next;
            return Err(LedgerWriteFailure::Write(self.write_error(
                &error,
                mutation_may_have_committed,
                prior_effect_already_happened,
            )));
        }
        *self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        Ok(())
    }
}

#[async_trait]
impl WorktreeService for SystemWorktreeService {
    async fn create(
        &self,
        cwd: String,
        name: Option<String>,
    ) -> Result<Worktree, WorktreeServiceError> {
        self.create_with_initialization(cwd, name, false).await
    }

    async fn create_initialized(
        &self,
        cwd: String,
        name: Option<String>,
    ) -> Result<Worktree, WorktreeServiceError> {
        self.create_with_initialization(cwd, name, true).await
    }

    async fn persist(&self, key: &str, worktree: &Worktree) -> Result<(), WorktreeServiceError> {
        // The inventory and its full-snapshot store are one logical value.
        // Serialize the clone and durable replace so a concurrent persist or
        // post-removal rewrite cannot commit an older snapshot after a newer
        // one and silently forget an unrelated managed checkout.
        let mutation = Arc::clone(&self.mutation_gate).lock_owned().await;
        self.ensure_ledger_certain()?;
        let record = ManagedWorktreeRecord {
            key: key.to_owned(),
            path: worktree.path.clone(),
            session_cwd: worktree.session_cwd.clone(),
            branch: worktree.branch.clone(),
            repo_root: worktree.repo_root.clone(),
            source_cwd: worktree.source_cwd.clone(),
            dirty_source: worktree.dirty_source,
        };
        let next = {
            let mut records = self.ledger.records.lock().map_err(|_| {
                WorktreeServiceError::new("managed worktree inventory lock was poisoned")
            })?;
            let mut next = records.clone();
            next.retain(|current| current.key != key);
            next.push(record);
            if *records == next {
                return Ok(());
            }
            if self.ledger.store.is_none() {
                *records = next;
                return Err(WorktreeServiceError::new(
                    "managed worktree metadata is unavailable for this run; the checkout remains tracked in memory",
                ));
            }
            next
        };
        self.commit_records(mutation, next, false)
            .await
            .map_err(|failure| match failure {
                LedgerWriteFailure::StorePoisoned => WorktreeServiceError::outcome_unknown(
                    "managed worktree ledger lock was poisoned",
                ),
                LedgerWriteFailure::Write(error) => error,
                LedgerWriteFailure::WorkerFailed(error) => WorktreeServiceError::outcome_unknown(
                    format!("managed worktree ledger write task failed: {error}"),
                ),
            })
    }

    async fn discard(&self, worktree: Worktree) -> Option<String> {
        let git = Arc::clone(&self.git);
        tokio::task::spawn_blocking(move || {
            let removed = remove_worktree(&worktree, git.as_ref(), false);
            (!removed.removed).then(|| {
                format!(
                    " A worktree was left at {} (branch {}); nothing ever ran in it, so it is safe to delete.",
                    worktree.path.display(),
                    worktree.branch
                )
            })
        })
        .await
        .unwrap_or_else(|error| {
            Some(format!(
                " The worktree cleanup task failed ({error}); inspect the isolated checkout before retrying."
            ))
        })
    }
}

#[async_trait]
impl ManagedWorktreeInventory for SystemWorktreeService {
    fn records(&self) -> Result<Vec<ManagedWorktreeRecord>, WorktreeServiceError> {
        self.ensure_ledger_certain()?;
        let records = self.ledger.records.lock().map_err(|_| {
            WorktreeServiceError::new("managed worktree inventory lock was poisoned")
        })?;
        self.ensure_ledger_certain()?;
        Ok(records.clone())
    }

    async fn verify(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<ManagedWorktreeRegistration, WorktreeServiceError> {
        let git = Arc::clone(&self.git);
        tokio::task::spawn_blocking(move || {
            let worktree = record.worktree();
            let registration = managed_worktree_registration(&worktree, git.as_ref(), false);
            if registration != ManagedWorktreeRegistration::Active {
                return registration;
            }
            // A stale source-repository registration can outlive a moved or
            // replaced directory. Re-prove the actual checkout toplevel, HEAD,
            // containment, and readable dirty state before treating it as
            // lease-worthy managed authority.
            match verify_worktree_for_lease(&worktree, git.as_ref(), false) {
                Ok(_) => ManagedWorktreeRegistration::Active,
                // The durable ledger still claims this exact path. Failure to
                // prove its checkout identity is stale/uncertain authority,
                // not evidence that the path is now an ordinary workspace.
                Err(_) => ManagedWorktreeRegistration::Unknown,
            }
        })
        .await
        .map_err(|error| {
            WorktreeServiceError::new(format!(
                "managed worktree verification task failed: {error}"
            ))
        })
    }

    async fn remove(
        &self,
        record: ManagedWorktreeRecord,
    ) -> Result<RemoveResult, WorktreeServiceError> {
        // This identity check, Git effect, and full-snapshot commit are one
        // transaction with session persistence. In particular, a same-key
        // persist cannot re-admit a checkout while Git is deleting it.
        let mutation = Arc::clone(&self.mutation_gate).lock_owned().await;
        self.ensure_ledger_certain()?;
        let recorded = self
            .ledger
            .records
            .lock()
            .map_err(|_| WorktreeServiceError::new("managed worktree inventory lock was poisoned"))?
            .contains(&record);
        if !recorded {
            return Ok(RemoveResult {
                removed: false,
                kept: Some(interface_registry::RemoveKept::Failed),
                changes: None,
                detail: Some("managed worktree registration is absent".into()),
            });
        }
        let git = Arc::clone(&self.git);
        let worktree = record.worktree();
        let result =
            tokio::task::spawn_blocking(move || remove_worktree(&worktree, git.as_ref(), false))
                .await
                .map_err(|error| {
                    WorktreeServiceError::outcome_unknown(format!(
                        "managed worktree cleanup task failed: {error}"
                    ))
                })?;
        if !result.removed {
            return Ok(result);
        }

        if let Err(error) = self.ensure_ledger_certain() {
            return Err(WorktreeServiceError::outcome_unknown(error.message));
        }
        let next = {
            let mut records = self.ledger.records.lock().map_err(|_| {
                WorktreeServiceError::outcome_unknown(
                    "the checkout was removed, but the managed worktree inventory lock was poisoned",
                )
            })?;
            let mut next = records.clone();
            let before = next.len();
            next.retain(|current| current != &record);
            if next.len() == before {
                return Ok(result);
            }
            if self.ledger.store.is_none() {
                *records = next;
                return Ok(result);
            }
            next
        };
        match self.commit_records(mutation, next, true).await {
            Ok(()) => Ok(result),
            Err(LedgerWriteFailure::StorePoisoned) => Err(WorktreeServiceError::outcome_unknown(
                "the checkout was removed, but the managed worktree ledger lock was poisoned",
            )),
            Err(LedgerWriteFailure::Write(error)) => {
                Err(WorktreeServiceError::outcome_unknown(format!(
                    "the checkout was removed, but its managed record could not be saved: {}",
                    error.message
                )))
            }
            Err(LedgerWriteFailure::WorkerFailed(error)) => {
                Err(WorktreeServiceError::outcome_unknown(format!(
                    "the checkout was removed, but its managed record could not be saved: managed worktree ledger write task failed: {error}"
                )))
            }
        }
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
