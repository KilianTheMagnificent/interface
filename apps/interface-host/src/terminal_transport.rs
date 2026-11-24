//! Bounded binary transport for terminal bytes.
//!
//! Terminal lifecycle and policy remain on the ordinary JSON control plane.
//! This module only multiplexes ordered byte frames between the Rust host and
//! Electron. The child process writes host-to-Electron frames on file
//! descriptor 3 and reads Electron-to-host frames on file descriptor 4.

use std::{
    collections::{HashMap, VecDeque},
    fs::OpenOptions,
    io::{self, IoSlice},
    os::unix::fs::FileTypeExt,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Notify, mpsc, watch},
    task::JoinHandle,
};
use uuid::Uuid;

pub const TERMINAL_DATA_MAGIC: [u8; 4] = *b"ITRM";
pub const TERMINAL_DATA_VERSION: u8 = 2;
pub const TERMINAL_DATA_HEADER_BYTES: usize = 44;
pub const TERMINAL_DATA_MAX_PAYLOAD_BYTES: usize = 64 * 1024;
pub const TERMINAL_DATA_HOST_OUTPUT_FD: i32 = 3;
pub const TERMINAL_DATA_HOST_INPUT_FD: i32 = 4;
// Cap is mandatory: popping into an unbounded scratch frees queue byte
// accounting before those bytes are written, so a saturated pane could refill
// while its previous frames still sit unwritten.
const TERMINAL_WRITER_BATCH_BYTES: usize = 256 * 1024;
// Credits span Rust, MessagePort and the completed xterm write, per generation.
const TERMINAL_OUTPUT_WINDOW_BYTES: usize = 256 * 1024;
const TERMINAL_OUTPUT_WINDOW_FRAMES: usize = 64;
/// The largest merged output payload: four such frames, headers included,
/// fill one generation's byte window exactly.
const TERMINAL_OUTPUT_BATCH_BYTES: usize =
    TERMINAL_OUTPUT_WINDOW_BYTES / 4 - TERMINAL_DATA_HEADER_BYTES;
const TERMINAL_OUTPUT_MAX_WINDOWS: usize = 128;

const MAGIC_OFFSET: usize = 0;
const VERSION_OFFSET: usize = 4;
const DIRECTION_OFFSET: usize = 5;
const KIND_OFFSET: usize = 6;
const RESERVED_OFFSET: usize = 7;
const TERMINAL_ID_OFFSET: usize = 8;
const GENERATION_OFFSET: usize = 24;
const SEQUENCE_OFFSET: usize = 32;
const PAYLOAD_LENGTH_OFFSET: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TerminalDataDirection {
    HostToElectron = 1,
    ElectronToHost = 2,
}

