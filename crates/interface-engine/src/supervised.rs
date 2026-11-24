//! One supervised JSON-RPC provider child over stdio.
//!
//! engine-codex and engine-acp spawn, write, read, drain stderr and bound
//! their child the same way. What stays provider-shaped is the event its
//! session reads and the nouns in the frozen error prose; both come from the
//! [`Classify`] impl on that event, so a transport fix is made once.

use std::{ffi::OsString, marker::PhantomData, sync::Arc, time::Duration};

use serde::Serialize;
use serde_json::{Value, to_value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    time::sleep,
};

use crate::{
    executable::ProcessSpec,
    jsonrpc::{
        FrameError, NdjsonDecoder, PendingRouter, PreparedRequest, RouteEvent, RpcId,
        RpcRemoteError, delivery_from_rpc, frame_error, notification_frame, prepare_error,
        server_error_frame, server_result_frame,
    },
    outcome::{DeliveryOutcome, EngineError, EngineErrorKind},
};

/// Upper bound on encoded frame payload held by each supervised channel.
pub const FRAME_QUEUE_BYTES: usize = 64 * 1024 * 1024;

pub const DEFAULT_MAX_PENDING: usize = 64;
pub const DEFAULT_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_INBOUND_CAPACITY: usize = 512;
pub const DEFAULT_OUTBOUND_CAPACITY: usize = 64;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct TransportLimits {
    pub max_pending: usize,
    pub max_frame_bytes: usize,
    pub inbound_capacity: usize,
    pub outbound_capacity: usize,
    pub request_timeout: Duration,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            max_pending: DEFAULT_MAX_PENDING,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            inbound_capacity: DEFAULT_INBOUND_CAPACITY,
            outbound_capacity: DEFAULT_OUTBOUND_CAPACITY,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl TransportLimits {
    /// Count and maximum frame size jointly bound queued encoded payloads.
    /// The outbound queue uses this; the inbound queue counts the bytes each
    /// read actually queued (see `InboundSender`).
    #[must_use]
    pub fn frame_queue_capacity(&self, requested: usize) -> usize {
        requested
            .min(FRAME_QUEUE_BYTES / self.max_frame_bytes.saturating_add(1))
            .max(1)
    }
}

/// The stdout reader's half of a supervised connection's inbound queue.
///
/// The queue holds at most `inbound_capacity` events and at most
/// [`FRAME_QUEUE_BYTES`] of the stdout they were decoded from, counted per
/// read rather than as count times the frame limit. A full queue makes the
/// reader wait, which backs pressure onto the child's stdout pipe: a burst of
/// small frames while the session is busy is ordinary provider output, not a
/// fault. A single frame over the frame limit is still refused by the decoder.
struct InboundSender<T> {
    events: mpsc::Sender<QueuedInbound<T>>,
    bytes: Arc<Semaphore>,
    byte_budget: usize,
}

/// The session's half of the inbound queue. Receiving an event releases the
/// bytes its read was charged.
pub struct InboundReceiver<T> {
    events: mpsc::Receiver<QueuedInbound<T>>,
}

struct QueuedInbound<T> {
    event: T,
    _bytes: Option<OwnedSemaphorePermit>,
}

/// Create one connection's inbound queue from its limits.
#[must_use]
fn inbound_queue<T>(limits: &TransportLimits) -> (InboundSender<T>, InboundReceiver<T>) {
    inbound_queue_with_budget(limits.inbound_capacity, FRAME_QUEUE_BYTES)
}

fn inbound_queue_with_budget<T>(
    capacity: usize,
    byte_budget: usize,
) -> (InboundSender<T>, InboundReceiver<T>) {
    let (events, receiver) = mpsc::channel(capacity.max(1));
    (
        InboundSender {
            events,
            bytes: Arc::new(Semaphore::new(byte_budget)),
            byte_budget,
        },
        InboundReceiver { events: receiver },
    )
}

impl<T> Clone for InboundSender<T> {
    fn clone(&self) -> Self {
        Self {
            events: self.events.clone(),
            bytes: Arc::clone(&self.bytes),
            byte_budget: self.byte_budget,
        }
    }
}

impl<T> InboundSender<T> {
    /// Queue one read's decoded events in order, charging them the `bytes` of
    /// stdout they came from and waiting for room when the queue is full.
    ///
    /// Room that is already free is always taken first, so a batch is never
    /// dropped merely because shutdown raced it. Returns `false`, having
    /// queued only a prefix, once `shutdown` changes while waiting or the
    /// session has dropped its receiver.
    pub async fn send_batch<I>(
        &self,
        events: I,
        bytes: usize,
        shutdown: &mut watch::Receiver<bool>,
    ) -> bool
    where
        I: IntoIterator<Item = T>,
    {
        let mut events = events.into_iter().peekable();
        if events.peek().is_none() {
            return true;
        }
        // A read is charged at most one frame limit plus one read buffer.
        // Clamping to the budget keeps the wait satisfiable even at the
        // largest frame limit validate_limits admits.
        let charge = u32::try_from(bytes.min(self.byte_budget)).unwrap_or(u32::MAX);
        let mut permit = if charge == 0 {
            None
        } else {
            tokio::select! {
                biased;
                permit = Arc::clone(&self.bytes).acquire_many_owned(charge) => match permit {
                    Ok(permit) => Some(permit),
                    Err(_) => return false,
                },
                () = self.events.closed() => return false,
                changed = shutdown.changed() => {
                    let _ = changed;
                    return false;
                }
            }
        };
        while let Some(event) = events.next() {
            let slot = tokio::select! {
                biased;
                slot = self.events.reserve() => match slot {
                    Ok(slot) => slot,
                    Err(_) => return false,
                },
                changed = shutdown.changed() => {
                    let _ = changed;
                    return false;
                }
            };
            // The last event carries the read's charge, so the bytes stay
            // counted until the whole batch has left the queue.
            let bytes = if events.peek().is_none() {
                permit.take()
            } else {
                None
            };
            slot.send(QueuedInbound {
                event,
                _bytes: bytes,
            });
        }
        true
    }

    /// Queue a terminal event without waiting and without a byte charge.
    /// Returns `false` when the queue is full or the receiver is gone.
    pub fn try_send(&self, event: T) -> bool {
        self.events
            .try_send(QueuedInbound {
                event,
                _bytes: None,
            })
            .is_ok()
    }
}

impl<T> InboundReceiver<T> {
    /// A receiver whose sender is already gone; `recv` returns `None` at once.
    #[must_use]
    pub fn closed() -> Self {
        Self {
            events: mpsc::channel(1).1,
        }
    }

    /// Receive the next event, releasing its read's byte charge.
    pub async fn recv(&mut self) -> Option<T> {
        self.events.recv().await.map(|queued| queued.event)
    }
}

/// One encoded frame queued for the writer, with the dispatch-ledger id it
/// belongs to and an optional delivery receipt.
struct OutboundFrame {
    bytes: Vec<u8>,
    request_id: Option<RpcId>,
    receipt: Option<oneshot::Sender<DeliveryOutcome<()>>>,
}

/// Refuse a transport whose bounds would disable a limit.
///
/// # Errors
///
/// Returns a definitely-unsent error when any limit is zero.
pub fn validate_limits(label: &str, limits: &TransportLimits) -> Result<(), EngineError> {
    if limits.max_pending == 0
        || limits.max_frame_bytes == 0
        || limits.inbound_capacity == 0
        || limits.outbound_capacity == 0
        || limits.request_timeout.is_zero()
    {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("{label} transport limits must be positive"),
            false,
        ));
    }
    if limits.max_frame_bytes >= FRAME_QUEUE_BYTES {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("{label} frame limit must fit its bounded frame queue"),
            false,
        ));
    }
    Ok(())
}

