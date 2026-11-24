//! Terminal dismissal and its reopen-authority transaction.

use super::{
    Arc, CHANNELS, CleanupRetry, DismissTerminalRequest, DispatchError, HostTerminalLifecycleSink,
    ReopenAuthorityTransactionPhase, TerminalHostDispatcher, TerminalId, TerminalLifecycleSink,
    TerminalReopenAuthoritySlot, Value, blocking_step, decode_payload, encode_result,
    terminal_closed_error, terminal_dispatch_error, terminal_state_error, terminal_worker_error,
};

impl TerminalHostDispatcher {
    /// Dismisses a terminal and removes its restart proof in one transaction.
    pub(super) async fn dismiss_terminal(&self, payload: Value) -> Result<Value, DispatchError> {
        let request = decode_payload::<DismissTerminalRequest>(CHANNELS.dismiss_terminal, payload)?;
        let terminal_id = request.id;
        let prior = self
            .begin_dismiss_authority_transaction(terminal_id)
            .await?;
        let Ok(permit) = Arc::clone(&self.blocking_gate).acquire_owned().await else {
            self.finish_dismiss_authority_transaction(terminal_id, false)
                .await?;
            return Err(terminal_closed_error());
        };
        let service = self.service.clone();
        let lifecycle = Arc::clone(&self.lifecycle_sink);
        let callback_prior = prior.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            service.dismiss_after(request, || {
                lifecycle.remove_dismiss_authority(terminal_id, &callback_prior)
            })
        })
        .await;
        match outcome {
            Ok(Ok(Ok(result))) => {
                self.finish_dismiss_authority_transaction(terminal_id, true)
                    .await?;
                encode_result(result)
            }
            Ok(Ok(Err(error))) => {
                self.rollback_dismiss_authority_transaction(terminal_id)
                    .await?;
                Err(terminal_dispatch_error(&error))
            }
            Ok(Err(error)) => {
                self.rollback_dismiss_authority_transaction(terminal_id)
                    .await?;
                Err(error)
            }
            Err(error) => {
                // A panicking worker may have removed the sidecar before
                // it stopped. Keep a queued Upsert hidden rather than
                // accidentally re-authorizing a terminal whose service
                // mutation is unknown.
                let _ = self
                    .finish_dismiss_authority_transaction(terminal_id, true)
                    .await;
                Err(terminal_worker_error(&error))
            }
        }
    }

    /// Opens a dismissal transaction off the async workers; see
    /// [`HostTerminalLifecycleSink::begin_dismiss_authority_transaction`].
    pub(super) async fn begin_dismiss_authority_transaction(
        &self,
        terminal_id: TerminalId,
    ) -> Result<TerminalReopenAuthoritySlot, DispatchError> {
        self.lifecycle_blocking(move |sink| sink.begin_dismiss_authority_transaction(terminal_id))
            .await
    }

    pub(super) async fn finish_dismiss_authority_transaction(
        &self,
        terminal_id: TerminalId,
        committed: bool,
    ) -> Result<(), DispatchError> {
        let sink = Arc::clone(&self.lifecycle_sink);
        tokio::task::spawn_blocking(move || {
            let replay = sink.conclude_dismiss_transaction(terminal_id, committed)?;
            // A failed action can have raced a legitimate provider update
            // before it acquired the service event gate. Re-submit that
            // coalesced state only after rollback restored its prior sidecar.
            if let Some(event) = replay {
                sink.publish(event);
            }
            Ok(())
        })
        .await
        .map_err(|error| terminal_worker_error(&error))?
    }

    pub(super) async fn rollback_dismiss_authority_transaction(
        &self,
        terminal_id: TerminalId,
    ) -> Result<(), DispatchError> {
        self.lifecycle_blocking(move |sink| {
            let mut lifecycle = sink.state.lock().map_err(|_| terminal_state_error())?;
            let transaction = lifecycle
                .transactions
                .get_mut(&terminal_id)
                .ok_or_else(|| {
                    DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's dismissal transaction disappeared during rollback.",
                        true,
                        false,
                    )
                })?;
            transaction.phase = ReopenAuthorityTransactionPhase::RollingBack;
            Ok(())
        })
        .await?;
        match self.ensure_reopen_authority_rollback(terminal_id).await {
            Ok(()) => {
                self.finish_dismiss_authority_transaction(terminal_id, false)
                    .await
            }
            Err(error) => {
                self.schedule_dismiss_authority_rollback(terminal_id);
                Err(error)
            }
        }
    }

    pub(super) fn schedule_dismiss_authority_rollback(&self, terminal_id: TerminalId) {
        tokio::spawn(finish_dismiss_authority_rollback(
            Arc::clone(&self.lifecycle_sink),
            terminal_id,
        ));
    }
}

/// Finishes a failed dismissal's rollback in the background, retrying with a
/// capped backoff for as long as rollback still owns the transaction.
/// `terminal-authority-changed` is retryable, so the transaction itself,
/// read under the lifecycle lock, decides when there is nothing left to do.
pub(super) async fn finish_dismiss_authority_rollback(
    sink: Arc<HostTerminalLifecycleSink>,
    terminal_id: TerminalId,
) {
    let mut retry = CleanupRetry::default();
    loop {
        let rollback = Arc::clone(&sink);
        let Err(failure) = blocking_step(move || {
            rollback
                .retry_authority_rollback(terminal_id)
                .map_err(|error| error.code)
        })
        .await
        else {
            break;
        };
        let owner = Arc::clone(&sink);
        let pending = tokio::task::spawn_blocking(move || owner.rollback_pending(terminal_id))
            .await
            .unwrap_or(true);
        if !pending {
            return;
        }
        retry
            .fail(
                terminal_id,
                "retrying dismissal authority rollback",
                &failure,
            )
            .await;
    }
    let completed = tokio::task::spawn_blocking(move || {
        let replay = sink.conclude_dismiss_transaction(terminal_id, false)?;
        if let Some(event) = replay {
            sink.publish(event);
        }
        Ok::<(), DispatchError>(())
    })
    .await;
    match completed {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(
            code = %error.code,
            id = %terminal_id,
            "dismissal rollback could not release its lifecycle transaction"
        ),
        Err(error) => tracing::error!(
            %error,
            id = %terminal_id,
            "dismissal rollback completion worker failed"
        ),
    }
}
