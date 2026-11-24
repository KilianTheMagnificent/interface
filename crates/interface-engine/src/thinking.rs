//! Coalesces a provider session's repeated thinking signals where its events are emitted.
//!
//! Codex and ACP report reasoning as one notification per token, and each maps to the same
//! `ThinkingSignal`. The desktop drops a signal identical to the event before it, so the
//! repeats only cost a sink measurement, a runtime persist, a host push and an IPC hop each.
use std::time::{Duration, Instant};

use interface_contracts::engine::{EngineEvent, EngineEventKind};

use crate::EventSink;

/// A repeat still leaves this often, so a long reasoning span keeps the session's last
/// activity moving at the cadence the host pushes it (one timestamp-only round a second).
pub const THINKING_REPEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Drops a thinking signal identical to the event emitted immediately before it — exactly
/// the repeat the desktop drops — except once per [`THINKING_REPEAT_INTERVAL`]. Any other
/// event resets it, so a signal after anything else always leaves.
#[derive(Debug, Default)]
pub struct ThinkingRepeats {
    last: Option<(bool, Option<String>, Instant)>,
}

impl ThinkingRepeats {
    /// Emits `event` into `sink` unless it is a dropped repeat. Every emit of the session must
    /// come through here under one guard, so "immediately before" is the order the sink
    /// carries. Returns `false` once the sink refuses events, as `EventSink::emit` would.
    pub fn emit(&mut self, sink: &EventSink<EngineEvent>, event: EngineEvent) -> bool {
        if self.admits_at(&event, Instant::now) {
            sink.emit(event).is_ok()
        } else {
            !sink.is_closed()
        }
    }

    /// Whether `event` should be emitted. Records it as the latest event when it is.
    fn admits_at(&mut self, event: &EngineEvent, now: impl FnOnce() -> Instant) -> bool {
        let EngineEventKind::ThinkingSignal { active, agent_key } = &event.event else {
            self.last = None;
            return true;
        };
        let now = now();
        if let Some((last_active, last_agent, at)) = &self.last
            && last_active == active
            && last_agent == agent_key
            && now.saturating_duration_since(*at) < THINKING_REPEAT_INTERVAL
        {
            return false;
        }
        self.last = Some((*active, agent_key.clone(), now));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking(active: bool, agent_key: Option<&str>) -> EngineEvent {
        EngineEvent {
            at: None,
            event: EngineEventKind::ThinkingSignal {
                active,
                agent_key: agent_key.map(ToOwned::to_owned),
            },
        }
    }

    fn delta() -> EngineEvent {
        EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "visible".into(),
                agent_key: None,
            },
        }
    }

    #[test]
    fn only_a_repeat_of_the_event_just_emitted_is_dropped() {
        let start = Instant::now();
        let mut repeats = ThinkingRepeats::default();
        let admitted: Vec<bool> = [
            thinking(true, None),
            thinking(true, None),
            thinking(true, None),
            thinking(true, Some("child")),
            thinking(true, Some("child")),
            thinking(true, None),
            thinking(false, None),
            thinking(false, None),
            delta(),
            thinking(false, None),
            thinking(true, None),
            delta(),
            delta(),
            thinking(true, None),
        ]
        .iter()
        .map(|event| repeats.admits_at(event, || start))
        .collect();
        assert_eq!(
            admitted,
            [
                true, false, false, true, false, true, true, false, true, true, true, true, true,
                true
            ]
        );
    }

    #[tokio::test]
    async fn emit_sends_one_signal_per_span_and_reports_a_closed_sink() {
        let (sink, mut stream) = crate::bounded_event_stream(16);
        let mut repeats = ThinkingRepeats::default();
        for _ in 0..50 {
            assert!(repeats.emit(&sink, thinking(true, None)));
        }
        assert!(repeats.emit(&sink, thinking(false, None)));
        let mut received = Vec::new();
        while let Ok(Ok(Some(event))) =
            tokio::time::timeout(Duration::from_millis(50), stream.recv()).await
        {
            received.push(event);
        }
        assert_eq!(received, [thinking(true, None), thinking(false, None)]);
        drop(stream);
        assert!(
            !repeats.emit(&sink, thinking(false, None)),
            "a dropped repeat"
        );
        assert!(
            !repeats.emit(&sink, thinking(true, None)),
            "an emitted signal"
        );
    }

    #[test]
    fn a_long_span_repeats_once_per_interval() {
        let start = Instant::now();
        let mut repeats = ThinkingRepeats::default();
        let at = |offset: Duration| move || start + offset;
        let signal = thinking(true, None);
        assert!(repeats.admits_at(&signal, at(Duration::ZERO)));
        assert!(!repeats.admits_at(&signal, at(Duration::from_millis(999))));
        assert!(repeats.admits_at(&signal, at(THINKING_REPEAT_INTERVAL)));
        assert!(!repeats.admits_at(&signal, at(Duration::from_millis(1500))));
        assert!(repeats.admits_at(&signal, at(Duration::from_millis(2000))));
    }
}
