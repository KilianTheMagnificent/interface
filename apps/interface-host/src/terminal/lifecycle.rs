//! Durable reopen-authority transactions and lifecycle publication.
//! Lock order remains lifecycle state -> input lanes -> input admission.
use super::{
    BridgeError, CHANNELS, DispatchError, HostTerminalLifecycleSink, HostTerminalLifecycleState,
    PathBuf, PushFrame, ReopenAuthorityTransaction, ReopenAuthorityTransactionKind,
    ReopenAuthorityTransactionPhase, SavedTerminalProviderSessionPersistence, TerminalId,
    TerminalLifecycleEvent, TerminalLifecycleSink, TerminalProviderSessionPersistence,
    TerminalReopenAuthority, TerminalReopenAuthorityCompareExchange, TerminalReopenAuthoritySlot,
    TerminalSummary, desktop_error, terminal_lifecycle_id, terminal_state_error,
};

impl HostTerminalLifecycleState {
    pub(super) fn begin_transaction(
        &mut self,
        id: TerminalId,
        transaction: ReopenAuthorityTransaction,
    ) {
        self.transactions.insert(id, transaction);
        self.sync_admission(id);
    }

    pub(super) fn end_transaction(&mut self, id: TerminalId) {
        self.transactions.remove(&id);
        self.sync_admission(id);
    }

    pub(super) fn show(&mut self, terminal: &TerminalSummary) {
        self.visible_terminals.insert(terminal.id, terminal.clone());
        self.sync_admission(terminal.id);
    }

    pub(super) fn hide(&mut self, id: TerminalId) {
        self.visible_terminals.remove(&id);
        self.sync_admission(id);
    }

    pub(super) fn sync_admission(&self, id: TerminalId) {
        let admitted = if self.transactions.contains_key(&id) {
            None
        } else {
            self.visible_terminals
                .get(&id)
                .map(|terminal| terminal.generation)
        };
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match admitted {
            Some(generation) => {
                admission.insert(id, generation);
            }
            None => {
                admission.remove(&id);
            }
        }
    }
}

impl HostTerminalLifecycleSink {
    pub(super) fn send_event(&self, event: TerminalLifecycleEvent) {
        let frame = serde_json::to_value(event)
            .map_err(|error| {
                BridgeError::new(
                    "terminal-event-encode-failed",
                    error.to_string(),
                    false,
                    true,
                )
            })
            .and_then(|payload| PushFrame::new(CHANNELS.terminal_lifecycle, payload));
        match frame {
            Ok(frame) => {
                if let Err(error) = self.pushes.blocking_send(frame) {
                    tracing::warn!(%error, "terminal lifecycle receiver closed");
                }
            }
            Err(error) => {
                tracing::error!(code = %error.code, "could not encode terminal lifecycle event");
            }
        }
    }

    pub(super) fn deliver_event(
        &self,
        state: &mut HostTerminalLifecycleState,
        mut event: TerminalLifecycleEvent,
    ) {
        state.delivered_revision = state.delivered_revision.saturating_add(1);
        match &mut event {
            TerminalLifecycleEvent::Upsert { revision, terminal } => {
                *revision = state.delivered_revision;
                state.show(terminal);
                self.remote_hub
                    .replay
                    .show((terminal.id.as_uuid(), terminal.generation.0));
                let (id, generation) = (terminal.id, terminal.generation);
                if let Ok(mut tails) = self.output_tails.lock() {
                    tails.retain(|(tail_id, tail_generation), _| {
                        *tail_id != id || *tail_generation >= generation
                    });
                }
                // Protocol 50: Rust decides the shell handoff here, for every
                // visible clean agent exit, whatever the window shows.
                if interface_terminal::shell_handoff::ended_on_its_own_terms(terminal)
                    && let Some(handoffs) = self.shell_handoffs.get()
                {
                    let _ = handoffs.send((id, generation));
                }
            }
            TerminalLifecycleEvent::Removed { revision, id, .. } => {
                *revision = state.delivered_revision;
                state.hide(*id);
                // Dropping the only sender lets the lane drain and exit.
                if let Ok(mut lanes) = self.input_lanes.lock() {
                    lanes.remove(id);
                }
                if let Ok(mut tails) = self.output_tails.lock() {
                    tails.retain(|(tail_id, _), _| tail_id != id);
                }
                self.remote_hub.replay.hide(id.as_uuid());
            }
        }
        self.send_event(event);
    }

