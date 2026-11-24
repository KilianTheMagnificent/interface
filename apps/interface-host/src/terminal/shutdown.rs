//! Dispatcher shutdown: fence, drain, reconcile open authority
//! transactions and stop every terminal process.

use super::authority::reopen_authority_slot_launch_session;
use super::{
    Arc, Duration, HostTerminalLifecycleSink, Instant, Ordering, ReopenAuthorityTransactionKind,
    TERMINAL_BLOCKING_CONCURRENCY_PERMITS, TerminalGenerationRequest, TerminalHostDispatcher,
    TerminalId, TerminalService, TerminalState,
};

const TERMINAL_SHUTDOWN_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const TERMINAL_SHUTDOWN_RETRY_DELAY: Duration = Duration::from_millis(50);

impl TerminalHostDispatcher {
    /// Stops input admission, terminates every process tree, and drains all
    /// output pumps before the binary pipes are closed by the process owner.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.transport_available.store(false, Ordering::Release);
        // Fence every create/restart that may already be paused in an async
        // workspace verifier. Those paths recheck `closed` immediately before
        // and after spawning; taking the same gate here then makes teardown a
        // barrier after their rollback, not a one-time snapshot they can
        // escape after shutdown.
        let _lifecycle_dispatch = self.lifecycle_dispatch_gate.lock().await;
        let input = self.input_pump.lock().ok().and_then(|mut slot| slot.take());
        if let Some(input) = input {
            input.abort();
            let _ = input.await;
        }
        // Dropping every lane sender ends each lane thread once it has drained.
        // They are deliberately not awaited: a lane parked in a PTY write is
        // released by the process teardown below, not by waiting on it here.
        self.close_input_lanes();
        let monitor = self
            .failure_monitor
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(monitor) = monitor {
            monitor.abort();
            let _ = monitor.await;
        }
        // No shell is handed to an agent that quitting ends.
        let handoff = self
            .shell_handoff
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(handoff) = handoff {
            handoff.abort();
            let _ = handoff.await;
        }

        // The binary-plane owner shuts down only after this dispatcher
        // returns. Close our shared outbound queue before process settlement
        // so a pump parked on full-queue backpressure cannot stop consuming
        // its terminal output and prevent Exited publication.
        if let Some(sender) = self.data_sender.as_ref() {
            sender.close();
        }

        // A cancelled request drops its async future but Tokio cannot cancel
        // an already-running spawn_blocking closure. Drain every dispatcher
        // permit before inspecting transactions so no orphaned create,
        // restart, or dismiss can mutate the service after our snapshot.
        let _blocking_drain = match Arc::clone(&self.blocking_gate)
            .acquire_many_owned(TERMINAL_BLOCKING_CONCURRENCY_PERMITS)
            .await
        {
            Ok(permits) => Some(permits),
            Err(error) => {
                tracing::error!(%error, "terminal blocking workers could not be drained");
                None
            }
        };
        let service = self.service.clone();
        let lifecycle = Arc::clone(&self.lifecycle_sink);
        match tokio::task::spawn_blocking(move || {
            reconcile_lifecycle_transactions_before_shutdown(&service, &lifecycle)
                && close_active_terminals(&service)
                && reconcile_lifecycle_transactions_after_shutdown(&service, &lifecycle)
                && service.release_settled_for_shutdown()
        })
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::error!("terminal resources remained active after shutdown teardown");
            }
            Err(error) => tracing::error!(%error, "terminal shutdown worker failed"),
        }
        let mut pumps = self.output_pumps.lock().map_or_else(
            |_| Vec::new(),
            |mut pumps| std::mem::take(&mut *pumps).into_values().collect(),
        );
        // Release every not-yet-visible reader before awaiting its blocking
        // task. Otherwise shutdown could wait forever for a create request's
        // local admission sender while the app restart watchdog is ticking.
        for pump in &mut pumps {
            pump.admission.take();
        }
        if !pumps.is_empty() {
            let _ = tokio::task::spawn_blocking(move || {
                for pump in pumps {
                    let _ = pump.task.join();
                }
            })
            .await;
        }
    }
}

pub(super) fn transaction_is_pending(
    sink: &HostTerminalLifecycleSink,
    terminal_id: TerminalId,
) -> bool {
    sink.shutdown_transactions().map_or(true, |transactions| {
        transactions.iter().any(|(id, _)| *id == terminal_id)
    })
}

