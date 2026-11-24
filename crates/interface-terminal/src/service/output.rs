//! The bounded, single-consumer output queue of one terminal generation, and
//! its public reader.
use super::{
    Arc, Condvar, Duration, Mutex, TerminalError, TerminalGeneration, TerminalId, VecDeque, lock,
};

/// One exact, ordered terminal-output fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalOutputChunk {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub sequence: u64,
    pub bytes: Vec<u8>,
}

/// Result of a bounded wait on a terminal's dedicated output reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalOutputRead {
    Chunk(TerminalOutputChunk),
    TimedOut,
    Closed,
}

struct OutputState {
    queue: VecDeque<TerminalOutputChunk>,
    queued_bytes: usize,
    next_sequence: u64,
    closed: bool,
    consumer: OutputConsumerState,
    writer_waiting: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputConsumerState {
    NeverClaimed,
    Claimed,
    Abandoned,
}

pub(super) struct OutputBuffer {
    id: TerminalId,
    generation: TerminalGeneration,
    capacity_bytes: usize,
    state: Mutex<OutputState>,
    readable: Condvar,
    writable: Condvar,
}

impl OutputBuffer {
    pub(super) fn new(
        id: TerminalId,
        generation: TerminalGeneration,
        capacity_bytes: usize,
    ) -> Self {
        Self {
            id,
            generation,
            capacity_bytes,
            state: Mutex::new(OutputState {
                queue: VecDeque::new(),
                queued_bytes: 0,
                next_sequence: 0,
                closed: false,
                consumer: OutputConsumerState::NeverClaimed,
                writer_waiting: false,
            }),
            readable: Condvar::new(),
            writable: Condvar::new(),
        }
    }

    pub(super) fn push(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return true;
        }
        if bytes.len() > self.capacity_bytes {
            for chunk in bytes.chunks(self.capacity_bytes) {
                if !self.push_one(chunk.to_vec()) {
                    return false;
                }
            }
            return true;
        }
        self.push_one(bytes)
    }

    fn push_one(&self, bytes: Vec<u8>) -> bool {
        let mut state = lock(&self.state);
        let backpressured = state.queued_bytes.saturating_add(bytes.len()) > self.capacity_bytes;
        if backpressured {
            state.writer_waiting = true;
        }
        while !state.closed && state.queued_bytes.saturating_add(bytes.len()) > self.capacity_bytes
        {
            state = self
                .writable
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if backpressured {
            state.writer_waiting = false;
        }
        if state.closed {
            return false;
        }
        let chunk = TerminalOutputChunk {
            id: self.id,
            generation: self.generation,
            sequence: state.next_sequence,
            bytes,
        };
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.queued_bytes = state.queued_bytes.saturating_add(chunk.bytes.len());
        state.queue.push_back(chunk);
        self.readable.notify_one();
        true
    }

    pub(super) fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        self.readable.notify_all();
        self.writable.notify_all();
    }

    /// Close only when no consumer owns the queue and the PTY reader cannot
    /// make progress. The decision and mutation share one lock so a consumer
    /// cannot claim the tail between the predicate and `closed = true`.
    pub(super) fn close_if_unconsumed_backpressure(&self, include_never_claimed: bool) -> bool {
        let mut state = lock(&self.state);
        let reader_was_abandoned = state.consumer == OutputConsumerState::Abandoned;
        let never_claimed_and_backpressured = include_never_claimed
            && state.consumer == OutputConsumerState::NeverClaimed
            && state.writer_waiting;
        if !reader_was_abandoned && !never_claimed_and_backpressured {
            return false;
        }
        state.closed = true;
        self.readable.notify_all();
        self.writable.notify_all();
        true
    }

    pub(super) fn claim(self: &Arc<Self>) -> Result<TerminalOutputReader, TerminalError> {
        let mut state = lock(&self.state);
        if state.consumer == OutputConsumerState::Claimed {
            return Err(TerminalError::OutputAlreadyClaimed(self.id));
        }
        state.consumer = OutputConsumerState::Claimed;
        drop(state);
        Ok(TerminalOutputReader {
            buffer: Arc::clone(self),
        })
    }

    fn release_reader(&self) {
        let mut state = lock(&self.state);
        if state.consumer == OutputConsumerState::Claimed {
            state.consumer = OutputConsumerState::Abandoned;
        }
    }
}

/// The single-consumer, bounded reader for one terminal generation. Dropping
/// it permits a replacement consumer to drain any remaining buffered output.
pub struct TerminalOutputReader {
    buffer: Arc<OutputBuffer>,
}

impl TerminalOutputReader {
    #[must_use]
    pub fn id(&self) -> TerminalId {
        self.buffer.id
    }

    #[must_use]
    pub fn generation(&self) -> TerminalGeneration {
        self.buffer.generation
    }

    /// Wait for one chunk, closure, or timeout. Chunks are never dropped or
    /// merged, and their sequence starts at zero for each generation.
    #[must_use]
    pub fn read_timeout(&self, timeout: Duration) -> TerminalOutputRead {
        let mut state = lock(&self.buffer.state);
        if state.queue.is_empty() && !state.closed {
            let waited = self
                .buffer
                .readable
                .wait_timeout(state, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waited.0;
        }
        if let Some(chunk) = state.queue.pop_front() {
            state.queued_bytes = state.queued_bytes.saturating_sub(chunk.bytes.len());
            self.buffer.writable.notify_one();
            return TerminalOutputRead::Chunk(chunk);
        }
        if state.closed {
            TerminalOutputRead::Closed
        } else {
            TerminalOutputRead::TimedOut
        }
    }

    /// Wait without a timeout until output is queued or the generation ends,
    /// then take every queued byte up to `max_bytes` in order: always the
    /// first chunk, and each further chunk that still fits. Returns `None`
    /// only once the buffer is closed and drained. Merging happens only when
    /// a backlog already exists, so a lone chunk is returned as soon as it is
    /// read; `read_timeout` keeps its one-chunk contract.
    #[must_use]
    pub fn read_batch(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let mut state = lock(&self.buffer.state);
        while state.queue.is_empty() && !state.closed {
            state = self
                .buffer
                .readable
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let mut bytes = state.queue.pop_front()?.bytes;
        while let Some(next) = state.queue.pop_front() {
            if bytes.len().saturating_add(next.bytes.len()) > max_bytes {
                state.queue.push_front(next);
                break;
            }
            bytes.extend_from_slice(&next.bytes);
        }
        state.queued_bytes = state.queued_bytes.saturating_sub(bytes.len());
        self.buffer.writable.notify_one();
        Some(bytes)
    }
}

impl Drop for TerminalOutputReader {
    fn drop(&mut self) {
        self.buffer.release_reader();
    }
}
