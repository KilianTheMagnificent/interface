use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::event_budget::BufferedEvent;
use tokio::sync::Notify;

/// Slots an idle session's queue keeps. The bound is the separate `capacity`
/// field, so this only decides how much allocation survives a drained backlog.
const DRAINED_QUEUE_CAPACITY: usize = 64;

/// Compatibility error shape for consumers of the bounded stream API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventStreamError {
    Overflow { capacity: usize },
    ByteBudget { bytes: usize },
}

impl fmt::Display for EventStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ByteBudget { bytes } => write!(
                formatter,
                "engine event stream exceeded its {bytes}-byte payload budget with control events"
            ),
            Self::Overflow { capacity } => {
                write!(
                    formatter,
                    "engine event stream exceeded its {capacity}-event capacity"
                )
            }
        }
    }
}

impl std::error::Error for EventStreamError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventStreamSendError<T> {
    Closed(T),
}

struct Queued<T> {
    value: T,
    bytes: usize,
    disposable: bool,
    gap: bool,
}
struct Policy<T> {
    bytes: usize,
    measure: fn(&T) -> usize,
    disposable: fn(&T) -> bool,
    omission: fn(&T) -> T,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Gap {
    None,
    Queued,
    Reported,
}

struct State<T> {
    queue: VecDeque<Queued<T>>,
    bytes: usize,
    gap: Gap,
    capacity: usize,
    senders: usize,
    receiver_alive: bool,
    closed: bool,
    terminal: Option<EventStreamError>,
    /// The control-capacity error that closed the stream. Unlike `terminal`,
    /// which the receiver takes, it stays for the producer to read.
    overflow: Option<EventStreamError>,
}

struct Shared<T> {
    policy: Option<Policy<T>>,
    state: Mutex<State<T>>,
    changed: Notify,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Cloneable producer for one bounded session event stream.
pub struct EventSink<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for EventSink<T> {
    fn clone(&self) -> Self {
        self.shared.lock().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> EventSink<T> {
    /// Enqueue one event without blocking a provider read loop.
    ///
    /// Legacy channels drop the oldest event at capacity. Policy v2 channels
    /// discard only transcript output with an explicit notice; exhausted control
    /// capacity closes the stream with a count or byte error for its receiver.
    ///
    /// # Errors
    ///
    /// Returns the original value only when the stream is closed.
    pub fn emit(&self, value: T) -> Result<(), EventStreamSendError<T>> {
        let policy = self.shared.policy.as_ref();
        let incoming = Queued {
            bytes: policy.map_or(0, |policy| (policy.measure)(&value)),
            disposable: policy.is_some_and(|policy| (policy.disposable)(&value)),
            value,
            gap: false,
        };
        let (result, discarded) = {
            let mut state = self.shared.lock();
            enqueue(&mut state, incoming, policy)
        };
        self.shared.changed.notify_one();
        // A payload destructor can contact the sink; never run it under lock.
        drop(discarded);
        result
    }

    pub fn close(&self) {
        let mut state = self.shared.lock();
        state.closed = true;
        drop(state);
        self.shared.changed.notify_waiters();
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        let state = self.shared.lock();
        state.closed || !state.receiver_alive
    }

    /// Why a policy v2 stream closed itself: exhausted control capacity.
    /// `None` while it is open, and when it closed any other way (the
    /// receiver dropped it, or a producer closed it).
    #[must_use]
    pub fn overflow(&self) -> Option<EventStreamError> {
        self.shared.lock().overflow.clone()
    }
}

impl<T> Drop for EventSink<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.senders = state.senders.saturating_sub(1);
        if state.senders == 0 {
            state.closed = true;
            drop(state);
            self.shared.changed.notify_waiters();
        }
    }
}

/// The single consumer half of a bounded engine event stream.
pub struct BoundedEventStream<T> {
    shared: Arc<Shared<T>>,
}

impl<T> BoundedEventStream<T> {
    /// Receive the next retained event or stream end.
    ///
    /// # Errors
    ///
    /// Returns a terminal stream error when a producer explicitly records one.
    pub async fn recv(&mut self) -> Result<Option<T>, EventStreamError> {
        loop {
            // Register before inspecting state so a notification cannot land in
            // the gap between the empty check and awaiting it.
            let changed = self.shared.changed.notified();
            {
                let mut state = self.shared.lock();
                if let Some(queued) = state.queue.pop_front() {
                    state.bytes = state.bytes.saturating_sub(queued.bytes);
                    if queued.gap {
                        state.gap = Gap::Reported;
                    }
                    if state.queue.is_empty() {
                        state.gap = Gap::None;
                    }
                    return Ok(Some(queued.value));
                }
                // A drained queue that grew during a backlog keeps its whole
                // doubled allocation for the session's life otherwise.
                if state.queue.capacity() > DRAINED_QUEUE_CAPACITY * 4 {
                    state.queue.shrink_to(DRAINED_QUEUE_CAPACITY);
                }
                if let Some(error) = state.terminal.take() {
                    return Err(error);
                }
                if state.closed || state.senders == 0 {
                    return Ok(None);
                }
            }
            changed.await;
        }
    }
}

impl<T> Drop for BoundedEventStream<T> {
    fn drop(&mut self) {
        let queued = {
            let mut state = self.shared.lock();
            state.receiver_alive = false;
            std::mem::take(&mut state.queue)
        };
        self.shared.changed.notify_waiters();
        // Surviving producers cannot deliver these values. Release their memory
        // now, outside the mutex: T's destructor may itself contact the sink.
        drop(queued);
    }
}

/// Create a one-consumer event channel with a strict, non-zero capacity.
///
/// # Panics
///
/// Panics when `capacity` is zero.
#[must_use]
pub fn bounded_event_stream<T>(capacity: usize) -> (EventSink<T>, BoundedEventStream<T>) {
    channel(capacity, None)
}

/// Policy v2: disposable output may be replaced by one in-order notice per
/// backlog. Control events are never evicted; exhausting their budget closes
/// the channel with an explicit overflow instead of losing authority silently.
/// One omission marker may occupy an additional count slot, within the byte cap.
///
/// # Panics
/// Panics when either bound is zero.
#[must_use]
pub fn bounded_event_stream_with_budget<T: BufferedEvent>(
    capacity: usize,
    bytes: usize,
) -> (EventSink<T>, BoundedEventStream<T>) {
    assert!(bytes > 0, "event byte budget must be positive");
    channel(
        capacity,
        Some(Policy {
            bytes,
            measure: T::payload_bytes,
            disposable: T::disposable,
            omission: T::omission,
        }),
    )
}

fn channel<T>(capacity: usize, policy: Option<Policy<T>>) -> (EventSink<T>, BoundedEventStream<T>) {
    assert!(capacity > 0, "event stream capacity must be positive");
    let shared = Arc::new(Shared {
        policy,
        state: Mutex::new(State {
            // Bound is `capacity`, not VecDeque's allocation. Idle sessions stay
            // small; doubling from 64 still beats a 5_000-slot (~0.8 MB) reserve.
            queue: VecDeque::with_capacity(DRAINED_QUEUE_CAPACITY.min(capacity)),
            capacity,
            bytes: 0,
            gap: Gap::None,
            senders: 1,
            receiver_alive: true,
            closed: false,
            terminal: None,
            overflow: None,
        }),
        changed: Notify::new(),
    });
    (
        EventSink {
            shared: Arc::clone(&shared),
        },
        BoundedEventStream { shared },
    )
}

// Returns discarded payloads to its caller for destruction outside the lock.
fn enqueue<T>(
    state: &mut State<T>,
    mut incoming: Queued<T>,
    policy: Option<&Policy<T>>,
) -> (Result<(), EventStreamSendError<T>>, Vec<T>) {
    let mut discarded = Vec::new();
    if state.closed || !state.receiver_alive {
        return (Err(EventStreamSendError::Closed(incoming.value)), discarded);
    }
    let Some(policy) = policy else {
        if state.queue.len() == state.capacity {
            discarded.push(state.queue.pop_front().expect("full queue").value);
        }
        state.queue.push_back(incoming);
        return (Ok(()), discarded);
    };
    loop {
        let count =
            state.queue.len() - usize::from(state.gap == Gap::Queued) + usize::from(!incoming.gap);
        let bytes = state.bytes.saturating_add(incoming.bytes);
        if count <= state.capacity && bytes <= policy.bytes {
            break;
        }
        if let Some(index) = state.queue.iter().position(|entry| entry.disposable) {
            let old = state.queue.remove(index).expect("found disposable event");
            state.bytes = state.bytes.saturating_sub(old.bytes);
            if state.gap == Gap::None && !incoming.gap {
                let value = (policy.omission)(&old.value);
                let bytes = (policy.measure)(&value);
                state.bytes = state.bytes.saturating_add(bytes);
                state.queue.insert(
                    index,
                    Queued {
                        value,
                        bytes,
                        disposable: false,
                        gap: true,
                    },
                );
                state.gap = Gap::Queued;
            }
            discarded.push(old.value);
        } else if incoming.disposable && state.bytes <= policy.bytes {
            if state.gap != Gap::None {
                discarded.push(incoming.value);
                return (Ok(()), discarded);
            }
            let value = (policy.omission)(&incoming.value);
            let bytes = (policy.measure)(&value);
            discarded.push(incoming.value);
            incoming = Queued {
                value,
                bytes,
                disposable: false,
                gap: true,
            };
        } else {
            let error = if bytes > policy.bytes {
                EventStreamError::ByteBudget {
                    bytes: policy.bytes,
                }
            } else {
                EventStreamError::Overflow {
                    capacity: state.capacity,
                }
            };
            state.terminal = Some(error.clone());
            state.overflow = Some(error);
            state.closed = true;
            discarded.extend(state.queue.drain(..).map(|entry| entry.value));
            state.bytes = 0;
            state.gap = Gap::None;
            return (Err(EventStreamSendError::Closed(incoming.value)), discarded);
        }
    }
    state.bytes = state.bytes.saturating_add(incoming.bytes);
    if incoming.gap {
        state.gap = Gap::Queued;
    }
    state.queue.push_back(incoming);
    (Ok(()), discarded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_receiver_releases_backlog_while_producers_survive() {
        let (sink, stream) = bounded_event_stream(5000);
        let payload = Arc::new(vec![0_u8; 1024]);
        for _ in 0..5000 {
            sink.emit(Arc::clone(&payload)).unwrap();
        }
        assert_eq!(Arc::strong_count(&payload), 5001);
        drop(stream);
        assert_eq!(Arc::strong_count(&payload), 1);
        assert!(sink.is_closed());
        assert!(matches!(
            sink.emit(Arc::clone(&payload)),
            Err(EventStreamSendError::Closed(_))
        ));
        // A dropped receiver is not an overflow.
        assert_eq!(sink.overflow(), None);
    }

    #[test]
    fn receiver_drops_payloads_outside_the_state_lock() {
        struct Probe(Arc<Shared<Probe>>);
        impl Drop for Probe {
            fn drop(&mut self) {
                assert!(self.0.state.try_lock().is_ok());
            }
        }
        let (sink, stream) = bounded_event_stream(1);
        assert!(sink.emit(Probe(Arc::clone(&sink.shared))).is_ok());
        drop(stream);
        assert!(sink.is_closed());
    }

    #[tokio::test]
    async fn preserves_order_and_closes_when_the_last_sender_drops() {
        let (sink, mut stream) = bounded_event_stream(2);
        sink.emit(1).unwrap();
        sink.emit(2).unwrap();
        drop(sink);
        assert_eq!(stream.recv().await.unwrap(), Some(1));
        assert_eq!(stream.recv().await.unwrap(), Some(2));
        assert_eq!(stream.recv().await.unwrap(), None);
    }

    #[tokio::test]
    async fn overflow_drops_the_oldest_and_keeps_the_live_tail() {
        let (sink, mut stream) = bounded_event_stream(1);
        sink.emit("first").unwrap();
        sink.emit("latest").unwrap();
        assert_eq!(stream.recv().await.unwrap(), Some("latest"));
        sink.close();
        assert_eq!(stream.recv().await.unwrap(), None);
    }

    #[tokio::test]
    async fn high_capacity_still_drops_oldest_without_pre_reserving_the_bound() {
        let (sink, mut stream) = bounded_event_stream(5_000);
        for index in 0..5_002 {
            sink.emit(index).unwrap();
        }
        assert_eq!(stream.recv().await.unwrap(), Some(2));
        sink.close();
    }
    fn text(bytes: usize) -> interface_contracts::engine::EngineEvent {
        use interface_contracts::engine::{EngineEvent, EngineEventKind};
        EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "x".repeat(bytes),
                agent_key: None,
            },
        }
    }
    fn approval(id: &str) -> interface_contracts::engine::EngineEvent {
        use interface_contracts::engine::{EngineEvent, EngineEventKind};
        EngineEvent {
            at: None,
            event: EngineEventKind::ApprovalRequested {
                approval_id: id.into(),
                tool: "fixture".into(),
                summary: "owner approval".into(),
                input: serde_json::Value::Null,
            },
        }
    }