/// `child` is the process's full noun, e.g. `"Codex app-server"`.
#[must_use]
fn missing_pipe(child: &str) -> EngineError {
    EngineError::definitely_unsent(
        EngineErrorKind::Transport,
        format!("{child} did not expose piped stdio"),
        false,
    )
}

/// Prose for a frame refused before the writer accepted it. Both arms are
/// replay-safe: nothing reached the child's stdin.
#[must_use]
fn writer_send_message(label: &str, error: &mpsc::error::TrySendError<OutboundFrame>) -> String {
    match error {
        mpsc::error::TrySendError::Full(_) => format!("{label} writer queue is full"),
        mpsc::error::TrySendError::Closed(_) => format!("{label} writer is closed"),
    }
}

/// Drain the outbound queue into the child's stdin, keeping the dispatch ledger
/// honest: `Prepared` → `WriteStarted` → `FrameAccepted`, and a write that fails
/// after dispatch began settles as outcome-unknown, never unsent.
async fn writer_loop(
    label: &'static str,
    mut stdin: ChildStdin,
    mut outbound: mpsc::Receiver<OutboundFrame>,
    router: PendingRouter,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        let frame = tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                let _ = changed;
                break;
            }
            frame = outbound.recv() => {
                let Some(frame) = frame else { break; };
                frame
            }
        };
        if let Some(id) = &frame.request_id {
            if !router.mark_write_started(id) {
                continue;
            }
        }
        let result = async {
            stdin.write_all(&frame.bytes).await?;
            stdin.flush().await
        }
        .await;
        match result {
            Ok(()) => {
                if let Some(id) = &frame.request_id {
                    let _ = router.mark_frame_accepted(id);
                }
                if let Some(receipt) = frame.receipt {
                    let _ = receipt.send(DeliveryOutcome::Confirmed(()));
                }
            }
            Err(error) => {
                let message = format!("{label} writer failed after dispatch began: {error}");
                if let Some(id) = &frame.request_id {
                    let _ = router.expire(id, message.clone());
                }
                if let Some(receipt) = frame.receipt {
                    let _ = receipt.send(DeliveryOutcome::OutcomeUnknown(
                        EngineError::outcome_unknown(
                            EngineErrorKind::Transport,
                            message.clone(),
                            true,
                        ),
                    ));
                }
                router.transport_failed(message);
                let _ = shutdown.send(true);
                break;
            }
        }
    }
}

