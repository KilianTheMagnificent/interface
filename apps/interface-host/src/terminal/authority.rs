//! Reopen-authority verification and the create/restart authority
//! transaction, as run from the async dispatch path.

use super::{
    ApprovedTerminalRoots, Arc, ColdReopenWorkspaceLease, CreateTerminalRequest, DispatchError,
    ManagedWorkspaceVerification, Path, PathBuf, ReopenAuthoritySource,
    ReopenAuthorityTransactionKind, ReopenAuthorityTransactionPhase, TerminalHostDispatcher,
    TerminalId, TerminalLaunchSessionV1, TerminalReopenAuthority,
    TerminalReopenAuthorityCompareExchange, TerminalReopenAuthoritySlot,
    TerminalReopenAuthorityState, TerminalService, TerminalSummary, TerminalWorkspaceTerminal,
    VerifiedManagedWorkspace, canonical_terminal_roots, desktop_error, terminal_state_error,
    terminal_worker_error,
};

impl TerminalHostDispatcher {
    pub(super) async fn approved_roots(
        &self,
        retained_managed_root: Option<PathBuf>,
    ) -> Result<ApprovedTerminalRoots, DispatchError> {
        let mut candidates = self.desktop.terminal_workspace_roots();
        candidates.extend(self.runtime.approved_session_roots().map_err(|error| {
            tracing::error!(kind = ?error.kind, "could not read current terminal session roots");
            authority_unavailable("Interface could not read the current terminal workspaces.")
        })?);
        candidates.extend(retained_managed_root);
        // Gathering the candidates is in-memory, but canonicalizing them is 3
        // `canonicalize` and 4 `symlink_metadata` calls each — one candidate
        // per workspace root plus one per approved session — so the burst runs
        // on a blocking worker, never on the async dispatch loop. The
        // per-candidate probe inside `canonical_terminal_roots` is deliberate
        // TOCTOU narrowing; do not collapse it.
        tokio::task::spawn_blocking(move || canonical_terminal_roots(candidates))
            .await
            .map_err(|error| {
                tracing::error!(%error, "terminal root authority task failed");
                DispatchError::new(
                    "terminal-worker-failed",
                    "The integrated terminal worker stopped unexpectedly.",
                    false,
                    false,
                )
            })?
    }

    pub(super) fn begin_managed_reopen_proof(
        &self,
        terminal_id: TerminalId,
        managed: &VerifiedManagedWorkspace,
    ) -> Result<(), DispatchError> {
        let mut proofs = self
            .pending_managed_reopen_proofs
            .lock()
            .map_err(|_| terminal_state_error())?;
        if proofs.contains_key(&terminal_id) {
            return Err(DispatchError::new(
                "terminal-authority-busy",
                "That terminal's managed-workspace proof is already being committed. Try again.",
                true,
                true,
            ));
        }
        proofs.insert(terminal_id, managed.clone());
        Ok(())
    }

    pub(super) fn finish_managed_reopen_proof(
        &self,
        terminal_id: TerminalId,
        managed: &VerifiedManagedWorkspace,
    ) {
        if let Ok(mut proofs) = self.pending_managed_reopen_proofs.lock()
            && proofs.get(&terminal_id) == Some(managed)
        {
            proofs.remove(&terminal_id);
        }
    }

    /// Opens a create or restart authority transaction. The whole critical
    /// section runs off the async workers, keeping its order: lifecycle
    /// lock, then the desktop gate for the write-free capture barrier.
    pub(super) async fn begin_reopen_authority_commit(
        &self,
        terminal_id: TerminalId,
        prior: Option<TerminalReopenAuthority>,
        kind: ReopenAuthorityTransactionKind,
    ) -> Result<(), DispatchError> {
        self.lifecycle_blocking(move |sink| {
            sink.begin_reopen_authority_commit(terminal_id, prior, kind)
        })
        .await
    }

    pub(super) async fn finish_reopen_authority_commit(&self, terminal_id: TerminalId) {
        let released = self
            .lifecycle_blocking(move |sink| {
                if let Ok(mut lifecycle) = sink.state.lock() {
                    lifecycle.queued_events.remove(&terminal_id);
                    lifecycle.end_transaction(terminal_id);
                }
                Ok(())
            })
            .await;
        if let Err(error) = released {
            tracing::error!(
                code = %error.code,
                id = %terminal_id,
                "terminal authority transaction was not released"
            );
        }
    }

