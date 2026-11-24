//! The desktop push coalescer: one debounced, bounded coordinator per push
//! channel, the two-rate whole-registry coalescer the host's owners share,
//! and the registry frame they all send.

use std::{sync::Arc, time::Duration};

use interface_contracts::{engine::SessionSummary, ipc::CHANNELS};
use interface_runtime::{RuntimeError, SessionRuntime};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use crate::{
    dispatcher::{
        DispatchError, RuntimeCommandHandler, SessionAccessMemory, bridge_dispatch_error,
    },
    protocol::PushFrame,
    registry_access,
    session_access::mark_permission_modes,
};

const REGISTRY_DEBOUNCE: Duration = Duration::from_millis(50);
/// A registry round whose only change is `last_activity` leaves at most this
/// often. Deliberate departure from the 0.1.169 bridge, which rebroadcast
/// every 50 ms window; the renderer shows age at minute granularity, and a
/// streaming session no longer ships the whole registry every second.
const REGISTRY_ACTIVITY_INTERVAL: Duration = Duration::from_secs(5);

enum RegistryPushCommand {
    Dirty,
    Flush(oneshot::Sender<bool>),
    Close(oneshot::Sender<()>),
}

/// What one coalescer round has to push.
///
/// The three arms are the reason the producer is not a plain
/// `Fn() -> Option<PushFrame>`: "nothing to send" and "could not build the
/// frame" have opposite consequences for the coordinator task.
pub(crate) enum CoalescedPush {
    /// Send this frame.
    Frame(PushFrame),
    /// Nothing to send this round; the coordinator keeps running.
    Idle,
    /// The frame could not be produced — already logged. The coordinator stops,
    /// so the next `schedule` reports the push surface as gone.
    Unavailable,
}

/// One process-local coalescer for a desktop push channel.
///
/// The frozen bridge broadcasts the latest whole registry at most once per
/// 50 ms window, then flushes the final state when a stream closes. A bounded
/// single-slot command queue preserves that behavior without allowing token
/// volume to allocate an unbounded number of notifications. The orchestration
/// pump reuses the same shape for its `orchestration_event` bursts, so the
/// protocol lives here once.
///
/// A registry coalescer started with [`Self::start_registry`] is two-rate: a
/// round whose registry content revision equals the one its last frame left
/// with changed nothing but `last_activity`, so it waits until
/// [`REGISTRY_ACTIVITY_INTERVAL`] after that frame and then always sends a
/// trailing frame. Any other change still leaves within the 50 ms window, and
/// flush and close always send at once. The revision is the registry's own
/// counter, so a frame another emitter pushed in between can never make a
/// real change look like a timestamp.
#[derive(Clone)]
pub(crate) struct RegistryPushCoordinator {
    commands: mpsc::Sender<RegistryPushCommand>,
}

impl RegistryPushCoordinator {
    /// `produce` builds the frame for one round. It is a closure rather than a
    /// runtime handle because the callers hold three different sources — a
    /// `RuntimeCommandHandler`, an `OrchestrationRuntimePort`, and a latest-event
    /// slot — and all of them must share this one debounce protocol.
    pub(crate) fn start<P>(produce: P, pushes: mpsc::Sender<PushFrame>) -> Self
    where
        P: Fn() -> CoalescedPush + Send + Sync + 'static,
    {
        Self::start_registry(produce, || None, pushes)
    }

    /// A whole-registry coalescer. `content_revision` reads the registry's
    /// [`interface_registry::SessionRegistry::content_revision`]; `None`
    /// (unknown source) keeps every round on the 50 ms cadence.
    pub(crate) fn start_registry<P, C>(
        produce: P,
        content_revision: C,
        pushes: mpsc::Sender<PushFrame>,
    ) -> Self
    where
        P: Fn() -> CoalescedPush + Send + Sync + 'static,
        C: Fn() -> Option<u64> + Send + Sync + 'static,
    {
        let (commands, receiver) = mpsc::channel(1);
        tokio::spawn(run_registry_push_coordinator(
            produce,
            content_revision,
            pushes,
            receiver,
        ));
        Self { commands }
    }

    pub(crate) fn schedule(&self) -> bool {
        match self.commands.try_send(RegistryPushCommand::Dirty) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    pub(crate) async fn flush(&self) -> bool {
        let (complete, completed) = oneshot::channel();
        if self
            .commands
            .send(RegistryPushCommand::Flush(complete))
            .await
            .is_err()
        {
            return false;
        }
        completed.await.unwrap_or(false)
    }

    /// Emits the last pending frame and stops the coordinator task, so it does
    /// not park on `recv` for the life of the process. Returns only after that
    /// final frame has been queued, and every later `schedule` reports `false`.
    pub(crate) async fn close(&self) {
        let (done, closed) = oneshot::channel();
        if self
            .commands
            .send(RegistryPushCommand::Close(done))
            .await
            .is_ok()
        {
            let _ = closed.await;
        }
    }
}

/// Told after each shared registry round, and after a session listing that
/// changed the registry (`RuntimeHostDispatcher::with_listing_observer`).
/// Called on the coalescer's or the request's task, so it must only signal,
/// never block.
pub trait RegistryRoundObserver: Send + Sync + 'static {
    fn registry_round(&self);
}