/// Drain the child's stderr without mirroring it: provider stderr can carry
/// credential material and is never surfaced.
async fn stderr_loop(mut stderr: ChildStderr, mut shutdown: watch::Receiver<bool>) {
    let mut buffer = [0_u8; 4_096];
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            read = stderr.read(&mut buffer) => match read {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }
}

/// The provider-shaped half of a supervised transport, implemented by the
/// event its session reads. Every decoded stdout line is routed through the
/// shared [`PendingRouter`] first; the provider decides what the result
/// becomes and which nouns its frozen error prose uses.
pub trait Classify: Send + Sized + 'static {
    /// The provider noun in transport prose, e.g. `"Codex"` or `"ACP"`.
    const LABEL: &'static str;
    /// The child process's full noun, e.g. `"Codex app-server"`.
    const CHILD: &'static str;

    /// One routed stdout line, or the framing error that replaced it.
    fn classify(decoded: Result<RouteEvent, FrameError>) -> Self;

    /// The terminal event carrying why the transport ended.
    fn closed(message: String) -> Self;
}

/// Process-free NDJSON wire core used by a child-stdio owner.
pub struct Wire<E> {
    decoder: NdjsonDecoder,
    router: PendingRouter,
    decoded: Vec<Result<Value, FrameError>>,
    events: PhantomData<fn() -> E>,
}

impl<E: Classify> Wire<E> {
    /// Attach a decoder to the same router used by an independent writer.
    ///
    /// Keeping the read loop independent is required: awaiting a request from
    /// the task that drains stdout would deadlock every response.
    #[must_use]
    pub fn with_router(router: PendingRouter, max_frame_bytes: usize) -> Self {
        Self {
            decoder: NdjsonDecoder::new(max_frame_bytes),
            router,
            decoded: Vec::new(),
            events: PhantomData,
        }
    }

    /// Routes the chunk into `out`, which is **cleared first**: the caller owns
    /// one buffer across reads and never has to empty it itself. Returns the
    /// stdout bytes the routed batch was decoded from (see
    /// [`NdjsonDecoder::push_into`]).
    pub fn push_stdout(&mut self, chunk: &[u8], out: &mut Vec<E>) -> usize {
        let bytes = self.decoder.push_into(chunk, &mut self.decoded);
        self.classify_decoded(out);
        bytes
    }

    /// Same clear-on-entry contract and return value as [`Wire::push_stdout`].
    pub fn finish_stdout(&mut self, out: &mut Vec<E>) -> usize {
        let bytes = self.decoder.finish_into(&mut self.decoded);
        self.classify_decoded(out);
        bytes
    }

