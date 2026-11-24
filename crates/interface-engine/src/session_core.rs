//! The session half engine-codex and engine-acp share over a
//! [`SupervisedTransport`]: readiness, the one emit path, and how a session
//! reports that its transport died. Provider nouns come from the transport's
//! [`Classify`] impl, so the prose each crate emitted before stays the same.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use interface_common::sync::lock;
use interface_contracts::engine::{EngineEvent, EngineEventKind};
use serde_json::Value;
use tokio::sync::watch;

use crate::{
    DeliveryOutcome, EngineError, EngineErrorKind, EventSink, ReadySession, SessionLifecycle,
    ThinkingRepeats,
    supervised::{Classify, SupervisedTransport},
};

#[derive(Clone, Debug)]
pub enum ReadyState {
    Pending,
    Ready(String),
    Failed(EngineError),
}

/// `emit` takes `lifecycle`, then `thinking` (which feeds `events`), then on
/// a dead sink `transport`. No guard here is ever held across an `.await`.
pub struct SessionCore<E> {
    pub lifecycle: Mutex<SessionLifecycle>,
    pub events: EventSink<EngineEvent>,
    /// Every emit goes through this guard; it drops repeated thinking signals.
    thinking: Mutex<ThinkingRepeats>,
    pub ready: watch::Sender<ReadyState>,
    pub closed: AtomicBool,
    transport: Mutex<Option<Arc<SupervisedTransport<E>>>>,
}

impl<E: Classify> SessionCore<E> {
    /// An opening session that emits into `events`, and the readiness
    /// receiver its handle keeps (which keeps every readiness send stored).
    #[must_use]
    pub fn new(events: EventSink<EngineEvent>) -> (Self, watch::Receiver<ReadyState>) {
        let (ready, ready_rx) = watch::channel(ReadyState::Pending);
        let core = Self {
            lifecycle: Mutex::default(),
            events,
            thinking: Mutex::default(),
            ready,
            closed: AtomicBool::new(false),
            transport: Mutex::new(None),
        };
        (core, ready_rx)
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Apply `event` to the lifecycle and emit it. A sink that refuses events
    /// closes the session and shuts its transport down; this returns `false`.
    pub fn emit(&self, event: EngineEvent) -> bool {
        lock(&self.lifecycle).apply_event(&event);
        if !lock(&self.thinking).emit(&self.events, event) {
            self.closed.store(true, Ordering::Release);
            if let Some(transport) = lock(&self.transport).as_ref() {
                transport.shutdown();
            }
            return false;
        }
        true
    }

    /// Fail a session that is still open: readiness fails with `error`, the
    /// error is emitted, and the event stream ends.
    pub fn fail_open(&self, error: EngineError) {
        if self.is_closed() {
            return;
        }
        lock(&self.lifecycle).mark_failed();
        let _ = self.ready.send(ReadyState::Failed(error.clone()));
        let _ = self.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Error {
                cause: None,
                message: error.message,
                retryable: error.retryable,
            },
        });
        self.events.close();
    }

    /// Report the transport's death once, whether its closed event or the end
    /// of inbound routing arrives first: what was in flight has an unknown
    /// outcome, readiness still pending fails with it, the error is emitted
    /// as retryable, and the event stream ends.
    pub fn fail_transport(&self, message: String) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        lock(&self.lifecycle).mark_failed();
        let error = EngineError::outcome_unknown(EngineErrorKind::Transport, message, true);
        self.fail_pending_readiness(error.clone());
        let _ = self.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Error {
                cause: None,
                message: error.message,
                retryable: true,
            },
        });
        self.events.close();
    }

    /// Fail readiness with `error` unless it already settled.
    pub fn fail_pending_readiness(&self, error: EngineError) {
        if matches!(&*self.ready.borrow(), ReadyState::Pending) {
            let _ = self.ready.send(ReadyState::Failed(error));
        }
    }

    /// Wait on the handle's readiness receiver until readiness settles,
    /// returning the provider's session id.
    ///
    /// # Errors
    ///
    /// Returns the error readiness failed with.
    pub async fn wait_ready(
        &self,
        mut ready: watch::Receiver<ReadyState>,
    ) -> Result<ReadySession, EngineError> {
        loop {
            let state = ready.borrow().clone();
            match state {
                ReadyState::Pending => {}
                ReadyState::Ready(session_id) => return Ok(ReadySession { session_id }),
                ReadyState::Failed(error) => return Err(error),
            }
            ready.changed().await.map_err(|_| {
                EngineError::outcome_unknown(
                    EngineErrorKind::Lifecycle,
                    format!("{} readiness ended without a result", E::LABEL),
                    true,
                )
            })?;
        }
    }

    pub fn attach_transport(&self, transport: &Arc<SupervisedTransport<E>>) {
        *lock(&self.transport) = Some(Arc::clone(transport));
    }

    /// The session's transport once opening attached it.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent refusal before then.
    pub fn transport(&self) -> Result<Arc<SupervisedTransport<E>>, EngineError> {
        lock(&self.transport).clone().ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                format!("{} provider is not ready; no request was sent", E::LABEL),
                true,
            )
        })
    }

    /// The refusal for an effect asked of a closed session.
    #[must_use]
    pub fn closed_outcome(&self, operation: &str) -> DeliveryOutcome<()> {
        DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            format!("{} session is closed; cannot {operation}", E::LABEL),
            false,
        ))
    }
}