/// The one whole-registry push coalescer a host process shares between the
/// runtime, collaboration and orchestration owners, so the desktop gets at
/// most one registry frame per window whichever owner scheduled it (they all
/// read the same registry). `main` builds it before any owner starts and is
/// the only closer, after every owner has drained: a closed coordinator stops
/// registry pushes and event forwarding for all of them. An unreadable
/// registry likewise stops it for every owner.
#[derive(Clone)]
pub struct SharedRegistryPushes(pub(crate) RegistryPushCoordinator);

impl SharedRegistryPushes {
    #[must_use]
    pub fn start(runtime: Arc<SessionRuntime>, pushes: mpsc::Sender<PushFrame>) -> Self {
        let revision_runtime = Arc::clone(&runtime);
        Self(RegistryPushCoordinator::start_registry(
            move || registry_push_round(registry_access::sorted_visible(&runtime).map(Some)),
            move || registry_access::content_revision(&revision_runtime),
            pushes,
        ))
    }

    /// Like [`Self::start`], and tells `observer` after every round that
    /// read the registry, so state kept beside the registry (threads) sees
    /// each change the desktop is told about. Each row carries the permission
    /// mode `access` keeps for it (protocol 68).
    #[must_use]
    pub fn start_observed(
        runtime: Arc<SessionRuntime>,
        pushes: mpsc::Sender<PushFrame>,
        observer: Arc<dyn RegistryRoundObserver>,
        access: Option<Arc<dyn SessionAccessMemory>>,
    ) -> Self {
        let revision_runtime = Arc::clone(&runtime);
        let revision_access = access.clone();
        Self(RegistryPushCoordinator::start_registry(
            move || {
                let round = registry_push_round(registry_access::sorted_visible(&runtime).map(
                    |mut sessions| {
                        mark_permission_modes(&mut sessions, access.as_ref());
                        Some(sessions)
                    },
                ));
                observer.registry_round();
                round
            },
            // Protocol 68: a changed permission mode is a change of what the
            // frame carries, not a timestamp-only round.
            move || {
                registry_access::content_revision(&revision_runtime).map(|revision| {
                    revision.wrapping_add(
                        revision_access
                            .as_ref()
                            .map_or(0, |access| access.revision()),
                    )
                })
            },
            pushes,
        ))
    }

    /// Emits the last pending frame and stops the shared coalescer.
    pub async fn close(&self) {
        self.0.close().await;
    }
}