    pub(super) async fn ensure_reopen_authority_idle(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        self.lifecycle_blocking(move |sink| {
            if sink
                .state
                .lock()
                .map_err(|_| terminal_state_error())?
                .transactions
                .contains_key(&terminal_id)
            {
                return Err(DispatchError::new(
                    "terminal-authority-busy",
                    "That terminal's restart proof is still being committed or rolled back. Try again.",
                    true,
                    true,
                ));
            }
            Ok(())
        })
        .await
    }

    pub(super) async fn begin_reopen_authority_rollback(
        &self,
        terminal_id: TerminalId,
    ) -> Result<Option<TerminalLaunchSessionV1>, DispatchError> {
        let sink = Arc::clone(&self.lifecycle_sink);
        tokio::task::spawn_blocking(move || {
            let mut lifecycle = sink.state.lock().map_err(|_| terminal_state_error())?;
            let transaction = lifecycle
                .transactions
                .get_mut(&terminal_id)
                .ok_or_else(|| {
                    DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's restart-proof transaction changed before rollback.",
                        true,
                        false,
                    )
                })?;
            if transaction.phase == ReopenAuthorityTransactionPhase::RollingBack {
                return Ok(reopen_authority_slot_launch_session(&transaction.prior));
            }
            let prior = transaction.prior.clone();
            transaction.phase = ReopenAuthorityTransactionPhase::RollingBack;
            Ok(reopen_authority_slot_launch_session(&prior))
        })
        .await
        .map_err(|error| terminal_worker_error(&error))?
    }

    pub(super) async fn ensure_reopen_authority_rollback(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        let sink = Arc::clone(&self.lifecycle_sink);
        tokio::task::spawn_blocking(move || sink.retry_authority_rollback(terminal_id))
            .await
            .map_err(|error| terminal_worker_error(&error))?
    }

    pub(super) async fn finish_reopen_authority_rollback(
        &self,
        terminal_id: TerminalId,
        expose_latest: bool,
    ) {
        let sink = Arc::clone(&self.lifecycle_sink);
        if let Err(error) = tokio::task::spawn_blocking(move || {
            sink.finish_rollback(terminal_id, expose_latest);
        })
        .await
        {
            tracing::error!(%error, id = %terminal_id, "terminal rollback worker failed");
        }
    }

    pub(super) async fn commit_reopen_authority(
        &self,
        expected: &TerminalSummary,
        managed: Option<&VerifiedManagedWorkspace>,
    ) -> Result<TerminalSummary, DispatchError> {
        // Provider receipt application updates the service summary before it
        // reaches the lifecycle sink. Taking the current snapshot while this
        // transaction lock is held therefore closes the last race: a receipt
        // already waiting to publish is included in the sidecar commit, while
        // a later receipt sees no pending transaction and persists normally.
        let sink = Arc::clone(&self.lifecycle_sink);
        let service = self.service.clone();
        let desktop = Arc::clone(&self.desktop);
        let expected = expected.clone();
        let managed = managed.cloned();
        tokio::task::spawn_blocking(move || {
            let mut lifecycle = sink.state.lock().map_err(|_| terminal_state_error())?;
            let prior = match lifecycle.transactions.get(&expected.id) {
                Some(transaction)
                    if transaction.phase == ReopenAuthorityTransactionPhase::Pending =>
                {
                    transaction.prior.clone()
                }
                _ => {
                    return Err(DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's restart-proof transaction changed before it committed.",
                        true,
                        false,
                    ));
                }
            };
            let summary = terminal_for_authority_commit(&service, &expected)?;
            let mut authority = TerminalReopenAuthority::from_summary(&summary);
            authority.managed_worktree_key = managed
                .as_ref()
                .map(|managed| managed.key.clone())
                .or_else(|| match &prior {
                    TerminalReopenAuthoritySlot::Authority(prior)
                        if prior.root == authority.root
                            && summary_matches_reopen_authority(&summary, &prior.terminal) =>
                    {
                        prior.managed_worktree_key.clone()
                    }
                    TerminalReopenAuthoritySlot::Absent
                    | TerminalReopenAuthoritySlot::Authority(_)
                    | TerminalReopenAuthoritySlot::PendingMigration(_) => None,
                });
            let attempted = TerminalReopenAuthoritySlot::Authority(authority);
            let transaction = lifecycle
                .transactions
                .get_mut(&expected.id)
                .expect("validated authority transaction exists");
            if transaction.phase != ReopenAuthorityTransactionPhase::Pending {
                return Err(DispatchError::new(
                    "terminal-authority-changed",
                    "That terminal's restart-proof transaction changed before it committed.",
                    true,
                    false,
                ));
            }
            // Record the exact bytes we are about to attempt before entering
            // the writer. A post-rename error can then roll back only this
            // value and never clobber a later receipt or migration winner.
            transaction.attempted = Some(attempted.clone());
            match desktop
                .compare_exchange_terminal_reopen_authority(
                    summary.id,
                    &prior,
                    &attempted,
                )
                .map_err(|error| desktop_error(error, false))?
            {
                TerminalReopenAuthorityCompareExchange::Exchanged
                | TerminalReopenAuthorityCompareExchange::AlreadyReplacement => {}
                TerminalReopenAuthorityCompareExchange::Conflict(_) => {
                    // A compare-exchange conflict is a definite no-write. Do
                    // not leave the attempted value armed: cleanup must not
                    // try to roll back bytes this transaction never owned.
                    transaction.attempted = None;
                    return Err(DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's restart proof changed before Interface could commit it. Try again.",
                        true,
                        false,
                    ));
                }
            }
            lifecycle.end_transaction(summary.id);
            if let Some(event) = lifecycle.queued_events.remove(&summary.id) {
                sink.deliver_event(&mut lifecycle, event);
            }
            Ok(summary)
        })
        .await
        .map_err(|error| terminal_worker_error(&error))?
    }

    pub(super) async fn verify_reopen_authority(
        &self,
        authority: &TerminalReopenAuthority,
        source: ReopenAuthoritySource,
    ) -> Result<Option<(VerifiedManagedWorkspace, ColdReopenWorkspaceLease)>, DispatchError> {
        let authority_root = PathBuf::from(&authority.root);
        let lease = ColdReopenWorkspaceLease::acquire(
            &self.desktop,
            &authority_root,
            authority.terminal.id,
        )?;
        let verified = match self
            .worktree_release
            .verify_managed_workspace(&authority_root, authority.managed_worktree_key.as_deref())
            .await
        {
            ManagedWorkspaceVerification::Verified(verified)
                if authority.managed_worktree_key.is_some() =>
            {
                Some(verified)
            }
            ManagedWorkspaceVerification::Verified(_) => {
                return Err(DispatchError::new(
                    "terminal-reopen-authority-stale",
                    "That saved terminal path is now owned by a managed workspace generation its restart proof does not identify.",
                    false,
                    true,
                ));
            }
            ManagedWorkspaceVerification::NotManaged
                if authority.managed_worktree_key.is_none() =>
            {
                None
            }
            ManagedWorkspaceVerification::NotManaged => {
                return Err(DispatchError::new(
                    "terminal-reopen-authority-stale",
                    "That saved terminal belongs to a different managed workspace generation.",
                    false,
                    true,
                ));
            }
            ManagedWorkspaceVerification::Unavailable => {
                return Err(authority_unavailable(
                    "Interface could not re-verify that saved terminal workspace. Try again.",
                ));
            }
        };
        if verified
            .as_ref()
            .is_some_and(|verified| verified.root != authority_root)
            || !Path::new(&authority.terminal.cwd).starts_with(&authority_root)
        {
            return Err(DispatchError::new(
                "terminal-reopen-authority-changed",
                "That saved terminal changed while Interface was verifying it. Try again.",
                true,
                true,
            ));
        }
        let current = match source {
            ReopenAuthoritySource::SavedRoster => saved_authority(
                self.reopen_authority_state(authority.terminal.id).await,
                "Interface could not finish reading that saved terminal's restart proof. Try again.",
                || {
                    DispatchError::new(
                        "terminal-reopen-authority-changed",
                        "That saved terminal changed while Interface was verifying it. Try again.",
                        true,
                        true,
                    )
                },
            )?,
            ReopenAuthoritySource::LiveUnsaved => self
                .live_reopen_authority(authority.terminal.id)
                .await?
                .ok_or_else(|| {
                    DispatchError::new(
                        "terminal-reopen-authority-changed",
                        "That live terminal lost its restart proof while Interface was verifying it.",
                        true,
                        true,
                    )
                })?,
        };
        if current.root != authority.root
            || current.managed_worktree_key != authority.managed_worktree_key
            || current.terminal.id != authority.terminal.id
            || current.terminal.cwd != authority.terminal.cwd
            || current.terminal.session_key != authority.terminal.session_key
            || current.terminal.launch != authority.terminal.launch
        {
            return Err(DispatchError::new(
                "terminal-reopen-authority-changed",
                "That terminal changed while Interface was verifying its restart proof. Try again.",
                true,
                true,
            ));
        }
        Ok(verified.map(|verified| (verified, lease)))
    }

    pub(super) async fn migrate_terminal_reopen_authority(
        &self,
        request: &CreateTerminalRequest,
        terminal: Option<TerminalWorkspaceTerminal>,
    ) -> Result<(), DispatchError> {
        let Some(terminal) = terminal else {
            return Ok(());
        };
        if !request_matches_reopen_authority(request, &terminal) {
            return Ok(());
        }
        let expected_key = terminal.session_key.as_deref();
        let verified = match self
            .worktree_release
            .verify_managed_workspace(Path::new(&terminal.cwd), expected_key)
            .await
        {
            ManagedWorkspaceVerification::Verified(managed) => {
                if expected_key.is_none() {
                    return Err(authority_unproven(
                        "That legacy saved terminal does not identify its original managed workspace generation.",
                    ));
                }
                Some(TerminalReopenAuthority {
                    root: managed.root.to_string_lossy().into_owned(),
                    managed_worktree_key: Some(managed.key),
                    terminal,
                })
            }
            ManagedWorkspaceVerification::NotManaged => {
                // A historical key mismatch must never fall through as a new
                // spawn and replay the old transcript under a reused path. A
                // second keyless lookup distinguishes that case from an
                // ordinary, non-managed saved terminal that needs no special
                // worktree authority.
                match self
                    .worktree_release
                    .verify_managed_workspace(Path::new(&terminal.cwd), None)
                    .await
                {
                    ManagedWorkspaceVerification::Verified(_) => {
                        return Err(DispatchError::new(
                            "terminal-reopen-authority-stale",
                            "That saved terminal belongs to a different managed workspace generation.",
                            false,
                            true,
                        ));
                    }
                    ManagedWorkspaceVerification::NotManaged => {
                        let approved = self.approved_roots(None).await?;
                        let Ok(root) = approved.owning_root(Path::new(&terminal.cwd)) else {
                            self.resolve_reopen_authority_migration(request.id, None)
                                .await?;
                            return Err(authority_unproven(
                                "That legacy saved terminal is outside Interface's current approved workspaces.",
                            ));
                        };
                        Some(TerminalReopenAuthority {
                            root: root.to_string_lossy().into_owned(),
                            managed_worktree_key: None,
                            terminal,
                        })
                    }
                    ManagedWorkspaceVerification::Unavailable => {
                        return Err(authority_unavailable(
                            "Interface could not verify that legacy saved worktree. Try again.",
                        ));
                    }
                }
            }
            ManagedWorkspaceVerification::Unavailable => {
                return Err(authority_unavailable(
                    "Interface could not verify that legacy saved worktree. Try again.",
                ));
            }
        };
        self.resolve_reopen_authority_migration(request.id, verified)
            .await
    }

    /// Resolves one startup migration candidate, a durable sidecar write,
    /// off the async workers.
    pub(super) async fn resolve_reopen_authority_migration(
        &self,
        id: TerminalId,
        verified: Option<TerminalReopenAuthority>,
    ) -> Result<(), DispatchError> {
        self.desktop_blocking(move |desktop| {
            desktop.resolve_terminal_reopen_authority_migration(id, verified.as_ref())
        })
        .await?
        .map(|_| ())
        .map_err(|error| desktop_error(error, true))
    }

    /// A live terminal's host-private restart proof, read off the async
    /// workers.
    pub(super) async fn live_reopen_authority(
        &self,
        id: TerminalId,
    ) -> Result<Option<TerminalReopenAuthority>, DispatchError> {
        self.desktop_blocking(move |desktop| desktop.live_terminal_reopen_authority(id))
            .await?
            .map_err(|error| desktop_error(error, true))
    }

    /// Reads a terminal's saved-roster reopen authority on the blocking pool:
    /// the desktop's terminal-state gate is held across durable roster writes.
    /// A failed worker reads as `Unavailable`, which every caller refuses.
    pub(super) async fn reopen_authority_state(
        &self,
        id: TerminalId,
    ) -> TerminalReopenAuthorityState {
        let desktop = Arc::clone(&self.desktop);
        tokio::task::spawn_blocking(move || desktop.terminal_reopen_authority_state(id))
            .await
            .unwrap_or(TerminalReopenAuthorityState::Unavailable)
    }
}