/// Refuse a launch or resume cwd that is empty, unbounded or relative.
///
/// # Errors
///
/// Returns a definitely-unsent refusal naming `label`'s cwd.
pub fn validate_cwd(label: &str, cwd: &str) -> Result<(), EngineError> {
    if cwd.is_empty() || cwd.len() > 16_384 || !Path::new(cwd).is_absolute() {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("{label} cwd must be a bounded absolute path"),
            false,
        ));
    }
    Ok(())
}

/// The non-empty string at `path` inside `value`.
#[must_use]
pub fn value_text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::EngineEventKind;

    use super::*;
    use crate::{
        EffectCertainty, bounded_event_stream,
        jsonrpc::{FrameError, RouteEvent},
    };

    struct Fixture;

    impl Classify for Fixture {
        const LABEL: &'static str = "Fixture";
        const CHILD: &'static str = "fixture child";

        fn classify(_: Result<RouteEvent, FrameError>) -> Self {
            Self
        }

        fn closed(_: String) -> Self {
            Self
        }
    }

    #[tokio::test]
    async fn transport_death_is_reported_once_and_fails_pending_readiness() {
        let (sink, mut stream) = bounded_event_stream(8);
        let (core, ready) = SessionCore::<Fixture>::new(sink);
        // The closed event and the end of inbound routing both report it.
        core.fail_transport("fixture child exited with exit status: 1".into());
        core.fail_transport("Fixture inbound routing ended unexpectedly".into());
        let error = core.wait_ready(ready).await.unwrap_err();
        assert_eq!(error.message, "fixture child exited with exit status: 1");
        assert_eq!(error.kind, EngineErrorKind::Transport);
        assert_eq!(error.certainty, EffectCertainty::OutcomeUnknown);
        let event = stream.recv().await.unwrap().unwrap();
        assert_eq!(
            event.event,
            EngineEventKind::Error {
                cause: None,
                message: "fixture child exited with exit status: 1".into(),
                retryable: true,
            }
        );
        assert!(stream.recv().await.unwrap().is_none());
        assert!(core.is_closed());
        assert!(lock(&core.lifecycle).require_sendable().is_err());
    }

    #[tokio::test]
    async fn settled_readiness_survives_a_later_failure() {
        let (sink, _stream) = bounded_event_stream(8);
        let (core, ready) = SessionCore::<Fixture>::new(sink);
        let _ = core.ready.send(ReadyState::Ready("s-1".into()));
        core.fail_transport("fixture child closed stdout".into());
        core.fail_pending_readiness(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "closed",
            false,
        ));
        assert_eq!(core.wait_ready(ready).await.unwrap().session_id, "s-1");
    }

    #[tokio::test]
    async fn fail_open_reports_its_error_unless_already_closed() {
        let (sink, mut stream) = bounded_event_stream(8);
        let (core, ready) = SessionCore::<Fixture>::new(sink);
        core.fail_open(EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            "could not start fixture child",
            true,
        ));
        assert_eq!(
            core.wait_ready(ready).await.unwrap_err().message,
            "could not start fixture child"
        );
        assert_eq!(
            stream.recv().await.unwrap().unwrap().event,
            EngineEventKind::Error {
                cause: None,
                message: "could not start fixture child".into(),
                retryable: true,
            }
        );
        assert!(stream.recv().await.unwrap().is_none());

        let (sink, mut stream) = bounded_event_stream(8);
        let (core, _ready) = SessionCore::<Fixture>::new(sink);
        core.closed.store(true, Ordering::Release);
        core.fail_open(EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            "late",
            true,
        ));
        core.events.close();
        assert!(stream.recv().await.unwrap().is_none());
    }

    #[test]
    fn refusals_name_the_provider() {
        let (sink, _stream) = bounded_event_stream(8);
        let (core, _ready) = SessionCore::<Fixture>::new(sink);
        let DeliveryOutcome::DefinitelyUnsent(closed) = core.closed_outcome("send") else {
            panic!("a closed session refuses before any effect");
        };
        assert_eq!(closed.message, "Fixture session is closed; cannot send");
        assert!(!closed.retryable);
        let Err(unready) = core.transport() else {
            panic!("no transport is attached yet");
        };
        assert_eq!(
            unready.message,
            "Fixture provider is not ready; no request was sent"
        );
        assert!(unready.replay_is_safe());
        assert_eq!(
            validate_cwd("Fixture", "relative").unwrap_err().message,
            "Fixture cwd must be a bounded absolute path"
        );
        assert!(validate_cwd("Fixture", "/work").is_ok());
        core.events.close();
        assert!(!core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::SessionClosed,
        }));
        assert!(core.is_closed(), "a refused emit closes the session");
        let value = serde_json::json!({"a": {"b": "text", "empty": ""}});
        assert_eq!(value_text(&value, &["a", "b"]), Some("text"));
        assert_eq!(value_text(&value, &["a", "empty"]), None);
    }
}