    fn classify_decoded(&mut self, out: &mut Vec<E>) {
        out.clear();
        out.reserve(self.decoded.len());
        for decoded in self.decoded.drain(..) {
            out.push(E::classify(decoded.map(|value| self.router.route(value))));
        }
    }

    #[must_use]
    pub const fn router(&self) -> &PendingRouter {
        &self.router
    }
}

/// Cloneable request side of one supervised provider child. `E` fixes the
/// nouns in its prose and the event [`SupervisedTransport::spawn`] hands the
/// session.
pub struct SupervisedTransport<E> {
    router: PendingRouter,
    outbound: mpsc::Sender<OutboundFrame>,
    shutdown: watch::Sender<bool>,
    max_frame_bytes: usize,
    request_timeout: Duration,
    /// The child's pid as spawned (`None` if it had already exited).
    process_id: Option<u32>,
    events: PhantomData<fn() -> E>,
}

/// A spawned transport and the session's half of its inbound queue.
pub struct Connection<E> {
    pub transport: Arc<SupervisedTransport<E>>,
    pub inbound: InboundReceiver<E>,
}

impl<E: Classify> SupervisedTransport<E> {
    /// Spawn the exact argv-only process and supervise all three stdio pipes.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent error if the process cannot be spawned or
    /// does not expose the required pipes.
    pub fn spawn(
        spec: &ProcessSpec,
        limits: &TransportLimits,
    ) -> Result<Connection<E>, EngineError> {
        Self::spawn_command(spec.command(), limits)
    }

    /// Spawn with a small host-owned environment overlay. This remains
    /// argv-only and inherits the ordinary process environment; the caller
    /// decides which launches receive an overlay.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent error if the process cannot be spawned or
    /// does not expose the required pipes.
    pub fn spawn_with_environment(
        spec: &ProcessSpec,
        limits: &TransportLimits,
        environment: &[(OsString, OsString)],
    ) -> Result<Connection<E>, EngineError> {
        let mut command = spec.command();
        command.envs(
            environment
                .iter()
                .map(|(name, value)| (name.as_os_str(), value.as_os_str())),
        );
        Self::spawn_command(command, limits)
    }

    fn spawn_command(
        mut command: Command,
        limits: &TransportLimits,
    ) -> Result<Connection<E>, EngineError> {
        validate_limits(E::LABEL, limits)?;
        let mut child = command.spawn().map_err(|error| {
            EngineError::definitely_unsent(
                EngineErrorKind::Unavailable,
                format!("could not start {}: {error}", E::CHILD),
                true,
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| missing_pipe(E::CHILD))?;
        let stdout = child.stdout.take().ok_or_else(|| missing_pipe(E::CHILD))?;
        let stderr = child.stderr.take().ok_or_else(|| missing_pipe(E::CHILD))?;
        let process_id = child.id();

        let router = PendingRouter::new(limits.max_pending, limits.max_frame_bytes);
        let (outbound_tx, outbound_rx) =
            mpsc::channel(limits.frame_queue_capacity(limits.outbound_capacity));
        let (inbound_tx, inbound_rx) = inbound_queue(limits);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(writer_loop(
            E::LABEL,
            stdin,
            outbound_rx,
            router.clone(),
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        ));
        tokio::spawn(reader_loop(
            stdout,
            inbound_tx.clone(),
            router.clone(),
            limits.max_frame_bytes,
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        ));
        tokio::spawn(stderr_loop(stderr, shutdown_rx.clone()));
        tokio::spawn(supervise(
            child,
            router.clone(),
            inbound_tx,
            shutdown_tx.clone(),
            shutdown_rx,
        ));

        Ok(Connection {
            transport: Arc::new(Self {
                router,
                outbound: outbound_tx,
                shutdown: shutdown_tx,
                max_frame_bytes: limits.max_frame_bytes,
                request_timeout: limits.request_timeout,
                process_id,
                events: PhantomData,
            }),
            inbound: inbound_rx,
        })
    }

    /// The provider child's pid as spawned, for naming it to a team tools
    /// binding; `None` when it had exited before its pid was read.
    #[must_use]
    pub const fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    pub async fn request<T: Serialize>(&self, method: &str, params: &T) -> DeliveryOutcome<Value> {
        self.request_with_timeout(method, params, self.request_timeout)
            .await
    }

    pub async fn request_with_timeout<T: Serialize>(
        &self,
        method: &str,
        params: &T,
        timeout: Duration,
    ) -> DeliveryOutcome<Value> {
        self.request_with_timeout_reporting_dispatch(method, params, timeout, None)
            .await
    }

    /// `request_with_timeout`, firing `dispatched` as soon as the request
    /// frame is queued for the writer, or refused before it, and only then
    /// awaiting the response. A frame queued after that is written after this
    /// one: the writer drains its queue in order.
    pub async fn request_with_timeout_reporting_dispatch<T: Serialize>(
        &self,
        method: &str,
        params: &T,
        timeout: Duration,
        dispatched: Option<oneshot::Sender<()>>,
    ) -> DeliveryOutcome<Value> {
        if timeout.is_zero() {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                format!("{} request timeout must be positive", E::LABEL),
                false,
            ));
        }
        match encode_params(method, params) {
            Ok(params) => {
                self.dispatch(method, Some(params), timeout, dispatched)
                    .await
            }
            Err(error) => DeliveryOutcome::DefinitelyUnsent(error),
        }
    }