pub(super) fn terminal_for_authority_commit(
    service: &TerminalService,
    expected: &TerminalSummary,
) -> Result<TerminalSummary, DispatchError> {
    service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| {
            terminal.id == expected.id
                && terminal.generation == expected.generation
                && terminal.root == expected.root
                && terminal.cwd == expected.cwd
                && terminal.session_key == expected.session_key
                && terminal.launch == expected.launch
        })
        .ok_or_else(|| {
            DispatchError::new(
                "terminal-authority-changed",
                "The terminal changed before its restart proof could be committed.",
                true,
                false,
            )
        })
}

pub(super) fn request_matches_reopen_authority(
    request: &CreateTerminalRequest,
    authority: &TerminalWorkspaceTerminal,
) -> bool {
    request.id == authority.id
        && request.cwd == authority.cwd
        && request.session_key == authority.session_key
        && request.launch == authority.launch
}

pub(super) fn reopen_authority_slot_launch_session(
    slot: &TerminalReopenAuthoritySlot,
) -> Option<TerminalLaunchSessionV1> {
    match slot {
        TerminalReopenAuthoritySlot::Authority(authority) => {
            authority.terminal.launch_session.clone()
        }
        TerminalReopenAuthoritySlot::Absent | TerminalReopenAuthoritySlot::PendingMigration(_) => {
            None
        }
    }
}