    #[tokio::test]
    async fn byte_budget_bounds_a_large_burst_and_preserves_control_order() {
        use interface_contracts::engine::{EngineEvent, EngineEventKind};
        let (sink, mut stream) = bounded_event_stream_with_budget(5000, 128 * 1024);
        sink.emit(approval("before")).unwrap();
        for _ in 0..1024 {
            sink.emit(text(64 * 1024)).unwrap();
        }
        sink.emit(approval("after")).unwrap();
        {
            let state = sink.shared.lock();
            assert!(state.bytes <= 128 * 1024);
            assert!(state.queue.len() < 10);
        }
        sink.close();
        let mut controls = vec![];
        let mut notices = 0;
        let mut payload = 0;
        while let Some(EngineEvent { event, .. }) = stream.recv().await.unwrap() {
            match event {
                EngineEventKind::ApprovalRequested { approval_id, .. } => {
                    controls.push(approval_id);
                }
                EngineEventKind::Notice { .. } => notices += 1,
                EngineEventKind::MessageDelta { text, .. } => payload += text.len(),
                _ => panic!("unexpected event"),
            }
        }
        assert_eq!(controls, ["before", "after"]);
        assert_eq!(notices, 1);
        assert_eq!(payload, 64 * 1024);
        assert_eq!(sink.shared.lock().bytes, 0);
    }

