//! The shared post-spawn tail of create and restart: exposure of an
//! uncommitted generation and its rollback when that fails.

use super::{
    Arc, CleanupRetry, DispatchError, Duration, TerminalDataSender, TerminalGenerationRequest,
    TerminalHostDispatcher, TerminalLaunchSessionV1, TerminalState, TerminalSummary,
    VerifiedManagedWorkspace, blocking_step, not_replay_safe, terminal_worker_error,
};

const UNCOMMITTED_TERMINAL_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The log lines create and restart use for the shared exposure tail.
pub(super) struct SpawnExposure {
    transport_closed: &'static str,
    no_output_owner: &'static str,
    not_committed: &'static str,
    /// Only create logs the failed commit itself before cleaning up.
    log_commit_failure: bool,
}

pub(super) const CREATE_EXPOSURE: SpawnExposure = SpawnExposure {
    transport_closed: "terminal spawned after transport closure did not close cleanly",
    no_output_owner: "terminal without an output owner did not close cleanly",
    not_committed: "uncommitted terminal did not close cleanly",
    log_commit_failure: true,
};

pub(super) const RESTART_EXPOSURE: SpawnExposure = SpawnExposure {
    transport_closed: "restarted terminal spawned after transport closure did not close cleanly",
    no_output_owner: "restarted terminal without an output owner did not close cleanly",
    not_committed: "restarted terminal without durable authority did not close cleanly",
    log_commit_failure: false,
};