pub(super) fn summary_matches_reopen_authority(
    summary: &TerminalSummary,
    authority: &TerminalWorkspaceTerminal,
) -> bool {
    summary.id == authority.id
        && summary.cwd == authority.cwd
        && summary.session_key == authority.session_key
        && summary.launch == authority.launch
}

/// `terminal-authority-unavailable`: a restart proof or workspace Interface
/// could not read or verify. Retryable, and nothing has changed.
pub(super) fn authority_unavailable(message: &'static str) -> DispatchError {
    DispatchError::new("terminal-authority-unavailable", message, true, true)
}

/// `terminal-reopen-authority-unproven`: no restart proof covers the
/// terminal. The renderer matches this code.
pub(super) fn authority_unproven(message: &'static str) -> DispatchError {
    DispatchError::new("terminal-reopen-authority-unproven", message, false, true)
}

/// The saved-roster authority a state holds. An unreadable roster is refused
/// as unavailable with `unavailable`; every other state with `otherwise`.
pub(super) fn saved_authority(
    state: TerminalReopenAuthorityState,
    unavailable: &'static str,
    otherwise: impl FnOnce() -> DispatchError,
) -> Result<TerminalReopenAuthority, DispatchError> {
    match state {
        TerminalReopenAuthorityState::Authority(authority) => Ok(authority),
        TerminalReopenAuthorityState::Unavailable => Err(authority_unavailable(unavailable)),
        TerminalReopenAuthorityState::Unsaved
        | TerminalReopenAuthorityState::PendingMigration(_)
        | TerminalReopenAuthorityState::Unproven => Err(otherwise()),
    }
}
