//! Event buffer policy v2: bounded payloads, visible transcript omissions, and
//! protected control events. The legacy count-only channel remains available.
use interface_contracts::engine::{EngineEvent, EngineEventKind, NoticeTone};
use serde::Serialize;
use std::io::{self, Write};

pub const EVENT_BUFFER_POLICY_VERSION: u8 = 2;
pub const PROVIDER_EVENT_BYTES: usize = 8 * 1024 * 1024;
pub const SUBSCRIBER_EVENT_BYTES: usize = 4 * 1024 * 1024;
pub const OMITTED_TRANSCRIPT: &str = "Some live transcript output was omitted because its consumer fell behind. Inspect the retained provider history for the complete output.";

/// Payload accounting, disposable-content classification, and an in-order gap
/// marker. Implementations must never classify authority or lifecycle as text.
pub trait BufferedEvent {
    fn payload_bytes(&self) -> usize;
    fn disposable(&self) -> bool;
    #[must_use]
    fn omission(&self) -> Self;
}

/// Counts serialized payload without allocating another copy of large JSON.
/// This is a payload budget, not a measurement of allocator overhead or RSS.
pub fn serialized_bytes(value: &impl Serialize) -> usize {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    if serde_json::to_writer(&mut counter, value).is_err() {
        return usize::MAX;
    }
    counter.0
}

impl BufferedEvent for EngineEvent {
    fn payload_bytes(&self) -> usize {
        // Deltas dominate streaming traffic. Count their owned strings directly
        // instead of serializing each token a second time.
        if let EngineEventKind::MessageDelta { text, agent_key } = &self.event {
            return size_of::<Self>()
                .saturating_add(text.len())
                .saturating_add(agent_key.as_ref().map_or(0, String::len))
                .saturating_add(self.at.as_ref().map_or(0, String::len));
        }
        size_of::<Self>().saturating_add(serialized_bytes(self))
    }
    fn disposable(&self) -> bool {
        matches!(
            self.event,
            EngineEventKind::MessageDelta { .. }
                | EngineEventKind::ToolCall { .. }
                | EngineEventKind::ToolResult { .. }
                | EngineEventKind::DiffProduced { .. }
        )
    }
    fn omission(&self) -> Self {
        Self {
            at: self.at.clone(),
            event: EngineEventKind::Notice {
                message: OMITTED_TRANSCRIPT.into(),
                tone: NoticeTone::Warning,
            },
        }
    }
}
