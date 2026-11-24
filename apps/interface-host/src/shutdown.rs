//! Shutdown visibility without cancelling durable settlement or provider work.
use std::{future::Future, time::Duration};

/// Fence pane admission and finish its teardown before slower service drains.
/// Neither future is cancelled: durable provider settlement still runs normally.
pub(crate) async fn terminals_first<T>(
    terminals: impl Future<Output = ()>,
    services: impl Future<Output = T>,
) -> T {
    timed("terminals", terminals).await;
    services.await
}

pub(crate) async fn timed<T>(stage: &'static str, operation: impl Future<Output = T>) -> T {
    timed_with_notice(stage, operation, Duration::from_secs(5)).await
}

async fn timed_with_notice<T>(
    stage: &'static str,
    operation: impl Future<Output = T>,
    notice_after: Duration,
) -> T {
    let started = std::time::Instant::now();
    tokio::pin!(operation);
    let result = tokio::select! {
        result = &mut operation => result,
        () = tokio::time::sleep(notice_after) => {
            tracing::warn!(stage, "shutdown is waiting for safe settlement");
            operation.await
        }
    };
    tracing::info!(
        stage,
        elapsed_ms = started.elapsed().as_millis(),
        "shutdown stage completed"
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_cleanup_finishes_before_a_slow_service_drain_starts() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let closed = Arc::new(AtomicBool::new(false));
        let terminal_closed = Arc::clone(&closed);
        let service_closed = Arc::clone(&closed);
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(terminals_first(
            async move {
                terminal_closed.store(true, Ordering::Release);
            },
            async move {
                assert!(service_closed.load(Ordering::Acquire));
                started.send(()).unwrap();
                blocked.await.unwrap();
                42
            },
        ));
        observed.await.unwrap();
        assert!(closed.load(Ordering::Acquire));
        assert!(
            !task.is_finished(),
            "slow service is still draining after panes are closed"
        );
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap(), 42);
    }

    #[tokio::test]
    async fn slow_stage_is_observed_without_cancelling_or_replaying_it() {
        let calls = std::cell::Cell::new(0);
        let result = timed_with_notice(
            "fixture",
            async {
                calls.set(calls.get() + 1);
                tokio::time::sleep(Duration::from_millis(5)).await;
                Err::<(), _>("original failure")
            },
            Duration::ZERO,
        )
        .await;
        assert_eq!(calls.get(), 1);
        assert_eq!(result, Err("original failure"));
    }
}