async fn run_registry_push_coordinator<P, C>(
    produce: P,
    content_revision: C,
    pushes: mpsc::Sender<PushFrame>,
    mut commands: mpsc::Receiver<RegistryPushCommand>,
) where
    P: Fn() -> CoalescedPush + Send + Sync + 'static,
    C: Fn() -> Option<u64> + Send + Sync + 'static,
{
    // When, and at which registry content revision, the last frame left.
    let mut last_emit: Option<(Instant, u64)> = None;
    while let Some(command) = commands.recv().await {
        let mut flush = None;
        let mut closing = None;
        match command {
            RegistryPushCommand::Flush(completed) => flush = Some(completed),
            RegistryPushCommand::Close(done) => {
                // Refuse further commands before the final emit, so a
                // `schedule` racing this close already reports the coordinator
                // as gone rather than queueing onto a task that is leaving.
                commands.close();
                closing = Some(done);
            }
            RegistryPushCommand::Dirty => {
                // `deadline` ends the debounce window or, once a round proved
                // timestamp-only, the activity window, whose trailing frame
                // always leaves so the desktop ends on the final lastActivity.
                let mut deadline = Instant::now() + REGISTRY_DEBOUNCE;
                let mut activity_wait = false;
                loop {
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => {
                            if activity_wait {
                                break;
                            }
                            match activity_window_end(last_emit, content_revision()) {
                                Some(end) if end > Instant::now() => {
                                    deadline = end;
                                    activity_wait = true;
                                }
                                _ => break,
                            }
                        }
                        next = commands.recv() => match next {
                            Some(RegistryPushCommand::Dirty) => {
                                if activity_wait {
                                    // This change may be more than a timestamp:
                                    // look again after one debounce window.
                                    deadline = Instant::now() + REGISTRY_DEBOUNCE;
                                    activity_wait = false;
                                }
                            }
                            Some(RegistryPushCommand::Flush(completed)) => {
                                flush = Some(completed);
                                break;
                            }
                            Some(RegistryPushCommand::Close(done)) => {
                                commands.close();
                                closing = Some(done);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        }
        // Read before producing: a change landing in between only makes the
        // next round send again, it can never be recorded as already sent.
        let content = content_revision();
        let emitted = match produce() {
            CoalescedPush::Frame(frame) => {
                let sent = pushes.send(frame).await.is_ok();
                if sent {
                    last_emit = content.map(|content| (Instant::now(), content));
                }
                sent
            }
            CoalescedPush::Idle => true,
            CoalescedPush::Unavailable => false,
        };
        if let Some(completed) = flush {
            let _ = completed.send(emitted);
        }
        if let Some(done) = closing {
            let _ = done.send(());
            break;
        }
        if !emitted {
            break;
        }
    }
}

/// When a round at `content` may send, if everything since the last frame is
/// a timestamp; `None` when it may send now.
fn activity_window_end(last_emit: Option<(Instant, u64)>, content: Option<u64>) -> Option<Instant> {
    let (at, previous) = last_emit?;
    (content? == previous).then(|| at + REGISTRY_ACTIVITY_INTERVAL)
}

async fn emit_push<P>(produce: &P, pushes: &mpsc::Sender<PushFrame>) -> bool
where
    P: Fn() -> CoalescedPush,
{
    match produce() {
        CoalescedPush::Frame(frame) => pushes.send(frame).await.is_ok(),
        CoalescedPush::Idle => true,
        CoalescedPush::Unavailable => false,
    }
}

pub(crate) async fn emit_registry_from_runtime<R>(
    runtime: &R,
    pushes: &mpsc::Sender<PushFrame>,
) -> bool
where
    R: RuntimeCommandHandler,
{
    emit_push(
        &|| registry_push_round(runtime.session_summaries().map(Some)),
        pushes,
    )
    .await
}

/// One coalescer round over a registry read. `Ok(None)` is a port with no
/// registry to project, which is idle rather than a failure.
pub(crate) fn registry_push_round(
    summaries: Result<Option<Vec<SessionSummary>>, RuntimeError>,
) -> CoalescedPush {
    let sessions = match summaries {
        Ok(Some(sessions)) => sessions,
        Ok(None) => return CoalescedPush::Idle,
        Err(error) => {
            tracing::error!(%error, "could not read the registry for a desktop push");
            return CoalescedPush::Unavailable;
        }
    };
    match registry_push_frame(&sessions) {
        Ok(frame) => CoalescedPush::Frame(frame),
        Err(error) => {
            tracing::error!(%error, "could not construct the registry push");
            CoalescedPush::Unavailable
        }
    }
}

/// Builds the frozen `iface:registry` push from one registry read.
///
/// Callers keep their own snapshot source and send-failure mapping.
///
/// # Errors
///
/// Returns a dispatch error when the snapshot cannot be encoded or the
/// registry channel is not a registered push.
pub(crate) fn registry_push_frame(sessions: &[SessionSummary]) -> Result<PushFrame, DispatchError> {
    // The largest push the desktop receives: encoded once, straight from the
    // rows, instead of through a value tree the writer walks a second time.
    PushFrame::encoded(CHANNELS.registry, sessions).map_err(bridge_dispatch_error)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::*;

    #[test]
    fn registry_push_frame_uses_the_frozen_registry_channel() {
        let frame = registry_push_frame(&[]).expect("empty registry");
        assert_eq!(frame.operation, CHANNELS.registry);
        assert_eq!(frame.payload, json!([]));
    }

    #[test]
    fn an_encoded_registry_frame_carries_exactly_the_rows_on_the_wire() {
        use interface_contracts::engine::{EngineId, SessionKind, SessionOrigin, SessionStatus};

        let full = SessionSummary {
            engine: EngineId::new("codex").expect("engine"),
            session_id: "one".into(),
            title: "Fix \"quotes\" and \u{1f600}".into(),
            cwd: "/repo/src".into(),
            status: SessionStatus::Working,
            cloud: false,
            model: Some("gpt-5.6".into()),
            effort: Some(None),
            last_activity: "2026-10-05T10:00:00.000Z".into(),
            first_seen: "2026-10-05T09:00:00.000Z".into(),
            archived: false,
            size: Some(u64::MAX),
            title_from_summary: Some(true),
            origin: Some(SessionOrigin::App),
            project_root: Some("/repo".into()),
            worktree_home: Some("/repo/src".into()),
            worktree_path: Some("/repo/.worktrees/one".into()),
            setting_sources: Some(vec!["user".into(), "project".into()]),
            disallowed_tools: Some(Vec::new()),
            kind: Some(SessionKind::Code),
            permission_mode: None,
        };
        let mut sparse = full.clone();
        sparse.session_id = "two".into();
        sparse.model = None;
        sparse.effort = None;
        sparse.size = None;
        sparse.setting_sources = None;
        let sessions = vec![full, sparse];
        let frame = registry_push_frame(&sessions).expect("registry frame");
        let line =
            serde_json::to_string(&crate::protocol::HostFrame::Push(frame)).expect("host frame");
        let wire: serde_json::Value = serde_json::from_str(&line).expect("frame JSON");
        assert_eq!(wire["kind"], "push");
        assert_eq!(wire["operation"], CHANNELS.registry);
        assert_eq!(
            wire["payload"],
            serde_json::to_value(&sessions).expect("rows")
        );
        assert!(wire["payload"][0]["effort"].is_null());
        assert!(wire["payload"][1].get("effort").is_none());
    }

    // Paused time: the cadence is asserted on the virtual clock, so machine load
    // cannot stretch the windows.
    #[tokio::test(start_paused = true)]
    async fn registry_coordinator_sends_timestamp_only_rounds_at_most_once_per_activity_window() {
        use std::sync::atomic::AtomicU64;
        use tokio::time::{Instant, sleep, timeout};

        async fn next_frame(frames: &mut mpsc::Receiver<PushFrame>, within: Duration) -> Duration {
            let started = Instant::now();
            timeout(within, frames.recv())
                .await
                .expect("registry frame")
                .expect("push channel");
            started.elapsed()
        }

        let content = Arc::new(AtomicU64::new(1));
        let (pushes, mut frames) = mpsc::channel(64);
        let coordinator = {
            let content = Arc::clone(&content);
            RegistryPushCoordinator::start_registry(
                || registry_push_round(Ok(Some(Vec::new()))),
                move || Some(content.load(Ordering::SeqCst)),
                pushes,
            )
        };

        // The first round has no previous frame to compare with.
        assert!(coordinator.schedule());
        next_frame(&mut frames, Duration::from_secs(2)).await;

        // Rounds that changed only lastActivity (same content revision) do not
        // leave every 50 ms: across two and a half activity windows of
        // streaming at most three frames leave, each a window apart.
        let window = REGISTRY_ACTIVITY_INTERVAL;
        let streaming = Instant::now();
        let mut sent = Vec::new();
        while streaming.elapsed() < window * 5 / 2 {
            assert!(coordinator.schedule());
            sleep(Duration::from_millis(20)).await;
            while frames.try_recv().is_ok() {
                sent.push(streaming.elapsed());
            }
        }
        assert!(
            (2..=3).contains(&sent.len()),
            "timestamp-only frames: {sent:?}"
        );
        assert!(
            sent.windows(2)
                .all(|pair| pair[1] - pair[0] >= window * 9 / 10),
            "timestamp-only frames too close: {sent:?}"
        );
        // With no further event, a trailing frame still carries the final
        // lastActivity within the activity window.
        next_frame(&mut frames, window + Duration::from_secs(2)).await;

        // A real change right after a frame leaves on the debounce, not the
        // activity window.
        content.fetch_add(1, Ordering::SeqCst);
        assert!(coordinator.schedule());
        let latency = next_frame(&mut frames, Duration::from_secs(2)).await;
        assert!(
            latency < Duration::from_millis(600),
            "content change took {latency:?}"
        );

        // Flush and close always send at once, even for a timestamp-only round.
        assert!(coordinator.flush().await);
        assert!(frames.try_recv().is_ok(), "flush sent a frame");
        coordinator.close().await;
        assert!(frames.try_recv().is_ok(), "close sent a final frame");
        assert!(!coordinator.schedule());
    }

    // An owner that pushes after a mutation flushes through the shared
    // coalescer: the round another owner schedules for the same change then
    // waits for the activity window instead of resending the registry.
    #[tokio::test(start_paused = true)]
    async fn a_flushed_frame_holds_back_a_round_scheduled_right_after_it() {
        use tokio::time::{sleep, timeout};

        let (pushes, mut frames) = mpsc::channel(8);
        let coordinator = RegistryPushCoordinator::start_registry(
            || registry_push_round(Ok(Some(Vec::new()))),
            || Some(7),
            pushes,
        );
        assert!(coordinator.flush().await);
        assert!(frames.try_recv().is_ok(), "the flush sent its frame");
        assert!(coordinator.schedule());
        sleep(REGISTRY_ACTIVITY_INTERVAL - Duration::from_millis(100)).await;
        assert!(frames.try_recv().is_err(), "the same registry was resent");
        timeout(Duration::from_millis(500), frames.recv())
            .await
            .expect("the trailing frame")
            .expect("push channel");
        coordinator.close().await;
    }
}