    pub(super) fn begin_reopen_authority_commit(
        &self,
        terminal_id: TerminalId,
        prior: Option<TerminalReopenAuthority>,
        kind: ReopenAuthorityTransactionKind,
    ) -> Result<(), DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        if lifecycle.transactions.contains_key(&terminal_id) {
            return Err(DispatchError::new(
                "terminal-authority-busy",
                "That terminal's restart proof is already being committed. Try again.",
                true,
                true,
            ));
        }
        let prior = TerminalReopenAuthoritySlot::from(prior);
        match self
            .desktop
            .compare_exchange_terminal_reopen_authority(terminal_id, &prior, &prior)
            .map_err(|error| desktop_error(error, true))?
        {
            TerminalReopenAuthorityCompareExchange::AlreadyReplacement => {}
            TerminalReopenAuthorityCompareExchange::Exchanged => {
                unreachable!("equal authority slots cannot require a write")
            }
            TerminalReopenAuthorityCompareExchange::Conflict(_) => {
                return Err(DispatchError::new(
                    "terminal-authority-changed",
                    "That terminal's restart proof changed while Interface was verifying it. Try again.",
                    true,
                    true,
                ));
            }
        }
        lifecycle.begin_transaction(
            terminal_id,
            ReopenAuthorityTransaction {
                kind,
                phase: ReopenAuthorityTransactionPhase::Pending,
                prior,
                attempted: None,
            },
        );
        Ok(())
    }

    pub(super) fn begin_dismiss_authority_transaction(
        &self,
        terminal_id: TerminalId,
    ) -> Result<TerminalReopenAuthoritySlot, DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        if lifecycle.transactions.contains_key(&terminal_id) {
            return Err(DispatchError::new(
                "terminal-authority-busy",
                "That terminal's restart proof is already being committed. Try again.",
                true,
                true,
            ));
        }
        // Hold the lifecycle lock while capturing the exact private slot and
        // installing the buffer. Provider receipts take this lock before they
        // touch the sidecar, so none can slip between those two operations.
        // Renderer roster narrowing may still win first; the pre-removal
        // compare-and-exchange detects that without holding a desktop lock
        // across service event publication.
        let prior = self
            .desktop
            .terminal_reopen_authority_slot(terminal_id)
            .map_err(|error| desktop_error(error, true))?;
        lifecycle.begin_transaction(
            terminal_id,
            ReopenAuthorityTransaction {
                kind: ReopenAuthorityTransactionKind::Dismiss,
                phase: ReopenAuthorityTransactionPhase::Pending,
                prior: prior.clone(),
                attempted: None,
            },
        );
        Ok(prior)
    }

    pub(super) fn finish_rollback(&self, terminal_id: TerminalId, expose_latest: bool) {
        if let Ok(mut lifecycle) = self.state.lock() {
            let rolling_back =
                lifecycle
                    .transactions
                    .get(&terminal_id)
                    .is_some_and(|transaction| {
                        transaction.phase == ReopenAuthorityTransactionPhase::RollingBack
                            && transaction.attempted.is_none()
                    });
            if rolling_back {
                let latest = lifecycle.queued_events.remove(&terminal_id);
                lifecycle.end_transaction(terminal_id);
                if expose_latest && let Some(event) = latest {
                    self.deliver_event(&mut lifecycle, event);
                }
            }
        }
    }

    /// Whether rollback still owns `terminal_id`'s transaction. An unreadable
    /// state counts as owned, so a background retry never gives up on it.
    pub(super) fn rollback_pending(&self, terminal_id: TerminalId) -> bool {
        self.state.lock().map_or(true, |lifecycle| {
            lifecycle
                .transactions
                .get(&terminal_id)
                .is_some_and(|transaction| {
                    transaction.phase == ReopenAuthorityTransactionPhase::RollingBack
                })
        })
    }

    pub(super) fn retry_authority_rollback(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        let transaction = lifecycle
            .transactions
            .get_mut(&terminal_id)
            .ok_or_else(|| {
                DispatchError::new(
                    "terminal-authority-changed",
                    "That terminal's restart-proof transaction changed during rollback.",
                    true,
                    false,
                )
            })?;
        if transaction.phase != ReopenAuthorityTransactionPhase::RollingBack {
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "That terminal's restart-proof transaction is not owned by rollback.",
                true,
                false,
            ));
        }
        let Some(attempted) = transaction.attempted.clone() else {
            return Ok(());
        };
        let prior = transaction.prior.clone();
        match self
            .desktop
            .rollback_terminal_reopen_authority(terminal_id, &prior, &attempted)
        {
            Ok(_) => {}
            Err(error) if error.code == "terminal-reopen-authority-rollback-conflict" => {
                // A third exact value won while this transaction's disk write
                // was uncertain (for example renderer roster narrowing).
                // The desktop store preserved and reloaded that winner. It is
                // safe to retire this attempt, but never to overwrite it with
                // the older prior value or retry forever.
                tracing::warn!(
                    id = %terminal_id,
                    "preserved newer terminal authority during exact rollback"
                );
            }
            Err(error) => {
                return Err(DispatchError::new(
                    error.code,
                    error.message,
                    error.retryable,
                    false,
                ));
            }
        }
        // This lifecycle lock is the provider-persistence admission gate, so
        // the exact transaction cannot advance while the disk reconciliation
        // runs. Clearing `attempted` is the in-memory durable fence that must
        // precede receipt retirement and transaction release.
        transaction.attempted = None;
        Ok(())
    }

    pub(super) fn remove_dismiss_authority(
        &self,
        terminal_id: TerminalId,
        prior: &TerminalReopenAuthoritySlot,
    ) -> Result<(), DispatchError> {
        {
            let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
            let transaction = lifecycle
                .transactions
                .get_mut(&terminal_id)
                .filter(|transaction| {
                    transaction.phase == ReopenAuthorityTransactionPhase::Pending
                        && transaction.prior == *prior
                        && transaction.attempted.is_none()
                })
                .ok_or_else(|| {
                    DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's dismissal transaction changed before authority removal.",
                        true,
                        false,
                    )
                })?;
            // Record the exact value before entering the writer. A write error
            // may happen after rename, so rollback must know which bytes are
            // safe to compare against without guessing.
            transaction.attempted = Some(TerminalReopenAuthoritySlot::Absent);
        }

        let outcome = self
            .desktop
            .compare_exchange_terminal_reopen_authority(
                terminal_id,
                prior,
                &TerminalReopenAuthoritySlot::Absent,
            )
            .map_err(|error| desktop_error(error, false));
        match outcome {
            Ok(TerminalReopenAuthorityCompareExchange::Exchanged) => Ok(()),
            Ok(TerminalReopenAuthorityCompareExchange::AlreadyReplacement) => {
                // Workspace deletion can revoke this proof after dismissal
                // captures it but before its worker runs. Absence already
                // satisfies removal. Adopt it as the rollback baseline too:
                // a later capture-cleanup failure must not resurrect the
                // authority independently removed by the workspace save.
                let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
                let transaction = lifecycle
                    .transactions
                    .get_mut(&terminal_id)
                    .filter(|transaction| {
                        transaction.phase == ReopenAuthorityTransactionPhase::Pending
                            && transaction.prior == *prior
                            && transaction.attempted == Some(TerminalReopenAuthoritySlot::Absent)
                    })
                    .ok_or_else(terminal_state_error)?;
                transaction.prior = TerminalReopenAuthoritySlot::Absent;
                Ok(())
            }
            Ok(TerminalReopenAuthorityCompareExchange::Conflict(_)) => {
                // A definite mismatch performed no write. Clear the attempted
                // marker so abort can release the lifecycle buffer without an
                // unsafe absent->prior rollback.
                if let Ok(mut lifecycle) = self.state.lock()
                    && let Some(transaction) = lifecycle.transactions.get_mut(&terminal_id)
                    && transaction.phase == ReopenAuthorityTransactionPhase::Pending
                    && transaction.prior == *prior
                {
                    transaction.attempted = None;
                }
                Err(DispatchError::new(
                    "terminal-authority-changed",
                    "That terminal's restart authority changed before dismissal.",
                    true,
                    true,
                ))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn conclude_dismiss_transaction(
        &self,
        terminal_id: TerminalId,
        committed: bool,
    ) -> Result<Option<TerminalLifecycleEvent>, DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        let owned = lifecycle
            .transactions
            .get(&terminal_id)
            .is_some_and(|transaction| {
                if committed {
                    transaction.phase == ReopenAuthorityTransactionPhase::Pending
                        && transaction.attempted == Some(TerminalReopenAuthoritySlot::Absent)
                } else {
                    transaction.attempted.is_none()
                }
            });
        if !owned {
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "That terminal's dismissal transaction changed before it completed.",
                true,
                false,
            ));
        }
        let queued = lifecycle.queued_events.remove(&terminal_id);
        lifecycle.end_transaction(terminal_id);
        if committed {
            if let Some(event) = queued {
                if matches!(event, TerminalLifecycleEvent::Removed { .. }) {
                    // The sidecar is already durably absent. Deliver directly
                    // so the ordinary Removed path cannot race a second write.
                    self.deliver_event(&mut lifecycle, event);
                } else {
                    tracing::error!(
                        id = %terminal_id,
                        "discarded an upsert superseded by a committed terminal dismissal"
                    );
                }
            }
            Ok(None)
        } else {
            Ok(queued)
        }
    }

    pub(super) fn shutdown_transactions(
        &self,
    ) -> Result<Vec<(TerminalId, ReopenAuthorityTransaction)>, DispatchError> {
        let lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        Ok(lifecycle
            .transactions
            .iter()
            .map(|(id, transaction)| (*id, transaction.clone()))
            .collect())
    }

    pub(super) fn prepare_shutdown_rollback(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        if let Some(transaction) = lifecycle.transactions.get_mut(&terminal_id) {
            transaction.phase = ReopenAuthorityTransactionPhase::RollingBack;
        }
        Ok(())
    }

    pub(super) fn finish_shutdown_transaction(&self, terminal_id: TerminalId) {
        if let Ok(mut lifecycle) = self.state.lock() {
            lifecycle.queued_events.remove(&terminal_id);
            lifecycle.end_transaction(terminal_id);
        }
        if let Ok(mut proofs) = self.pending_managed_reopen_proofs.lock() {
            proofs.remove(&terminal_id);
        }
    }

    pub(super) fn commit_shutdown_dismissal(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        let mut lifecycle = self.state.lock().map_err(|_| terminal_state_error())?;
        let Some(transaction) = lifecycle.transactions.get_mut(&terminal_id) else {
            return Ok(());
        };
        if transaction.kind != ReopenAuthorityTransactionKind::Dismiss {
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "A non-dismissal terminal transaction reached dismissal recovery.",
                false,
                false,
            ));
        }
        let prior = transaction.prior.clone();
        let absent = TerminalReopenAuthoritySlot::Absent;
        transaction.attempted = Some(absent.clone());
        match self
            .desktop
            .compare_exchange_terminal_reopen_authority(terminal_id, &prior, &absent)
            .map_err(|error| desktop_error(error, false))?
        {
            TerminalReopenAuthorityCompareExchange::Exchanged
            | TerminalReopenAuthorityCompareExchange::AlreadyReplacement => Ok(()),
            TerminalReopenAuthorityCompareExchange::Conflict(_) => {
                // A concurrent exact winner performed the last durable write;
                // preserve it instead of broadening shutdown into overwrite.
                transaction.attempted = None;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
impl HostTerminalLifecycleSink {
    fn run_durable_write_hook(&self) {
        let hook = self
            .durable_write_hook
            .lock()
            .ok()
            .and_then(|hook| hook.clone());
        if let Some(hook) = hook {
            hook();
        }
    }
}

impl TerminalLifecycleSink for HostTerminalLifecycleSink {
    fn publish(&self, event: TerminalLifecycleEvent) {
        let Ok(mut lifecycle) = self.state.lock() else {
            tracing::error!("terminal lifecycle transaction state is unavailable");
            return;
        };
        let terminal_id = terminal_lifecycle_id(&event);
        let transaction_pending = lifecycle.transactions.contains_key(&terminal_id);
        match &event {
            TerminalLifecycleEvent::Upsert { terminal, .. } => {
                if transaction_pending {
                    tracing::warn!(
                        id = %terminal.id,
                        "buffered lifecycle state for an uncommitted terminal"
                    );
                } else {
                    let Ok(proofs) = self.pending_managed_reopen_proofs.lock() else {
                        tracing::error!(
                            id = %terminal.id,
                            "suppressed reopen authority because managed proof state is unavailable"
                        );
                        self.deliver_event(&mut lifecycle, event);
                        return;
                    };
                    let managed = proofs.get(&terminal.id).cloned();
                    drop(proofs);
                    #[cfg(test)]
                    self.run_durable_write_hook();
                    let result = match managed.as_ref() {
                        Some(managed) if managed.root == PathBuf::from(&terminal.root) => Some(
                            self.desktop
                                .record_terminal_managed_reopen_authority(terminal, &managed.key),
                        ),
                        Some(_) => {
                            tracing::error!(
                                id = %terminal.id,
                                "suppressed keyless authority for a managed terminal whose root changed"
                            );
                            None
                        }
                        None => Some(self.desktop.record_terminal_reopen_authority(terminal)),
                    };
                    match result {
                        Some(Ok(_)) => {
                            if managed.is_some()
                                && let Ok(mut proofs) = self.pending_managed_reopen_proofs.lock()
                                && proofs.get(&terminal.id) == managed.as_ref()
                            {
                                proofs.remove(&terminal.id);
                            }
                        }
                        Some(Err(error)) => {
                            tracing::error!(
                                code = %error.code,
                                id = %terminal.id,
                                "terminal cold-reopen authority was not committed"
                            );
                        }
                        None => {}
                    }
                }
            }
            TerminalLifecycleEvent::Removed { id, .. } => {
                if let Ok(mut proofs) = self.pending_managed_reopen_proofs.lock() {
                    proofs.remove(id);
                }
                if !transaction_pending
                    && let Err(error) = self.desktop.remove_terminal_reopen_authority(*id)
                {
                    tracing::error!(
                        code = %error.code,
                        id = %id,
                        "terminal cold-reopen authority was not removed"
                    );
                }
            }
        }
        if transaction_pending {
            lifecycle.queued_events.insert(terminal_id, event);
        } else {
            self.deliver_event(&mut lifecycle, event);
        }
    }

    fn persist_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        let Ok(lifecycle) = self.state.lock() else {
            return TerminalProviderSessionPersistence::RetryableFailure;
        };
        let managed_proof_pending = self
            .pending_managed_reopen_proofs
            .lock()
            .map_or(true, |proofs| proofs.contains_key(&terminal.id));
        if lifecycle.transactions.contains_key(&terminal.id) || managed_proof_pending {
            return TerminalProviderSessionPersistence::RetryableFailure;
        }
        match self.desktop.persist_terminal_provider_reopen(terminal) {
            Ok(SavedTerminalProviderSessionPersistence::Durable) => {
                TerminalProviderSessionPersistence::Durable
            }
            Ok(SavedTerminalProviderSessionPersistence::NotSaved) => {
                TerminalProviderSessionPersistence::NotSaved
            }
            Err(error) => {
                tracing::error!(
                    code = %error.code,
                    id = %terminal.id,
                    "terminal provider reopen identity was not filed in the saved-workspace store"
                );
                TerminalProviderSessionPersistence::RetryableFailure
            }
        }
    }
}