impl TryFrom<u8> for TerminalDataDirection {
    type Error = TerminalDataProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::HostToElectron),
            2 => Ok(Self::ElectronToHost),
            other => Err(TerminalDataProtocolError::InvalidDirection(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TerminalDataKind {
    Data = 1,
    Eof = 2,
    /// Empty Electron-to-host frame echoing the consumed output watermark.
    OutputAck = 3,
}

impl TryFrom<u8> for TerminalDataKind {
    type Error = TerminalDataProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Data),
            2 => Ok(Self::Eof),
            3 => Ok(Self::OutputAck),
            other => Err(TerminalDataProtocolError::InvalidKind(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalDataFrame {
    pub terminal_id: Uuid,
    pub generation: u64,
    pub sequence: u64,
    pub direction: TerminalDataDirection,
    pub kind: TerminalDataKind,
    pub payload: Vec<u8>,
}

impl TerminalDataFrame {
    /// Builds one validated terminal-data frame.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataProtocolError`] when the generation is zero, the
    /// payload exceeds the protocol limit, or an EOF frame carries payload.
    pub fn new(
        terminal_id: Uuid,
        generation: u64,
        sequence: u64,
        direction: TerminalDataDirection,
        kind: TerminalDataKind,
        payload: Vec<u8>,
    ) -> Result<Self, TerminalDataProtocolError> {
        let frame = Self {
            terminal_id,
            generation,
            sequence,
            direction,
            kind,
            payload,
        };
        frame.validate(TERMINAL_DATA_MAX_PAYLOAD_BYTES)?;
        Ok(frame)
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        TERMINAL_DATA_HEADER_BYTES + self.payload.len()
    }

    /// Encodes this frame in the fixed-width terminal-data wire format.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataProtocolError`] when this frame violates a wire
    /// invariant or its payload cannot be represented within the protocol.
    pub fn encode(&self) -> Result<Vec<u8>, TerminalDataProtocolError> {
        let mut encoded = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut encoded)?;
        Ok(encoded)
    }

    fn encode_into(&self, encoded: &mut Vec<u8>) -> Result<(), TerminalDataProtocolError> {
        let header = self.encode_header()?;
        encoded.reserve(self.encoded_len());
        encoded.extend_from_slice(&header);
        encoded.extend_from_slice(&self.payload);
        Ok(())
    }

    /// The validated fixed-width header that precedes this frame's payload.
    fn encode_header(&self) -> Result<[u8; TERMINAL_DATA_HEADER_BYTES], TerminalDataProtocolError> {
        self.validate(TERMINAL_DATA_MAX_PAYLOAD_BYTES)?;
        let payload_length = u32::try_from(self.payload.len()).map_err(|_| {
            TerminalDataProtocolError::PayloadTooLarge {
                actual: self.payload.len(),
                maximum: TERMINAL_DATA_MAX_PAYLOAD_BYTES,
            }
        })?;
        let mut header = [0_u8; TERMINAL_DATA_HEADER_BYTES];
        header[MAGIC_OFFSET..VERSION_OFFSET].copy_from_slice(&TERMINAL_DATA_MAGIC);
        header[VERSION_OFFSET] = TERMINAL_DATA_VERSION;
        header[DIRECTION_OFFSET] = self.direction as u8;
        header[KIND_OFFSET] = self.kind as u8;
        header[RESERVED_OFFSET] = 0;
        header[TERMINAL_ID_OFFSET..GENERATION_OFFSET].copy_from_slice(self.terminal_id.as_bytes());
        header[GENERATION_OFFSET..SEQUENCE_OFFSET].copy_from_slice(&self.generation.to_le_bytes());
        header[SEQUENCE_OFFSET..PAYLOAD_LENGTH_OFFSET]
            .copy_from_slice(&self.sequence.to_le_bytes());
        header[PAYLOAD_LENGTH_OFFSET..].copy_from_slice(&payload_length.to_le_bytes());
        Ok(header)
    }

    fn validate(&self, maximum_payload: usize) -> Result<(), TerminalDataProtocolError> {
        if self.generation == 0 {
            return Err(TerminalDataProtocolError::InvalidGeneration);
        }
        if self.payload.len() > maximum_payload {
            return Err(TerminalDataProtocolError::PayloadTooLarge {
                actual: self.payload.len(),
                maximum: maximum_payload,
            });
        }
        if self.kind == TerminalDataKind::Eof && !self.payload.is_empty() {
            return Err(TerminalDataProtocolError::EofWithPayload);
        }
        if self.kind == TerminalDataKind::OutputAck
            && (self.direction != TerminalDataDirection::ElectronToHost || !self.payload.is_empty())
        {
            return Err(TerminalDataProtocolError::InvalidAcknowledgement);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalDataProtocolError {
    #[error("terminal output acknowledgement was invalid")]
    InvalidAcknowledgement,
    #[error("terminal data frame magic did not match")]
    InvalidMagic,
    #[error("terminal data protocol version {0} is unsupported")]
    InvalidVersion(u8),
    #[error("terminal data direction {0} is invalid")]
    InvalidDirection(u8),
    #[error("terminal data kind {0} is invalid")]
    InvalidKind(u8),
    #[error("terminal data frame reserved bits were set")]
    ReservedBitsSet,
    #[error("terminal data frame used generation zero")]
    InvalidGeneration,
    #[error("terminal data payload was {actual} bytes; the maximum is {maximum}")]
    PayloadTooLarge { actual: usize, maximum: usize },
    #[error("terminal EOF frame contained a payload")]
    EofWithPayload,
    #[error("terminal data direction did not match this transport endpoint")]
    UnexpectedDirection,
    #[error("terminal data sequence was {actual}; expected {expected}")]
    InvalidSequence { expected: u64, actual: u64 },
    #[error("terminal data sequence space was exhausted")]
    SequenceExhausted,
    #[error("terminal data arrived after EOF")]
    FrameAfterEof,
    #[error("terminal data pipe ended in a partial frame ({buffered} bytes buffered)")]
    TruncatedFrame { buffered: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TerminalStreamKey {
    terminal_id: Uuid,
    generation: u64,
    direction: TerminalDataDirection,
}

impl From<&TerminalDataFrame> for TerminalStreamKey {
    fn from(frame: &TerminalDataFrame) -> Self {
        Self {
            terminal_id: frame.terminal_id,
            generation: frame.generation,
            direction: frame.direction,
        }
    }
}

/// Per-stream sequence cursor. Ended entries are retained so a post-EOF frame
/// stays [`TerminalDataProtocolError::FrameAfterEof`] rather than looking like
/// a fresh sequence 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StreamCursor {
    next: u64,
    ended: bool,
}

#[derive(Debug)]
pub struct TerminalDataDecoder {
    expected_direction: TerminalDataDirection,
    maximum_payload: usize,
    buffer: Vec<u8>,
    streams: HashMap<TerminalStreamKey, StreamCursor>,
}

impl TerminalDataDecoder {
    #[must_use]
    pub fn new(expected_direction: TerminalDataDirection, maximum_payload: usize) -> Self {
        Self {
            expected_direction,
            maximum_payload,
            buffer: Vec::new(),
            streams: HashMap::new(),
        }
    }

    /// Adds bytes to the incremental decoder and returns every complete frame.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataProtocolError`] for malformed, oversized,
    /// misdirected, out-of-order, or post-EOF frames.
    pub fn push(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<TerminalDataFrame>, TerminalDataProtocolError> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut consumed = 0;
        let result = self.push_from_buffer(&mut frames, &mut consumed);
        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        result?;
        Ok(frames)
    }

    fn push_from_buffer(
        &mut self,
        frames: &mut Vec<TerminalDataFrame>,
        consumed: &mut usize,
    ) -> Result<(), TerminalDataProtocolError> {
        loop {
            if self.buffer.len() - *consumed < TERMINAL_DATA_HEADER_BYTES {
                return Ok(());
            }
            let (frame, frame_length) = {
                let available = &self.buffer[*consumed..];
                self.validate_header(available)?;
                let payload_length = u32::from_le_bytes(frame_array::<4>(
                    available,
                    PAYLOAD_LENGTH_OFFSET,
                    TERMINAL_DATA_HEADER_BYTES,
                )?) as usize;
                if payload_length > self.maximum_payload {
                    return Err(TerminalDataProtocolError::PayloadTooLarge {
                        actual: payload_length,
                        maximum: self.maximum_payload,
                    });
                }
                let frame_length = TERMINAL_DATA_HEADER_BYTES + payload_length;
                if available.len() < frame_length {
                    return Ok(());
                }
                let terminal_id = Uuid::from_bytes(frame_array::<16>(
                    available,
                    TERMINAL_ID_OFFSET,
                    GENERATION_OFFSET,
                )?);
                let generation = u64::from_le_bytes(frame_array::<8>(
                    available,
                    GENERATION_OFFSET,
                    SEQUENCE_OFFSET,
                )?);
                let sequence = u64::from_le_bytes(frame_array::<8>(
                    available,
                    SEQUENCE_OFFSET,
                    PAYLOAD_LENGTH_OFFSET,
                )?);
                let direction = TerminalDataDirection::try_from(available[DIRECTION_OFFSET])?;
                let kind = TerminalDataKind::try_from(available[KIND_OFFSET])?;
                let payload = available[TERMINAL_DATA_HEADER_BYTES..frame_length].to_vec();
                (
                    TerminalDataFrame {
                        terminal_id,
                        generation,
                        sequence,
                        direction,
                        kind,
                        payload,
                    },
                    frame_length,
                )
            };
            frame.validate(self.maximum_payload)?;
            self.accept_sequence(&frame)?;
            *consumed += frame_length;
            frames.push(frame);
        }
    }

    /// Confirms that the underlying pipe ended between frames.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataProtocolError::TruncatedFrame`] when EOF arrives
    /// with a partial header or payload buffered.
    pub fn finish(&self) -> Result<(), TerminalDataProtocolError> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(TerminalDataProtocolError::TruncatedFrame {
                buffered: self.buffer.len(),
            })
        }
    }

    fn validate_header(&self, buffer: &[u8]) -> Result<(), TerminalDataProtocolError> {
        if buffer.len() < TERMINAL_DATA_HEADER_BYTES {
            return Err(TerminalDataProtocolError::TruncatedFrame {
                buffered: buffer.len(),
            });
        }
        if buffer[MAGIC_OFFSET..VERSION_OFFSET] != TERMINAL_DATA_MAGIC {
            return Err(TerminalDataProtocolError::InvalidMagic);
        }
        if buffer[VERSION_OFFSET] != TERMINAL_DATA_VERSION {
            return Err(TerminalDataProtocolError::InvalidVersion(
                buffer[VERSION_OFFSET],
            ));
        }
        let direction = TerminalDataDirection::try_from(buffer[DIRECTION_OFFSET])?;
        if direction != self.expected_direction {
            return Err(TerminalDataProtocolError::UnexpectedDirection);
        }
        TerminalDataKind::try_from(buffer[KIND_OFFSET])?;
        if buffer[RESERVED_OFFSET] != 0 {
            return Err(TerminalDataProtocolError::ReservedBitsSet);
        }
        Ok(())
    }

    fn accept_sequence(
        &mut self,
        frame: &TerminalDataFrame,
    ) -> Result<(), TerminalDataProtocolError> {
        // Ack watermarks echo output; they do not consume input sequence IDs.
        if frame.kind == TerminalDataKind::OutputAck {
            return Ok(());
        }
        let key = TerminalStreamKey::from(frame);
        let cursor = self.streams.entry(key).or_default();
        if cursor.ended {
            return Err(TerminalDataProtocolError::FrameAfterEof);
        }
        if frame.sequence != cursor.next {
            return Err(TerminalDataProtocolError::InvalidSequence {
                expected: cursor.next,
                actual: frame.sequence,
            });
        }
        cursor.next = cursor
            .next
            .checked_add(1)
            .ok_or(TerminalDataProtocolError::SequenceExhausted)?;
        if frame.kind == TerminalDataKind::Eof {
            cursor.ended = true;
        }
        Ok(())
    }
}

fn frame_array<const LENGTH: usize>(
    buffer: &[u8],
    start: usize,
    end: usize,
) -> Result<[u8; LENGTH], TerminalDataProtocolError> {
    buffer
        .get(start..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(TerminalDataProtocolError::TruncatedFrame {
            buffered: buffer.len(),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalQueueLimits {
    pub total_bytes: usize,
    pub per_terminal_bytes: usize,
}

impl Default for TerminalQueueLimits {
    fn default() -> Self {
        Self {
            total_bytes: 8 * 1024 * 1024,
            per_terminal_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalQueueError {
    #[error("terminal data frame exceeded the bounded queue")]
    Backpressure,
}

#[derive(Debug, Default)]
struct TerminalQueue {
    frames: VecDeque<TerminalDataFrame>,
    bytes: usize,
}

/// Fair per-terminal outbound queue. Hand-mirrored with
/// `electron/src/main/terminal-data-transport.ts` (`FairTerminalFrameQueue`),
/// including `is_empty` / `total_bytes`.
#[derive(Debug)]
pub struct FairTerminalFrameQueue {
    limits: TerminalQueueLimits,
    queues: HashMap<Uuid, TerminalQueue>,
    active: VecDeque<Uuid>,
    total_bytes: usize,
}

impl FairTerminalFrameQueue {
    #[must_use]
    pub fn new(limits: TerminalQueueLimits) -> Self {
        Self {
            limits,
            queues: HashMap::new(),
            active: VecDeque::new(),
            total_bytes: 0,
        }
    }

    /// Adds a frame without exceeding the global or per-terminal byte limits.
    ///
    /// # Errors
    ///
    /// Returns the frame with [`TerminalQueueError::Backpressure`] when either
    /// byte limit would be exceeded. The queue is unchanged on failure.
    pub fn try_push(
        &mut self,
        frame: TerminalDataFrame,
    ) -> Result<(), (TerminalDataFrame, TerminalQueueError)> {
        let frame_bytes = frame.encoded_len();
        let current_terminal_bytes = self
            .queues
            .get(&frame.terminal_id)
            .map_or(0, |queue| queue.bytes);
        if self.total_bytes.saturating_add(frame_bytes) > self.limits.total_bytes
            || current_terminal_bytes.saturating_add(frame_bytes) > self.limits.per_terminal_bytes
        {
            return Err((frame, TerminalQueueError::Backpressure));
        }
        let terminal_id = frame.terminal_id;
        let queue = self.queues.entry(terminal_id).or_default();
        if queue.frames.is_empty() {
            self.active.push_back(terminal_id);
        }
        queue.frames.push_back(frame);
        queue.bytes += frame_bytes;
        self.total_bytes += frame_bytes;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<TerminalDataFrame> {
        while let Some(terminal_id) = self.active.pop_front() {
            let popped = self.queues.get_mut(&terminal_id).and_then(|queue| {
                let frame = queue.frames.pop_front()?;
                let frame_bytes = frame.encoded_len();
                queue.bytes = queue.bytes.saturating_sub(frame_bytes);
                Some((frame, frame_bytes, queue.frames.is_empty()))
            });
            let Some((frame, frame_bytes, empty)) = popped else {
                self.queues.remove(&terminal_id);
                continue;
            };
            self.total_bytes = self.total_bytes.saturating_sub(frame_bytes);
            if empty {
                self.queues.remove(&terminal_id);
            } else {
                self.active.push_back(terminal_id);
            }
            return Some(frame);
        }
        None
    }

    /// Retained for parity with the TypeScript twin
    /// `electron/src/main/terminal-data-transport.ts:303 FairTerminalFrameQueue`;
    /// no production caller by design (the tests below exercise it).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Retained for parity with the TypeScript twin
    /// `electron/src/main/terminal-data-transport.ts:303 FairTerminalFrameQueue`;
    /// no production caller by design (the tests below exercise it).
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTransportSide {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTransportFailure {
    pub side: TerminalTransportSide,
    pub message: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalDataReadError {
    #[error("terminal data protocol failed: {0}")]
    Protocol(#[from] TerminalDataProtocolError),
    #[error("terminal data pipe read failed: {0}")]
    Io(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalDataSendError {
    #[error(transparent)]
    Protocol(#[from] TerminalDataProtocolError),
    #[error(transparent)]
    Queue(#[from] TerminalQueueError),
    #[error("terminal data transport is closed")]
    Closed,
    #[error("terminal data transport failed: {0}")]
    Failed(String),
    #[error("terminal data transport lock was poisoned")]
    Poisoned,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalDataTransportConfig {
    pub maximum_payload_bytes: usize,
    pub queue_limits: TerminalQueueLimits,
    pub inbound_capacity_frames: usize,
    pub read_chunk_bytes: usize,
}

impl Default for TerminalDataTransportConfig {
    fn default() -> Self {
        Self {
            maximum_payload_bytes: TERMINAL_DATA_MAX_PAYLOAD_BYTES,
            queue_limits: TerminalQueueLimits::default(),
            inbound_capacity_frames: 256,
            read_chunk_bytes: 16 * 1024,
        }
    }
}

#[derive(Debug)]
struct TerminalOutboundState {
    queue: FairTerminalFrameQueue,
    streams: HashMap<TerminalStreamKey, StreamCursor>,
    outstanding: HashMap<TerminalStreamKey, OutputWindow>,
    /// A pump parked because its own stream's window is full, keyed by that
    /// stream: only that stream's acknowledgement can let it continue, so
    /// only that acknowledgement wakes it. Pumps parked on the shared limits
    /// (window count, queue bytes) wait on [`TerminalOutboundShared::capacity`].
    credit_waiters: HashMap<TerminalStreamKey, Arc<Condvar>>,
    closed: bool,
    /// Times a parked pump woke, for the wakeup tests.
    #[cfg(test)]
    wakeups: usize,
}

#[derive(Debug, Default)]
struct OutputWindow {
    frames: VecDeque<(u64, usize)>,
    bytes: usize,
}

impl TerminalOutboundState {
    fn new(limits: TerminalQueueLimits) -> Self {
        Self {
            queue: FairTerminalFrameQueue::new(limits),
            streams: HashMap::new(),
            outstanding: HashMap::new(),
            credit_waiters: HashMap::new(),
            closed: false,
            #[cfg(test)]
            wakeups: 0,
        }
    }

    /// Whether `key`'s own window has no room for `bytes` more.
    fn window_full(&self, key: &TerminalStreamKey, bytes: usize) -> bool {
        self.outstanding.get(key).is_some_and(|window| {
            window.frames.len() >= TERMINAL_OUTPUT_WINDOW_FRAMES
                || window.bytes + bytes > TERMINAL_OUTPUT_WINDOW_BYTES
        })
    }

    /// Every parker, for close and transport failure.
    fn all_credit_waiters(&self) -> Vec<Arc<Condvar>> {
        self.credit_waiters.values().cloned().collect()
    }

    fn try_accept(
        &mut self,
        frame: TerminalDataFrame,
    ) -> Result<(), (Option<TerminalDataFrame>, TerminalDataSendError)> {
        let key = TerminalStreamKey::from(&frame);
        let cursor = self.streams.get(&key).copied().unwrap_or_default();
        if cursor.ended {
            return Err((None, TerminalDataProtocolError::FrameAfterEof.into()));
        }
        if frame.sequence != cursor.next {
            return Err((
                None,
                TerminalDataProtocolError::InvalidSequence {
                    expected: cursor.next,
                    actual: frame.sequence,
                }
                .into(),
            ));
        }
        let is_eof = frame.kind == TerminalDataKind::Eof;
        let Some(next) = cursor.next.checked_add(1) else {
            return Err((None, TerminalDataProtocolError::SequenceExhausted.into()));
        };
        if !self.outstanding.contains_key(&key)
            && self.outstanding.len() >= TERMINAL_OUTPUT_MAX_WINDOWS
        {
            return Err((Some(frame), TerminalQueueError::Backpressure.into()));
        }
        let bytes = frame.encoded_len();
        if self.window_full(&key, bytes) {
            return Err((Some(frame), TerminalQueueError::Backpressure.into()));
        }
        let window = self.outstanding.entry(key).or_default();
        let sequence = frame.sequence;
        if let Err((frame, error)) = self.queue.try_push(frame) {
            if window.frames.is_empty() {
                self.outstanding.remove(&key);
            }
            return Err((Some(frame), error.into()));
        }
        window.frames.push_back((sequence, bytes));
        window.bytes += bytes;
        self.streams.insert(
            key,
            StreamCursor {
                next,
                ended: is_eof,
            },
        );
        Ok(())
    }
}

#[derive(Debug)]
struct TerminalOutboundShared {
    state: Mutex<TerminalOutboundState>,
    notify: Notify,
    capacity: Condvar,
    failures: watch::Receiver<Option<TerminalTransportFailure>>,
}

#[derive(Debug, Clone)]
pub struct TerminalDataSender {
    shared: Arc<TerminalOutboundShared>,
    maximum_payload: usize,
}

impl TerminalDataSender {
    /// The most output bytes one merged host-output frame should carry.
    #[must_use]
    pub fn output_batch_bytes(&self) -> usize {
        TERMINAL_OUTPUT_BATCH_BYTES.min(self.maximum_payload)
    }

    fn acknowledge(&self, frame: &TerminalDataFrame) -> Result<(), TerminalDataProtocolError> {
        let key = TerminalStreamKey {
            direction: TerminalDataDirection::HostToElectron,
            ..TerminalStreamKey::from(frame)
        };
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| TerminalDataProtocolError::InvalidAcknowledgement)?;
        let Some(cursor) = state.streams.get(&key).copied() else {
            return Ok(()); // Unknown/stale generations cannot grant credit.
        };
        if frame.sequence >= cursor.next {
            return Err(TerminalDataProtocolError::InvalidAcknowledgement);
        }
        let mut window_closed = false;
        if let Some(window) = state.outstanding.get_mut(&key) {
            while window
                .frames
                .front()
                .is_some_and(|(sequence, _)| *sequence <= frame.sequence)
            {
                let (_, bytes) = window.frames.pop_front().expect("checked front");
                window.bytes -= bytes;
            }
            if window.frames.is_empty() {
                state.outstanding.remove(&key);
                window_closed = true;
            }
        }
        // Credit for this stream wakes only this stream's pump. A closed
        // window also frees a slot under the window-count limit.
        let waiter = state.credit_waiters.get(&key).cloned();
        drop(state);
        if let Some(waiter) = waiter {
            waiter.notify_all();
        }
        if window_closed {
            self.shared.capacity.notify_all();
        }
        Ok(())
    }

    /// Queues one ordered host-output frame without waiting for pipe capacity.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataSendError`] when the frame is invalid or
    /// out-of-order, the bounded queue is full, or the transport has closed or
    /// failed. Backpressure returns the frame so the caller can retry at the
    /// same sequence; other errors consume it.
    pub fn enqueue_output(
        &self,
        frame: TerminalDataFrame,
    ) -> Result<(), (Option<TerminalDataFrame>, TerminalDataSendError)> {
        self.enqueue_frame(frame, None)
    }

    /// Queues one ordered host-output frame, parking while the bounded queue is
    /// full. Credits, drains, close and transport failures all wake a parked
    /// caller; `timeout` bounds each wait only as a backstop.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalDataSendError`] when the frame is invalid or
    /// out-of-order, or the transport has closed or failed. Queue backpressure
    /// is retried until the frame is accepted or a fatal error occurs.
    pub fn enqueue_output_waiting(
        &self,
        frame: TerminalDataFrame,
        timeout: Duration,
    ) -> Result<(), TerminalDataSendError> {
        match self.enqueue_frame(frame, Some(timeout)) {
            Ok(()) => Ok(()),
            Err((_, error)) => Err(error),
        }
    }

    fn enqueue_frame(
        &self,
        mut frame: TerminalDataFrame,
        wait: Option<Duration>,
    ) -> Result<(), (Option<TerminalDataFrame>, TerminalDataSendError)> {
        if let Err(error) = frame.validate(self.maximum_payload) {
            return Err((None, error.into()));
        }
        if frame.direction != TerminalDataDirection::HostToElectron {
            return Err((None, TerminalDataProtocolError::UnexpectedDirection.into()));
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| (None, TerminalDataSendError::Poisoned))?;
        loop {
            // Borrowing the failures watch under the outbound state lock is
            // safe: only `report_failure` ever *sends* on that watch, and it
            // takes this lock only after the send, never around it, so the
            // nesting cannot invert.
            if let Some(failure) = self.shared.failures.borrow().clone() {
                return Err((None, TerminalDataSendError::Failed(failure.message)));
            }
            if state.closed {
                return Err((None, TerminalDataSendError::Closed));
            }
            match state.try_accept(frame) {
                Ok(()) => {
                    drop(state);
                    self.shared.notify.notify_one();
                    return Ok(());
                }
                Err((Some(returned), error @ TerminalDataSendError::Queue(_))) => {
                    let Some(timeout) = wait else {
                        return Err((Some(returned), error));
                    };
                    frame = returned;
                    let key = TerminalStreamKey::from(&frame);
                    // Park where the refusal can end: on this stream's own
                    // credit, or on the shared capacity every pump draws on.
                    // Registered under the state lock, so no wake is lost.
                    let parker = state
                        .window_full(&key, frame.encoded_len())
                        .then(|| Arc::clone(state.credit_waiters.entry(key).or_default()));
                    let waited = match &parker {
                        Some(parker) => parker.wait_timeout(state, timeout),
                        None => self.shared.capacity.wait_timeout(state, timeout),
                    };
                    match waited {
                        Ok((guard, _)) => state = guard,
                        Err(poisoned) => {
                            drop(poisoned.into_inner());
                            return Err((Some(frame), TerminalDataSendError::Poisoned));
                        }
                    }
                    if parker.is_some() {
                        state.credit_waiters.remove(&key);
                    }
                    #[cfg(test)]
                    {
                        state.wakeups += 1;
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub fn close(&self) {
        let parkers = self.shared.state.lock().map_or_else(
            |_| Vec::new(),
            |mut state| {
                state.closed = true;
                state.all_credit_waiters()
            },
        );
        // `notify_one` stores a permit when the writer has not parked yet;
        // `notify_waiters` would drop the close signal in that window.
        self.shared.notify.notify_one();
        self.shared.capacity.notify_all();
        for parker in parkers {
            parker.notify_all();
        }
    }
}

#[derive(Debug)]
pub struct TerminalDataPlane {
    pub outbound: TerminalDataSender,
    pub inbound: mpsc::Receiver<Result<TerminalDataFrame, TerminalDataReadError>>,
    failures: watch::Receiver<Option<TerminalTransportFailure>>,
    reader_task: Option<JoinHandle<()>>,
    writer_task: Option<JoinHandle<()>>,
}

impl TerminalDataPlane {
    #[must_use]
    pub fn start<R, W>(input: R, output: W, config: TerminalDataTransportConfig) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (failures_tx, failures) = watch::channel(None);
        let shared = Arc::new(TerminalOutboundShared {
            state: Mutex::new(TerminalOutboundState::new(config.queue_limits)),
            notify: Notify::new(),
            capacity: Condvar::new(),
            failures: failures.clone(),
        });
        let outbound = TerminalDataSender {
            shared: shared.clone(),
            maximum_payload: config.maximum_payload_bytes,
        };
        let (inbound_tx, inbound) = mpsc::channel(config.inbound_capacity_frames.max(1));
        let reader_failures = failures_tx.clone();
        let reader_task = tokio::spawn(run_terminal_reader(
            input,
            inbound_tx,
            reader_failures,
            outbound.clone(),
            config.maximum_payload_bytes,
            config.read_chunk_bytes.max(1),
        ));
        let writer_task = tokio::spawn(run_terminal_writer(output, shared, failures_tx));
        Self {
            outbound,
            inbound,
            failures,
            reader_task: Some(reader_task),
            writer_task: Some(writer_task),
        }
    }

    /// Opens the inherited fd4 input and fd3 output pipes for the host process.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when either inherited descriptor cannot be opened
    /// or is neither a socket nor a pipe.
    pub fn from_process_pipes(config: TerminalDataTransportConfig) -> io::Result<Self> {
        let input = open_process_descriptor(TERMINAL_DATA_HOST_INPUT_FD, true)?;
        let output = open_process_descriptor(TERMINAL_DATA_HOST_OUTPUT_FD, false)?;
        Ok(Self::start(
            process_reader(input)?,
            process_writer(output)?,
            config,
        ))
    }

    #[must_use]
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<TerminalTransportFailure>> {
        self.failures.clone()
    }

    pub async fn shutdown(mut self) {
        self.outbound.close();
        if let Some(writer) = self.writer_task.take() {
            let _ = writer.await;
        }
        if let Some(reader) = self.reader_task.take() {
            reader.abort();
            let _ = reader.await;
        }
    }
}

impl Drop for TerminalDataPlane {
    fn drop(&mut self) {
        self.outbound.close();
        if let Some(reader) = &self.reader_task {
            reader.abort();
        }
        if let Some(writer) = &self.writer_task {
            writer.abort();
        }
    }
}

fn open_process_descriptor(descriptor: i32, read: bool) -> io::Result<std::fs::File> {
    let path = PathBuf::from(format!("/dev/fd/{descriptor}"));
    let mut options = OpenOptions::new();
    options.read(read).write(!read);
    options.open(path)
}

type ProcessReader = Box<dyn AsyncRead + Unpin + Send>;
type ProcessWriter = Box<dyn AsyncWrite + Unpin + Send>;

// Inherited stdio is a socket or a pipe, never a regular file, so it is
// driven by readiness rather than `tokio::fs::File`, which would park a
// blocking-pool thread in read(2) for the host's life and copy every write
// through its own buffer. libuv's "pipe" stdio is a socketpair on macOS.
// Opening /dev/fd shares the open file description, so O_NONBLOCK also lands
// on the inherited descriptor; nothing else in the host reads fd 3 or 4.

fn process_reader(file: std::fs::File) -> io::Result<ProcessReader> {
    let file_type = file.metadata()?.file_type();
    if file_type.is_socket() {
        Ok(Box::new(readiness_socket(file)?))
    } else if file_type.is_fifo() {
        Ok(Box::new(tokio::net::unix::pipe::Receiver::from_file(file)?))
    } else {
        Err(unsupported_descriptor())
    }
}

fn process_writer(file: std::fs::File) -> io::Result<ProcessWriter> {
    let file_type = file.metadata()?.file_type();
    if file_type.is_socket() {
        Ok(Box::new(readiness_socket(file)?))
    } else if file_type.is_fifo() {
        Ok(Box::new(tokio::net::unix::pipe::Sender::from_file(file)?))
    } else {
        Err(unsupported_descriptor())
    }
}

fn readiness_socket(file: std::fs::File) -> io::Result<tokio::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(file));
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream)
}

fn unsupported_descriptor() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "terminal data descriptor is neither a socket nor a pipe",
    )
}

async fn run_terminal_reader<R>(
    mut input: R,
    inbound: mpsc::Sender<Result<TerminalDataFrame, TerminalDataReadError>>,
    failures: watch::Sender<Option<TerminalTransportFailure>>,
    outbound: TerminalDataSender,
    maximum_payload: usize,
    read_chunk_bytes: usize,
) where
    R: AsyncRead + Unpin,
{
    let mut decoder =
        TerminalDataDecoder::new(TerminalDataDirection::ElectronToHost, maximum_payload);
    let mut buffer = vec![0_u8; read_chunk_bytes];
    loop {
        match input.read(&mut buffer).await {
            Ok(0) => {
                if let Err(error) = decoder.finish() {
                    let message = error.to_string();
                    let _ = inbound.send(Err(error.into())).await;
                    report_failure(
                        &failures,
                        TerminalTransportSide::Read,
                        message,
                        &outbound.shared,
                    );
                }
                return;
            }
            Ok(read) => match decoder.push(&buffer[..read]) {
                Ok(frames) => {
                    for frame in frames {
                        // Credits bypass PTY input lanes, including a child
                        // that is not reading stdin.
                        if frame.kind == TerminalDataKind::OutputAck {
                            if let Err(error) = outbound.acknowledge(&frame) {
                                report_failure(
                                    &failures,
                                    TerminalTransportSide::Read,
                                    error.to_string(),
                                    &outbound.shared,
                                );
                                return;
                            }
                            continue;
                        }
                        if inbound.send(Ok(frame)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = inbound.send(Err(error.into())).await;
                    report_failure(
                        &failures,
                        TerminalTransportSide::Read,
                        message,
                        &outbound.shared,
                    );
                    return;
                }
            },
            Err(error) => {
                let message = error.to_string();
                let _ = inbound
                    .send(Err(TerminalDataReadError::Io(message.clone())))
                    .await;
                report_failure(
                    &failures,
                    TerminalTransportSide::Read,
                    message,
                    &outbound.shared,
                );
                return;
            }
        }
    }
}

async fn run_terminal_writer<W>(
    mut output: W,
    shared: Arc<TerminalOutboundShared>,
    failures: watch::Sender<Option<TerminalTransportFailure>>,
) where
    W: AsyncWrite + Unpin,
{
    let vectored = output.is_write_vectored();
    let mut batch = Vec::new();
    let mut scratch = Vec::new();
    loop {
        let notified = shared.notify.notified();
        let closed_and_idle = {
            let Ok(mut state) = shared.state.lock() else {
                report_failure(
                    &failures,
                    TerminalTransportSide::Write,
                    "terminal data transport lock was poisoned".to_owned(),
                    &shared,
                );
                return;
            };
            // Only move frames out under the lock; encoding happens after it
            // is released. Popping releases queue accounting before the bytes
            // are written, so the batch stays capped.
            let mut batch_bytes = 0;
            while batch_bytes < TERMINAL_WRITER_BATCH_BYTES {
                let Some(frame) = state.queue.pop() else {
                    break;
                };
                batch_bytes += frame.encoded_len();
                batch.push(frame);
            }
            batch.is_empty() && state.closed
        };
        if !batch.is_empty() {
            shared.capacity.notify_all();
        }
        if closed_and_idle {
            return;
        }
        if batch.is_empty() {
            notified.await;
            continue;
        }
        let written = if vectored {
            write_frames_vectored(&mut output, &batch).await
        } else {
            scratch.clear();
            match batch
                .iter()
                .try_for_each(|frame| frame.encode_into(&mut scratch))
            {
                Ok(()) => output
                    .write_all(&scratch)
                    .await
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            }
        };
        batch.clear();
        if let Err(message) = written {
            report_failure(&failures, TerminalTransportSide::Write, message, &shared);
            return;
        }
    }
}

/// Writes each frame's encoded header and its payload in place, so a
/// payload is never copied into a staging buffer.
async fn write_frames_vectored<W>(
    output: &mut W,
    frames: &[TerminalDataFrame],
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let headers = frames
        .iter()
        .map(TerminalDataFrame::encode_header)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut slices = headers
        .iter()
        .zip(frames)
        .flat_map(|(header, frame)| [IoSlice::new(header), IoSlice::new(&frame.payload)])
        .collect::<Vec<_>>();
    let mut remaining = slices.as_mut_slice();
    IoSlice::advance_slices(&mut remaining, 0);
    while !remaining.is_empty() {
        let written = output
            .write_vectored(remaining)
            .await
            .map_err(|error| error.to_string())?;
        if written == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero).to_string());
        }
        IoSlice::advance_slices(&mut remaining, written);
    }
    Ok(())
}

fn report_failure(
    failures: &watch::Sender<Option<TerminalTransportFailure>>,
    side: TerminalTransportSide,
    message: String,
    shared: &TerminalOutboundShared,
) {
    if failures.borrow().is_none() {
        let _ = failures.send(Some(TerminalTransportFailure { side, message }));
    }
    // A parked pump checks this watch under the outbound state lock and then
    // waits on `capacity` or its stream's parker. Passing through that lock
    // orders this notify after any check that missed the failure, so the
    // wakeup cannot be lost and the pump's wait timeout is only a backstop.
    let parkers = shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .all_credit_waiters();
    shared.capacity.notify_all();
    for parker in parkers {
        parker.notify_all();
    }
}

#[cfg(test)]
#[path = "terminal_transport_tests.rs"]
mod tests;