    pub async fn request_without_params(&self, method: &str) -> DeliveryOutcome<Value> {
        self.dispatch(method, None, self.request_timeout, None)
            .await
    }

    async fn dispatch(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
        dispatched: Option<oneshot::Sender<()>>,
    ) -> DeliveryOutcome<Value> {
        let prepared = match self.router.prepare_request(method, params) {
            Ok(prepared) => prepared,
            Err(error) => return prepare_error(method, &error),
        };
        let PreparedRequest {
            id,
            frame,
            completion,
        } = prepared;
        if let Err(error) = self.outbound.try_send(OutboundFrame {
            bytes: frame,
            request_id: Some(id.clone()),
            receipt: None,
        }) {
            let _ = self
                .router
                .fail_before_acceptance(&id, writer_send_message(E::LABEL, &error));
        }
        if let Some(dispatched) = dispatched {
            let _ = dispatched.send(());
        }

        let mut outcome = std::pin::pin!(completion.outcome());
        tokio::select! {
            result = &mut outcome => delivery_from_rpc(result),
            () = sleep(timeout) => {
                let _ = self.router.expire(
                    &id,
                    format!("timed out waiting for {method} response"),
                );
                delivery_from_rpc(outcome.await)
            }
        }
    }

    pub async fn notify<T: Serialize>(&self, method: &str, params: &T) -> DeliveryOutcome<()> {
        let params = match encode_params(method, params) {
            Ok(params) => params,
            Err(error) => return DeliveryOutcome::DefinitelyUnsent(error),
        };
        match notification_frame(method, Some(params), self.max_frame_bytes) {
            Ok(frame) => self.send_raw(frame).await,
            Err(error) => frame_error(error.to_string()),
        }
    }

    pub async fn server_result(&self, id: &RpcId, result: Value) -> DeliveryOutcome<()> {
        match server_result_frame(id, result, self.max_frame_bytes) {
            Ok(frame) => self.send_raw(frame).await,
            Err(error) => frame_error(error.to_string()),
        }
    }

    pub async fn server_error(&self, id: &RpcId, error: &RpcRemoteError) -> DeliveryOutcome<()> {
        match server_error_frame(id, error, self.max_frame_bytes) {
            Ok(frame) => self.send_raw(frame).await,
            Err(error) => frame_error(error.to_string()),
        }
    }

    async fn send_raw(&self, bytes: Vec<u8>) -> DeliveryOutcome<()> {
        let (receipt_tx, receipt_rx) = oneshot::channel();
        if let Err(error) = self.outbound.try_send(OutboundFrame {
            bytes,
            request_id: None,
            receipt: Some(receipt_tx),
        }) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Capacity,
                writer_send_message(E::LABEL, &error),
                true,
            ));
        }
        tokio::select! {
            result = receipt_rx => result.unwrap_or_else(|_| {
                DeliveryOutcome::OutcomeUnknown(EngineError::outcome_unknown(
                    EngineErrorKind::Transport,
                    format!("{} writer ended after accepting the frame", E::LABEL),
                    true,
                ))
            }),
            () = sleep(self.request_timeout) => DeliveryOutcome::OutcomeUnknown(
                EngineError::outcome_unknown(
                    EngineErrorKind::Timeout,
                    format!("{} writer did not confirm the accepted frame", E::LABEL),
                    true,
                ),
            ),
        }
    }

    pub fn shutdown(&self) {
        self.router
            .transport_failed(format!("{} transport was deliberately closed", E::LABEL));
        let _ = self.shutdown.send(true);
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.shutdown.borrow() || self.outbound.is_closed()
    }
}