impl TerminalHostDispatcher {
    pub(super) async fn cleanup_uncommitted_terminal(
        &self,
        summary: &TerminalSummary,
        dismiss_after_settle: bool,
    ) -> Result<(), DispatchError> {
        let prior_launch_session = self.begin_reopen_authority_rollback(summary.id).await?;
        let generation = TerminalGenerationRequest {
            id: summary.id,
            generation: summary.generation,
        };
        // Every failure below hands the rollback to the background retry
        // before returning. The restore branch moves the prior binding and
        // returns, so this borrow never outlives it.
        let defer = |error: DispatchError| {
            self.schedule_uncommitted_terminal_cleanup(
                generation,
                dismiss_after_settle,
                prior_launch_session.clone(),
            );
            error
        };
        if let Err(error) = self
            .run_blocking(move |service| service.close(generation))
            .await
        {
            return Err(defer(not_replay_safe(error)));
        }
        let settled = tokio::time::timeout(UNCOMMITTED_TERMINAL_SETTLE_TIMEOUT, async {
            loop {
                let settled = self
                    .service
                    .snapshot()
                    .terminals
                    .into_iter()
                    .find(|terminal| terminal.id == summary.id)
                    .is_none_or(|terminal| {
                        terminal.generation != summary.generation
                            || (terminal.state != TerminalState::Running
                                && terminal.state != TerminalState::Starting)
                    });
                if settled {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if settled.is_err() {
            return Err(defer(DispatchError::new(
                "terminal-authority-cleanup-timeout",
                "Interface is still closing an uncommitted terminal safely in the background.",
                true,
                false,
            )));
        }
        let service = self.service.clone();
        let publication_settled =
            tokio::task::spawn_blocking(move || service.generation_has_settled(generation))
                .await
                .map_err(|error| terminal_worker_error(&error))?;
        if !publication_settled {
            return Err(defer(DispatchError::new(
                "terminal-authority-cleanup-raced",
                "Interface is still closing an uncommitted terminal safely in the background.",
                true,
                false,
            )));
        }
        if let Err(error) = self.ensure_reopen_authority_rollback(summary.id).await {
            return Err(defer(error));
        }
        if !dismiss_after_settle {
            self.run_blocking(move |service| {
                service.restore_settled_provider_session_binding(generation, prior_launch_session)
            })
            .await?;
            self.finish_reopen_authority_rollback(summary.id, true)
                .await;
            return Ok(());
        }
        if let Err(error) = self
            .run_blocking(move |service| service.abandon_generation(generation))
            .await
        {
            return Err(defer(not_replay_safe(error)));
        }
        // This generation was never visible, so removal is deliberately
        // lifecycle-silent and must be finished here.
        self.finish_reopen_authority_rollback(summary.id, false)
            .await;
        Ok(())
    }

    /// Exposes a spawned, still uncommitted generation: rechecks the
    /// transport, claims its output, commits its reopen authority, then
    /// admits the output. Any failure closes the generation (dismissing it
    /// for a create) and returns the error. The order is load-bearing and
    /// shared by create and restart; each caller keeps its workspace guards
    /// alive until this returns.
    pub(super) async fn expose_spawned(
        &self,
        summary: TerminalSummary,
        sender: TerminalDataSender,
        managed: Option<&VerifiedManagedWorkspace>,
        dismiss_on_failure: bool,
        labels: &SpawnExposure,
    ) -> Result<TerminalSummary, DispatchError> {
        let output_generation = TerminalGenerationRequest {
            id: summary.id,
            generation: summary.generation,
        };
        if let Err(error) = self.ensure_transport() {
            if let Some(managed) = managed {
                self.finish_managed_reopen_proof(summary.id, managed);
            }
            self.cleanup_uncommitted_or_log(&summary, dismiss_on_failure, labels.transport_closed)
                .await;
            return Err(error);
        }
        // As before, a pump that cannot start leaves the managed proof to
        // cleanup, and its error is re-marked as not replay-safe.
        if let Err(error) = self.start_output_pump(output_generation, sender) {
            self.cleanup_uncommitted_or_log(&summary, dismiss_on_failure, labels.no_output_owner)
                .await;
            return Err(not_replay_safe(error));
        }
        let summary = match self.commit_reopen_authority(&summary, managed).await {
            Ok(committed) => committed,
            Err(error) => {
                self.cancel_output_pump(output_generation);
                if let Some(managed) = managed {
                    self.finish_managed_reopen_proof(summary.id, managed);
                }
                if labels.log_commit_failure {
                    tracing::error!(
                        code = %error.code,
                        id = %summary.id,
                        "terminal cold-reopen proof was not committed"
                    );
                }
                self.cleanup_uncommitted_or_log(&summary, dismiss_on_failure, labels.not_committed)
                    .await;
                return Err(error);
            }
        };
        if let Err(error) = self.admit_output_pump(output_generation) {
            self.mark_transport_failed(&error.message);
            return Err(error);
        }
        if let Some(managed) = managed {
            self.finish_managed_reopen_proof(summary.id, managed);
        }
        Ok(summary)
    }

    /// Closes an uncommitted generation whose caller is already failing,
    /// logging rather than returning a cleanup failure.
    pub(super) async fn cleanup_uncommitted_or_log(
        &self,
        summary: &TerminalSummary,
        dismiss_after_settle: bool,
        failure: &'static str,
    ) {
        if let Err(error) = self
            .cleanup_uncommitted_terminal(summary, dismiss_after_settle)
            .await
        {
            tracing::error!(code = %error.code, id = %summary.id, "{failure}");
        }
    }

    /// Finishes an uncommitted rollback in the background. It never gives
    /// up: every failed step is retried, with a capped backoff, until the
    /// generation is gone or superseded, or its binding is settled.
    pub(super) fn schedule_uncommitted_terminal_cleanup(
        &self,
        generation: TerminalGenerationRequest,
        dismiss_after_settle: bool,
        prior_launch_session: Option<TerminalLaunchSessionV1>,
    ) {
        let service = self.service.clone();
        let lifecycle = Arc::clone(&self.lifecycle_sink);
        tokio::spawn(async move {
            let mut retry = CleanupRetry::default();
            loop {
                let rollback = Arc::clone(&lifecycle);
                if let Err(failure) = blocking_step(move || {
                    rollback
                        .retry_authority_rollback(generation.id)
                        .map_err(|error| error.code)
                })
                .await
                {
                    retry
                        .fail(
                            generation.id,
                            "retrying exact terminal authority rollback",
                            &failure,
                        )
                        .await;
                    continue;
                }
                // This rollback owns exactly one failed spawn. A later
                // successful restart may reuse the ID, but never its
                // generation, so stale cleanup must become inert immediately.
                let Some(terminal) = service
                    .snapshot()
                    .terminals
                    .into_iter()
                    .find(|terminal| terminal.id == generation.id)
                    .filter(|terminal| terminal.generation == generation.generation)
                else {
                    break;
                };
                if terminal.state == TerminalState::Running
                    || terminal.state == TerminalState::Starting
                {
                    let closing = service.clone();
                    let _ = tokio::task::spawn_blocking(move || closing.close(generation)).await;
                    retry.wait().await;
                    continue;
                }
                let barrier = service.clone();
                let publication_settled =
                    tokio::task::spawn_blocking(move || barrier.generation_has_settled(generation))
                        .await
                        .unwrap_or(false);
                if !publication_settled {
                    retry.wait().await;
                    continue;
                }
                let settled = if dismiss_after_settle {
                    let dismissing = service.clone();
                    blocking_step(move || dismissing.abandon_generation(generation))
                        .await
                        .map_err(|failure| ("retrying uncommitted terminal abandonment", failure))
                } else {
                    let restoring = service.clone();
                    let prior = prior_launch_session.clone();
                    blocking_step(move || {
                        restoring.restore_settled_provider_session_binding(generation, prior)
                    })
                    .await
                    .map_err(|failure| ("retrying provider-session rollback", failure))
                };
                match settled {
                    Ok(()) => break,
                    Err((what, failure)) => retry.fail(generation.id, what, &failure).await,
                }
            }
            let _ = tokio::task::spawn_blocking(move || {
                lifecycle.finish_rollback(generation.id, !dismiss_after_settle);
            })
            .await;
        });
    }
}