pub(super) fn retry_shutdown_authority_rollback(
    sink: &HostTerminalLifecycleSink,
    terminal_id: TerminalId,
) -> bool {
    if let Err(error) = sink.prepare_shutdown_rollback(terminal_id) {
        tracing::error!(
            code = %error.code,
            id = %terminal_id,
            "could not prepare terminal authority rollback during shutdown"
        );
        return false;
    }
    loop {
        match sink.retry_authority_rollback(terminal_id) {
            Ok(()) => return true,
            Err(_) if !transaction_is_pending(sink, terminal_id) => return true,
            Err(error) => tracing::warn!(
                code = %error.code,
                id = %terminal_id,
                "retrying terminal authority rollback during shutdown"
            ),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn reconcile_lifecycle_transactions_before_shutdown(
    service: &TerminalService,
    sink: &HostTerminalLifecycleSink,
) -> bool {
    let transactions = match sink.shutdown_transactions() {
        Ok(transactions) => transactions,
        Err(error) => {
            tracing::error!(
                code = %error.code,
                "could not inspect terminal lifecycle transactions during shutdown"
            );
            return false;
        }
    };
    let live = service.snapshot().terminals;
    for (terminal_id, transaction) in transactions {
        match transaction.kind {
            ReopenAuthorityTransactionKind::Dismiss => {
                if live.iter().any(|terminal| terminal.id == terminal_id) {
                    if !retry_shutdown_authority_rollback(sink, terminal_id) {
                        return false;
                    }
                } else {
                    loop {
                        match sink.commit_shutdown_dismissal(terminal_id) {
                            Ok(()) => break,
                            Err(_) if !transaction_is_pending(sink, terminal_id) => break,
                            Err(error) => tracing::warn!(
                                code = %error.code,
                                id = %terminal_id,
                                "retrying terminal dismissal authority during shutdown"
                            ),
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                sink.finish_shutdown_transaction(terminal_id);
            }
            ReopenAuthorityTransactionKind::Create
            | ReopenAuthorityTransactionKind::Restart { .. } => {
                if !retry_shutdown_authority_rollback(sink, terminal_id) {
                    return false;
                }
            }
        }
    }
    true
}

pub(super) fn reconcile_lifecycle_transactions_after_shutdown(
    service: &TerminalService,
    sink: &HostTerminalLifecycleSink,
) -> bool {
    let transactions = match sink.shutdown_transactions() {
        Ok(transactions) => transactions,
        Err(error) => {
            tracing::error!(
                code = %error.code,
                "could not inspect settled terminal transactions during shutdown"
            );
            return false;
        }
    };
    for (terminal_id, transaction) in transactions {
        match transaction.kind {
            ReopenAuthorityTransactionKind::Create => loop {
                let terminal = service
                    .snapshot()
                    .terminals
                    .into_iter()
                    .find(|terminal| terminal.id == terminal_id);
                let Some(terminal) = terminal else {
                    break;
                };
                let generation = TerminalGenerationRequest {
                    id: terminal_id,
                    generation: terminal.generation,
                };
                match service.abandon_generation(generation) {
                    Ok(_) => break,
                    Err(error) => tracing::warn!(
                        %error,
                        id = %terminal_id,
                        "retrying abandoned create cleanup during shutdown"
                    ),
                }
                std::thread::sleep(Duration::from_millis(100));
            },
            ReopenAuthorityTransactionKind::Restart { prior_generation } => loop {
                let terminal = service
                    .snapshot()
                    .terminals
                    .into_iter()
                    .find(|terminal| terminal.id == terminal_id);
                let Some(terminal) = terminal else {
                    break;
                };
                if terminal.generation == prior_generation {
                    break;
                }
                let generation = TerminalGenerationRequest {
                    id: terminal_id,
                    generation: terminal.generation,
                };
                let prior_launch_session = reopen_authority_slot_launch_session(&transaction.prior);
                match service
                    .restore_settled_provider_session_binding(generation, prior_launch_session)
                {
                    Ok(_) => break,
                    Err(error) => tracing::warn!(
                        %error,
                        id = %terminal_id,
                        "retrying restarted provider binding rollback during shutdown"
                    ),
                }
                std::thread::sleep(Duration::from_millis(100));
            },
            ReopenAuthorityTransactionKind::Dismiss => {
                // Dismissals are resolved before process teardown, while the
                // service entry still proves whether removal committed.
            }
        }
        sink.finish_shutdown_transaction(terminal_id);
    }
    true
}

/// Threads that close panes at once during shutdown.
const SHUTDOWN_CLOSERS: usize = 8;

pub(super) fn close_active_terminals(service: &TerminalService) -> bool {
    let deadline = Instant::now() + TERMINAL_SHUTDOWN_SETTLE_TIMEOUT;
    loop {
        let active = service
            .snapshot()
            .terminals
            .into_iter()
            .filter(|terminal| {
                matches!(
                    terminal.state,
                    TerminalState::Starting | TerminalState::Running
                )
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return true;
        }
        // Each close may wait out a pane's HUP grace (up to 200 ms), and no
        // pane's teardown is ordered against another's, so quitting with many
        // panes closes them side by side instead of one after another.
        let per_closer = active.len().div_ceil(SHUTDOWN_CLOSERS);
        std::thread::scope(|scope| {
            for terminals in active.chunks(per_closer) {
                scope.spawn(move || {
                    for terminal in terminals {
                        if let Err(error) = service.close(TerminalGenerationRequest {
                            id: terminal.id,
                            generation: terminal.generation,
                        }) {
                            tracing::warn!(
                                %error,
                                id = %terminal.id,
                                generation = terminal.generation.0,
                                "retrying terminal process teardown"
                            );
                        }
                    }
                });
            }
        });
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(TERMINAL_SHUTDOWN_RETRY_DELAY);
    }
}