fn encode_params<T: Serialize>(method: &str, params: &T) -> Result<Value, EngineError> {
    to_value(params).map_err(|error| {
        EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("could not encode {method}: {error}"),
            false,
        )
    })
}

async fn reader_loop<E: Classify>(
    mut stdout: ChildStdout,
    inbound: InboundSender<E>,
    router: PendingRouter,
    max_frame_bytes: usize,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut wire = Wire::<E>::with_router(router.clone(), max_frame_bytes);
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut events = Vec::new();
    loop {
        let read = tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                let _ = changed;
                return;
            }
            read = stdout.read(&mut buffer) => read,
        };
        match read {
            Ok(0) => {
                let bytes = wire.finish_stdout(&mut events);
                let _ = inbound
                    .send_batch(events.drain(..), bytes, &mut shutdown_rx)
                    .await;
                let message = format!("{} closed stdout", E::CHILD);
                router.transport_failed(message.clone());
                let _ = inbound.try_send(E::closed(message));
                let _ = shutdown.send(true);
                return;
            }
            Ok(count) => {
                let bytes = wire.push_stdout(&buffer[..count], &mut events);
                // Waits while the session is busy; stops only when the
                // transport is shutting down or the session is gone.
                if !inbound
                    .send_batch(events.drain(..), bytes, &mut shutdown_rx)
                    .await
                {
                    router.transport_failed(format!("{} inbound queue closed", E::LABEL));
                    let _ = shutdown.send(true);
                    return;
                }
            }
            Err(error) => {
                let message = format!("{} stdout read failed: {error}", E::LABEL);
                router.transport_failed(message.clone());
                let _ = inbound.try_send(E::closed(message));
                let _ = shutdown.send(true);
                return;
            }
        }
    }
}