    #[tokio::test]
    async fn count_pressure_and_oversized_text_cannot_evict_approvals() {
        let (sink, mut stream) = bounded_event_stream_with_budget(1, 8192);
        sink.emit(approval("kept")).unwrap();
        sink.emit(text(64 * 1024)).unwrap();
        assert_eq!(stream.recv().await.unwrap(), Some(approval("kept")));
        let marker = stream.recv().await.unwrap().unwrap();
        assert!(matches!(
            marker.event,
            interface_contracts::engine::EngineEventKind::Notice { .. }
        ));
        assert!(!sink.is_closed());
        sink.emit(text(12)).unwrap();
        assert_eq!(stream.recv().await.unwrap(), Some(text(12)));
    }

    #[tokio::test]
    async fn protected_overflow_is_explicit_and_releases_the_backlog() {
        let (sink, mut stream) = bounded_event_stream_with_budget(1, 8192);
        sink.emit(approval("one")).unwrap();
        assert!(sink.emit(approval("two")).is_err());
        assert_eq!(
            stream.recv().await,
            Err(EventStreamError::Overflow { capacity: 1 })
        );
        assert!(sink.is_closed());
        assert_eq!(stream.recv().await.unwrap(), None);
        // The producer can still tell why after the receiver took the error.
        assert_eq!(
            sink.overflow(),
            Some(EventStreamError::Overflow { capacity: 1 })
        );
        let (sink, mut stream) = bounded_event_stream_with_budget(5000, 1024);
        assert!(sink.emit(approval(&"x".repeat(2048))).is_err());
        assert_eq!(
            stream.recv().await,
            Err(EventStreamError::ByteBudget { bytes: 1024 })
        );
        assert_eq!(sink.shared.lock().bytes, 0);
    }

    #[tokio::test]
    async fn a_notice_that_cannot_fit_also_fails_closed_within_the_byte_bound() {
        let protected = approval(&"a".repeat(500));
        let cap = protected.payload_bytes() + text(1).payload_bytes();
        let (sink, mut stream) = bounded_event_stream_with_budget(3, cap);
        sink.emit(protected).unwrap();
        sink.emit(text(1)).unwrap();
        assert!(sink.emit(text(4096)).is_err());
        assert_eq!(
            stream.recv().await,
            Err(EventStreamError::ByteBudget { bytes: cap })
        );
        assert_eq!(sink.shared.lock().bytes, 0);
    }
}