/// Reports the child's own exit to the router and the session, or kills and
/// reaps it once the transport shuts down.
async fn supervise<E: Classify>(
    mut child: Child,
    router: PendingRouter,
    inbound: InboundSender<E>,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::select! {
        status = child.wait() => {
            let message = match status {
                Ok(status) => format!("{} exited with {status}", E::CHILD),
                Err(error) => format!("{} wait failed: {error}", E::CHILD),
            };
            router.transport_failed(message.clone());
            let _ = inbound.try_send(E::closed(message));
            let _ = shutdown.send(true);
        }
        changed = shutdown_rx.changed() => {
            let _ = changed;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn frame_channels_are_bounded_by_count_and_worst_case_encoded_bytes() {
        let mut limits = TransportLimits::default();
        for bytes in [1, 4096, DEFAULT_MAX_FRAME_BYTES, FRAME_QUEUE_BYTES - 1] {
            limits.max_frame_bytes = bytes;
            validate_limits("fixture", &limits).unwrap();
            for requested in [1, 64, 512] {
                let count = limits.frame_queue_capacity(requested);
                assert!(count <= requested);
                assert!(count * (bytes + 1) <= FRAME_QUEUE_BYTES);
            }
        }
        limits.max_frame_bytes = FRAME_QUEUE_BYTES;
        assert!(validate_limits("fixture", &limits).is_err());
    }
}

#[cfg(test)]
mod inbound_queue_tests {
    use tokio::time::{sleep, timeout};

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);
    const SETTLE: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn a_burst_past_the_count_bound_waits_for_the_session_and_arrives_in_order() {
        let (sender, mut receiver) = inbound_queue_with_budget::<u32>(4, FRAME_QUEUE_BYTES);
        let (shutdown_tx, mut shutdown) = watch::channel(false);
        let reader =
            tokio::spawn(async move { sender.send_batch(0..64, 64 * 40, &mut shutdown).await });
        // The session is busy: the reader fills the queue and waits instead of failing.
        sleep(SETTLE).await;
        assert!(!reader.is_finished());
        for expected in 0..64 {
            assert_eq!(
                timeout(WAIT, receiver.recv()).await.unwrap(),
                Some(expected)
            );
        }
        assert!(timeout(WAIT, reader).await.unwrap().unwrap());
        drop(shutdown_tx);
    }

    #[tokio::test]
    async fn queued_bytes_bound_the_queue_until_a_reads_last_event_leaves() {
        let (sender, mut receiver) = inbound_queue_with_budget::<&str>(512, 100);
        let (shutdown_tx, mut shutdown) = watch::channel(false);
        assert!(sender.send_batch(["a", "b"], 60, &mut shutdown).await);
        let second = {
            let sender = sender.clone();
            let mut shutdown = shutdown.clone();
            tokio::spawn(async move { sender.send_batch(["c"], 60, &mut shutdown).await })
        };
        sleep(SETTLE).await;
        assert!(!second.is_finished());
        assert_eq!(receiver.recv().await, Some("a"));
        sleep(SETTLE).await;
        assert!(
            !second.is_finished(),
            "the first read is charged until \"b\" leaves"
        );
        assert_eq!(receiver.recv().await, Some("b"));
        assert!(timeout(WAIT, second).await.unwrap().unwrap());
        assert_eq!(receiver.recv().await, Some("c"));
        // A charge above the whole budget is clamped rather than waiting forever.
        assert!(
            timeout(WAIT, sender.send_batch(["d"], 1_000, &mut shutdown))
                .await
                .unwrap()
        );
        // Terminal events are never charged and never wait.
        assert!(sender.try_send("closed"));
        assert_eq!(receiver.recv().await, Some("d"));
        assert_eq!(receiver.recv().await, Some("closed"));
        drop(shutdown_tx);
    }

    #[tokio::test]
    async fn shutdown_or_a_dropped_session_releases_a_waiting_reader() {
        let (sender, mut receiver) = inbound_queue_with_budget::<u32>(1, FRAME_QUEUE_BYTES);
        let (shutdown_tx, mut shutdown) = watch::channel(false);
        let waiting =
            tokio::spawn(async move { sender.send_batch([1, 2], 2, &mut shutdown).await });
        sleep(SETTLE).await;
        assert!(!waiting.is_finished());
        shutdown_tx.send(true).unwrap();
        assert!(!timeout(WAIT, waiting).await.unwrap().unwrap());
        assert_eq!(receiver.recv().await, Some(1));

        let (sender, receiver) = inbound_queue_with_budget::<u32>(8, 10);
        let (shutdown_tx, mut shutdown) = watch::channel(false);
        assert!(sender.send_batch([1], 10, &mut shutdown).await);
        let waiting = tokio::spawn(async move { sender.send_batch([2], 10, &mut shutdown).await });
        sleep(SETTLE).await;
        assert!(!waiting.is_finished());
        drop(receiver);
        assert!(!timeout(WAIT, waiting).await.unwrap().unwrap());
        drop(shutdown_tx);
    }

    #[tokio::test]
    async fn free_room_is_taken_even_after_shutdown() {
        let (sender, mut receiver) = inbound_queue_with_budget::<u32>(4, 100);
        let (shutdown_tx, mut shutdown) = watch::channel(false);
        shutdown_tx.send(true).unwrap();
        assert!(sender.send_batch([1, 2], 10, &mut shutdown).await);
        assert_eq!(receiver.recv().await, Some(1));
        assert_eq!(receiver.recv().await, Some(2));
    }

    #[tokio::test]
    async fn a_closed_receiver_is_empty() {
        assert_eq!(InboundReceiver::<u32>::closed().recv().await, None);
    }
}

/// The one transport both providers run on, pinned through a fixture event:
/// every provider message is one of these templates around its [`Classify`]
/// nouns, which each provider crate pins in turn.
#[cfg(test)]
mod transport_tests {
    use serde_json::json;
    use tokio::time::timeout;

    use super::*;
    use crate::outcome::EffectCertainty;

    const WAIT: Duration = Duration::from_secs(10);

    #[derive(Debug, PartialEq)]
    enum Fixture {
        Line(Result<RouteEvent, FrameError>),
        Closed(String),
    }

    impl Classify for Fixture {
        const LABEL: &'static str = "Fixture";
        const CHILD: &'static str = "fixture child";

        fn classify(decoded: Result<RouteEvent, FrameError>) -> Self {
            Self::Line(decoded)
        }

        fn closed(message: String) -> Self {
            Self::Closed(message)
        }
    }

    fn python(script: &str) -> ProcessSpec {
        ProcessSpec {
            program: "/usr/bin/env".into(),
            args: vec!["python3".into(), "-c".into(), script.into()],
            cwd: None,
        }
    }

    fn spawn(script: &str) -> Connection<Fixture> {
        let limits = TransportLimits {
            request_timeout: WAIT,
            ..TransportLimits::default()
        };
        SupervisedTransport::<Fixture>::spawn(&python(script), &limits).unwrap()
    }

    fn message<T: std::fmt::Debug>(outcome: DeliveryOutcome<T>) -> (EffectCertainty, String) {
        match outcome {
            DeliveryOutcome::DefinitelyUnsent(error)
            | DeliveryOutcome::DefinitelyRejected(error)
            | DeliveryOutcome::OutcomeUnknown(error) => (error.certainty, error.message),
            DeliveryOutcome::Confirmed(value) => panic!("unexpected confirmation: {value:?}"),
        }
    }

    #[tokio::test]
    async fn a_child_that_cannot_start_is_named_and_definitely_unsent() {
        let spec = ProcessSpec {
            program: "/nonexistent/interface-fixture".into(),
            args: Vec::new(),
            cwd: None,
        };
        let Err(error) = SupervisedTransport::<Fixture>::spawn(&spec, &TransportLimits::default())
        else {
            panic!("a missing executable cannot start");
        };
        assert!(error.message.starts_with("could not start fixture child: "));
        assert_eq!(error.kind, EngineErrorKind::Unavailable);
        assert!(error.replay_is_safe());
        let limits = TransportLimits {
            max_pending: 0,
            ..TransportLimits::default()
        };
        let Err(error) = SupervisedTransport::<Fixture>::spawn(&spec, &limits) else {
            panic!("zero limits are refused");
        };
        assert_eq!(error.message, "Fixture transport limits must be positive");
    }

    #[tokio::test]
    async fn closed_stdout_settles_requests_as_unknown_and_ends_the_session_stream() {
        let Connection {
            transport,
            mut inbound,
        } = spawn("import os, sys, time\nsys.stdin.readline()\nos.close(1)\ntime.sleep(10)\n");
        let outcome = timeout(WAIT, transport.request("hang", &json!({})))
            .await
            .unwrap();
        assert_eq!(
            message(outcome),
            (
                EffectCertainty::OutcomeUnknown,
                "hang: fixture child closed stdout".into()
            )
        );
        assert_eq!(
            timeout(WAIT, inbound.recv()).await.unwrap(),
            Some(Fixture::Closed("fixture child closed stdout".into()))
        );
        assert!(transport.is_closed());
    }

    #[tokio::test]
    async fn the_childs_own_exit_is_reported_with_its_status() {
        // A grandchild keeps stdout open, so the exit, not EOF, ends the transport.
        let Connection {
            transport,
            mut inbound,
        } = spawn(
            "import os, time\nif os.fork() == 0:\n    time.sleep(3)\n    os._exit(0)\nos._exit(3)\n",
        );
        assert_eq!(
            timeout(WAIT, inbound.recv()).await.unwrap(),
            Some(Fixture::Closed(
                "fixture child exited with exit status: 3".into()
            ))
        );
        assert!(transport.is_closed());
    }

    #[tokio::test]
    async fn requests_keep_their_timeout_shutdown_and_zero_timeout_prose() {
        let Connection { transport, .. } = spawn("import time\ntime.sleep(10)\n");
        let (_, timed_out) = message(
            transport
                .request_with_timeout("slow", &json!({}), Duration::from_millis(100))
                .await,
        );
        assert_eq!(timed_out, "slow: timed out waiting for slow response");
        assert_eq!(
            message(
                transport
                    .request_with_timeout("slow", &json!({}), Duration::ZERO)
                    .await
            ),
            (
                EffectCertainty::DefinitelyUnsent,
                "Fixture request timeout must be positive".into()
            )
        );
        // The request registers on its first poll, before the shutdown runs.
        let params = json!({});
        let (outcome, ()) = tokio::join!(transport.request("hang", &params), async {
            sleep(Duration::from_millis(100)).await;
            transport.shutdown();
        });
        assert_eq!(
            message(outcome).1,
            "hang: Fixture transport was deliberately closed"
        );
        assert!(transport.is_closed());
    }
}
