use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::outcome::{DeliveryOutcome, EngineError, EngineErrorKind};

/// Kept above the largest routine frame (thread listing pages and history
/// replays run well past 64 KiB) so the steady state stops reallocating.
const RETAINED_DECODE_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(i64),
    String(String),
}

impl fmt::Display for RpcId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum FrameError {
    #[error("JSON-RPC frame must be an object")]
    NotAnObject,
    #[error("JSON-RPC frame exceeds the {limit}-byte limit")]
    TooLarge { limit: usize },
    #[error("JSON-RPC frame is not valid JSON")]
    InvalidJson,
}

/// Incremental newline-delimited JSON framing. Blank lines are ignored.
#[derive(Debug)]
pub struct NdjsonDecoder {
    buffer: Vec<u8>,
    max_frame_bytes: usize,
    discarding_oversize: bool,
}

impl NdjsonDecoder {
    /// Create a decoder with a strict per-line byte bound.
    ///
    /// # Panics
    ///
    /// Panics when `max_frame_bytes` is zero.
    #[must_use]
    pub fn new(max_frame_bytes: usize) -> Self {
        assert!(max_frame_bytes > 0, "JSON-RPC frame limit must be positive");
        Self {
            buffer: Vec::new(),
            max_frame_bytes,
            discarding_oversize: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<Result<Value, FrameError>> {
        let mut decoded = Vec::new();
        self.push_into(chunk, &mut decoded);
        decoded
    }

    /// Decode complete lines from `chunk`, appending per-line results to `decoded`.
    ///
    /// Returns how many bytes left the line buffer in the process: the lines
    /// appended, with their newlines, plus any blank or discarded bytes. A
    /// caller charges the appended batch to a byte budget with it.
    pub fn push_into(
        &mut self,
        chunk: &[u8],
        decoded: &mut Vec<Result<Value, FrameError>>,
    ) -> usize {
        let held = self.buffer.len();
        let mut start = 0;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            self.push_segment(&chunk[start..index], true, decoded);
            start = index + 1;
        }
        if start < chunk.len() {
            self.push_segment(&chunk[start..], false, decoded);
        }
        held.saturating_add(chunk.len())
            .saturating_sub(self.buffer.len())
    }

    /// Decode a final unterminated line when the peer closes stdout.
    pub fn finish(&mut self) -> Vec<Result<Value, FrameError>> {
        let mut decoded = Vec::new();
        self.finish_into(&mut decoded);
        decoded
    }

    /// Decode a final unterminated line, appending it to `decoded` when present.
    /// Returns the bytes that left the line buffer, as [`Self::push_into`] does.
    pub fn finish_into(&mut self, decoded: &mut Vec<Result<Value, FrameError>>) -> usize {
        let held = self.buffer.len();
        if self.discarding_oversize {
            self.discarding_oversize = false;
            self.clear_buffer();
            return held;
        }
        if self.buffer.is_empty() {
            return 0;
        }
        if let Some(value) = decode_line(&self.buffer) {
            decoded.push(value);
        }
        self.clear_buffer();
        held
    }

    fn push_segment(
        &mut self,
        segment: &[u8],
        terminated: bool,
        decoded: &mut Vec<Result<Value, FrameError>>,
    ) {
        if self.discarding_oversize {
            if terminated {
                self.discarding_oversize = false;
            }
            return;
        }
        if self.buffer.len().saturating_add(segment.len()) > self.max_frame_bytes {
            self.clear_buffer();
            self.discarding_oversize = !terminated;
            decoded.push(Err(FrameError::TooLarge {
                limit: self.max_frame_bytes,
            }));
            return;
        }
        // Bound already enforced: a whole line in `segment` must not skip it.
        if terminated && self.buffer.is_empty() {
            if let Some(value) = decode_line(segment) {
                decoded.push(value);
            }
            return;
        }
        self.buffer.extend_from_slice(segment);
        if terminated {
            if let Some(value) = decode_line(&self.buffer) {
                decoded.push(value);
            }
            self.clear_buffer();
        }
    }

    fn clear_buffer(&mut self) {
        self.buffer.clear();
        if self.buffer.capacity() > RETAINED_DECODE_BUFFER_BYTES {
            self.buffer.shrink_to(RETAINED_DECODE_BUFFER_BYTES);
        }
    }
}

fn decode_line(line: &[u8]) -> Option<Result<Value, FrameError>> {
    let first = line.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let last = line
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .expect("a non-whitespace byte exists");
    Some(serde_json::from_slice(&line[first..=last]).map_err(|_| FrameError::InvalidJson))
}

/// Serialize one JSON object followed by exactly one newline.
///
/// # Errors
///
/// Returns an error for non-object values, serialization failure, or an oversized frame.
pub fn encode_frame(value: &Value, max_frame_bytes: usize) -> Result<Vec<u8>, FrameError> {
    if !value.is_object() {
        return Err(FrameError::NotAnObject);
    }
    let mut encoded = serde_json::to_vec(value).map_err(|_| FrameError::InvalidJson)?;
    if encoded.len() > max_frame_bytes {
        return Err(FrameError::TooLarge {
            limit: max_frame_bytes,
        });
    }
    encoded.push(b'\n');
    Ok(encoded)
}

/// Whether `text` can travel as one JSON string in a frame bounded by
/// `max_frame_bytes`. A frame holds the escaped string plus its envelope, so
/// text whose escaped form alone reaches the bound can never be framed. An
/// adapter refuses such a prompt before any provider effect instead of after
/// one, as a retryable framing error would.
#[must_use]
pub fn text_fits_frame(text: &str, max_frame_bytes: usize) -> bool {
    /// Counts escaped bytes and stops as soon as the bound is reached.
    struct Bounded {
        written: usize,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.written = self.written.saturating_add(bytes.len());
            if self.written >= self.limit {
                return Err(std::io::Error::other("frame bound reached"));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(
        Bounded {
            written: 0,
            limit: max_frame_bytes,
        },
        text,
    )
    .is_ok()
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcNotification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcServerRequest {
    pub id: RpcId,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RpcRemoteError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcFailure {
    pub method: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RpcCallOutcome {
    Response(Value),
    Rejected(RpcRemoteError),
    DefinitelyUnsent(RpcFailure),
    OutcomeUnknown(RpcFailure),
}

impl RpcCallOutcome {
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(self, Self::DefinitelyUnsent(_))
    }
}

pub struct PendingCall {
    id: RpcId,
    receiver: oneshot::Receiver<RpcCallOutcome>,
}

impl PendingCall {
    #[must_use]
    pub fn id(&self) -> &RpcId {
        &self.id
    }

    pub async fn outcome(self) -> RpcCallOutcome {
        self.receiver.await.unwrap_or_else(|_| {
            RpcCallOutcome::OutcomeUnknown(RpcFailure {
                method: "unknown".into(),
                message: "JSON-RPC routing state ended without settling the request".into(),
            })
        })
    }
}

pub struct PreparedRequest {
    pub id: RpcId,
    pub frame: Vec<u8>,
    pub completion: PendingCall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchState {
    Prepared,
    WriteStarted,
    FrameAccepted,
}

struct PendingEntry {
    method: String,
    dispatch: DispatchState,
    sender: oneshot::Sender<RpcCallOutcome>,
}

struct RouterState {
    next_id: i64,
    max_pending: usize,
    max_frame_bytes: usize,
    closed: bool,
    pending: HashMap<RpcId, PendingEntry>,
}

#[derive(Clone)]
pub struct PendingRouter {
    shared: Arc<Mutex<RouterState>>,
}

fn lock(state: &Mutex<RouterState>) -> MutexGuard<'_, RouterState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum RouterError {
    #[error("JSON-RPC router is closed")]
    Closed,
    #[error("JSON-RPC method must be bounded non-empty text")]
    InvalidMethod,
    #[error("JSON-RPC pending-request limit of {limit} was reached")]
    PendingLimit { limit: usize },
    #[error("JSON-RPC request ids are exhausted")]
    IdExhausted,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcDiagnostic {
    UnknownResponseId(RpcId),
    IdlessErrorWithMultipleRequests { pending: usize },
    UnroutableMessage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RouteEvent {
    Notification(RpcNotification),
    ServerRequest(RpcServerRequest),
    ResponseSettled { id: RpcId },
    Diagnostic(RpcDiagnostic),
}

impl PendingRouter {
    /// Create a bounded request router.
    ///
    /// # Panics
    ///
    /// Panics when either bound is zero.
    #[must_use]
    pub fn new(max_pending: usize, max_frame_bytes: usize) -> Self {
        assert!(max_pending > 0, "pending request limit must be positive");
        assert!(max_frame_bytes > 0, "JSON-RPC frame limit must be positive");
        Self {
            shared: Arc::new(Mutex::new(RouterState {
                next_id: 1,
                max_pending,
                max_frame_bytes,
                closed: false,
                pending: HashMap::new(),
            })),
        }
    }

    /// Register a request before any attempt to write its returned frame.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid methods, closed/capacity state, exhausted ids,
    /// or a frame exceeding the configured byte limit.
    pub fn prepare_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<PreparedRequest, RouterError> {
        if method.is_empty()
            || method.len() > 256
            || method.trim() != method
            || method.chars().any(char::is_control)
        {
            return Err(RouterError::InvalidMethod);
        }
        let mut state = lock(&self.shared);
        if state.closed {
            return Err(RouterError::Closed);
        }
        if state.pending.len() == state.max_pending {
            return Err(RouterError::PendingLimit {
                limit: state.max_pending,
            });
        }
        let number = state.next_id;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or(RouterError::IdExhausted)?;
        let id = RpcId::Number(number);
        let mut object = Map::new();
        object.insert("jsonrpc".into(), Value::String("2.0".into()));
        object.insert("id".into(), rpc_id_value(&id));
        object.insert("method".into(), Value::String(method.into()));
        if let Some(params) = params {
            object.insert("params".into(), params);
        }
        let frame = encode_frame(&Value::Object(object), state.max_frame_bytes)?;
        let (sender, receiver) = oneshot::channel();
        state.pending.insert(
            id.clone(),
            PendingEntry {
                method: method.into(),
                dispatch: DispatchState::Prepared,
                sender,
            },
        );
        drop(state);
        Ok(PreparedRequest {
            id: id.clone(),
            frame,
            completion: PendingCall { id, receiver },
        })
    }

    /// Call immediately before invoking the transport writer.
    #[must_use]
    pub fn mark_write_started(&self, id: &RpcId) -> bool {
        self.transition_dispatch(id, DispatchState::Prepared, DispatchState::WriteStarted)
    }

    /// Call after the writer accepted the full frame.
    #[must_use]
    pub fn mark_frame_accepted(&self, id: &RpcId) -> bool {
        self.transition_dispatch(
            id,
            DispatchState::WriteStarted,
            DispatchState::FrameAccepted,
        )
    }

    /// A synchronous writer rejection proves that no bytes were accepted.
    ///
    /// The acceptance check and the removal share one lock acquisition so a
    /// concurrent [`Self::mark_frame_accepted`] can never be overtaken.
    pub fn fail_before_acceptance(&self, id: &RpcId, message: impl Into<String>) -> bool {
        let mut state = lock(&self.shared);
        let Some(entry) = state.pending.remove(id) else {
            return false;
        };
        if entry.dispatch == DispatchState::FrameAccepted {
            state.pending.insert(id.clone(), entry);
            return false;
        }
        drop(state);
        let _ = entry
            .sender
            .send(RpcCallOutcome::DefinitelyUnsent(RpcFailure {
                method: entry.method,
                message: message.into(),
            }));
        true
    }

    /// Expiry is unknown once a write began; prepared calls remain definitely unsent.
    pub fn expire(&self, id: &RpcId, message: impl Into<String>) -> bool {
        let message = message.into();
        self.remove_and_send(id, |method, dispatch| {
            failure_for_dispatch(method, dispatch, message)
        })
    }

    /// Settle every in-flight call when no more responses can arrive.
    pub fn transport_failed(&self, message: impl Into<String>) {
        let message = message.into();
        let entries = {
            let mut state = lock(&self.shared);
            state.closed = true;
            state
                .pending
                .drain()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        };
        for entry in entries {
            let PendingEntry {
                method,
                dispatch,
                sender,
            } = entry;
            let outcome = failure_for_dispatch(method, dispatch, message.clone());
            let _ = sender.send(outcome);
        }
    }

    /// Route one parsed object. Inbound `jsonrpc` is intentionally optional.
    pub fn route(&self, value: Value) -> RouteEvent {
        let Value::Object(mut object) = value else {
            return RouteEvent::Diagnostic(RpcDiagnostic::UnroutableMessage);
        };
        let id = object.get("id").and_then(read_rpc_id);
        if let Some(Value::String(method)) = object.remove("method") {
            let params = object.remove("params");
            return match id {
                Some(id) => RouteEvent::ServerRequest(RpcServerRequest { id, method, params }),
                None => RouteEvent::Notification(RpcNotification { method, params }),
            };
        }
        self.route_response(object, id)
    }

    fn route_response(&self, mut object: Map<String, Value>, id: Option<RpcId>) -> RouteEvent {
        let has_error = object.get("error").is_some_and(|error| !error.is_null());
        let Some(id) = id else {
            if has_error {
                let mut state = lock(&self.shared);
                if state.pending.len() == 1 {
                    let only_id = state
                        .pending
                        .keys()
                        .next()
                        .cloned()
                        .expect("one pending id");
                    let entry = state.pending.remove(&only_id).expect("pending id remains");
                    drop(state);
                    let _ = entry.sender.send(RpcCallOutcome::Rejected(remote_error(
                        object.remove("error"),
                        &entry.method,
                    )));
                    return RouteEvent::ResponseSettled { id: only_id };
                }
                return RouteEvent::Diagnostic(RpcDiagnostic::IdlessErrorWithMultipleRequests {
                    pending: state.pending.len(),
                });
            }
            return RouteEvent::Diagnostic(RpcDiagnostic::UnroutableMessage);
        };
        let entry = lock(&self.shared).pending.remove(&id);
        let Some(entry) = entry else {
            return RouteEvent::Diagnostic(RpcDiagnostic::UnknownResponseId(id));
        };
        let outcome = if has_error {
            RpcCallOutcome::Rejected(remote_error(object.remove("error"), &entry.method))
        } else if let Some(result) = object.remove("result") {
            RpcCallOutcome::Response(result)
        } else {
            RpcCallOutcome::OutcomeUnknown(RpcFailure {
                method: entry.method.clone(),
                message: "response contained neither result nor error".into(),
            })
        };
        let _ = entry.sender.send(outcome);
        RouteEvent::ResponseSettled { id }
    }

    fn transition_dispatch(
        &self,
        id: &RpcId,
        expected: DispatchState,
        next: DispatchState,
    ) -> bool {
        let mut state = lock(&self.shared);
        let Some(entry) = state.pending.get_mut(id) else {
            return false;
        };
        if entry.dispatch != expected {
            return false;
        }
        entry.dispatch = next;
        true
    }

    fn remove_and_send(
        &self,
        id: &RpcId,
        make_outcome: impl FnOnce(String, DispatchState) -> RpcCallOutcome,
    ) -> bool {
        let entry = lock(&self.shared).pending.remove(id);
        let Some(entry) = entry else {
            return false;
        };
        let outcome = make_outcome(entry.method, entry.dispatch);
        let _ = entry.sender.send(outcome);
        true
    }
}

fn failure_for_dispatch(
    method: String,
    dispatch: DispatchState,
    message: String,
) -> RpcCallOutcome {
    let failure = RpcFailure { method, message };
    match dispatch {
        DispatchState::Prepared => RpcCallOutcome::DefinitelyUnsent(failure),
        DispatchState::WriteStarted | DispatchState::FrameAccepted => {
            RpcCallOutcome::OutcomeUnknown(failure)
        }
    }
}

fn remote_error(value: Option<Value>, method: &str) -> RpcRemoteError {
    let Some(Value::Object(mut object)) = value else {
        return RpcRemoteError {
            code: -32_603,
            message: format!("{method}: provider rejected the request"),
            data: None,
        };
    };
    // Taken first so the message can be read by reference below.
    let data = object.remove("data");
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or(-32_603);
    let provider_message = object
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider rejected the request");
    RpcRemoteError {
        code,
        message: format!("{method}: {provider_message}"),
        data,
    }
}

fn read_rpc_id(value: &Value) -> Option<RpcId> {
    match value {
        Value::Number(number) => number.as_i64().map(RpcId::Number),
        Value::String(value) => Some(RpcId::String(value.clone())),
        _ => None,
    }
}

fn rpc_id_value(id: &RpcId) -> Value {
    match id {
        RpcId::Number(value) => Value::Number((*value).into()),
        RpcId::String(value) => Value::String(value.clone()),
    }
}

/// Build an outbound notification frame.
///
/// # Errors
///
/// Returns a framing error when serialization or the configured bound fails.
pub fn notification_frame(
    method: &str,
    params: Option<Value>,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), Value::String("2.0".into()));
    object.insert("method".into(), Value::String(method.into()));
    if let Some(params) = params {
        object.insert("params".into(), params);
    }
    encode_frame(&Value::Object(object), max_frame_bytes)
}

/// Build exactly one successful answer to a server-to-client request.
///
/// # Errors
///
/// Returns a framing error when serialization or the configured bound fails.
pub fn server_result_frame(
    id: &RpcId,
    result: Value,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), Value::String("2.0".into()));
    object.insert("id".into(), rpc_id_value(id));
    object.insert("result".into(), result);
    encode_frame(&Value::Object(object), max_frame_bytes)
}

/// Build exactly one error answer to a server-to-client request.
///
/// # Errors
///
/// Returns a framing error when serialization or the configured bound fails.
pub fn server_error_frame(
    id: &RpcId,
    error: &RpcRemoteError,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), Value::String("2.0".into()));
    object.insert("id".into(), rpc_id_value(id));
    object.insert(
        "error".into(),
        serde_json::to_value(error).map_err(|_| FrameError::InvalidJson)?,
    );
    encode_frame(&Value::Object(object), max_frame_bytes)
}

/// Translate wire outcomes without collapsing ambiguous delivery into failure.
/// Shared verbatim by every JSON-RPC engine adapter.
#[must_use]
pub fn delivery_from_rpc(outcome: RpcCallOutcome) -> DeliveryOutcome<Value> {
    match outcome {
        RpcCallOutcome::Response(value) => DeliveryOutcome::Confirmed(value),
        RpcCallOutcome::Rejected(error) => DeliveryOutcome::DefinitelyRejected(
            EngineError::definitely_rejected(EngineErrorKind::Provider, error.message, false),
        ),
        RpcCallOutcome::DefinitelyUnsent(failure) => {
            DeliveryOutcome::DefinitelyUnsent(rpc_failure_error(&failure, false))
        }
        RpcCallOutcome::OutcomeUnknown(failure) => {
            DeliveryOutcome::OutcomeUnknown(rpc_failure_error(&failure, true))
        }
    }
}

fn rpc_failure_error(failure: &RpcFailure, unknown: bool) -> EngineError {
    let message = format!("{}: {}", failure.method, failure.message);
    if unknown {
        EngineError::outcome_unknown(EngineErrorKind::Transport, message, true)
    } else {
        EngineError::definitely_unsent(EngineErrorKind::Transport, message, true)
    }
}

/// A request refused before any frame reached the writer: always replay-safe.
#[must_use]
pub fn prepare_error(method: &str, error: &RouterError) -> DeliveryOutcome<Value> {
    let kind = if matches!(error, RouterError::PendingLimit { .. }) {
        EngineErrorKind::Capacity
    } else {
        EngineErrorKind::Protocol
    };
    DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
        kind,
        format!("could not prepare {method}: {error}"),
        true,
    ))
}

/// A frame that could not be encoded, so nothing was written.
#[must_use]
pub fn frame_error(message: String) -> DeliveryOutcome<()> {
    DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
        EngineErrorKind::Protocol,
        message,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decoder_accepts_split_frames_blank_lines_and_unterminated_tail() {
        let mut decoder = NdjsonDecoder::new(1_024);
        assert!(decoder.push(br#"  {"id":1,"res"#).is_empty());
        assert_eq!(
            decoder.push(b"ult\":{}}\r\n\n"),
            vec![Ok(json!({"id": 1, "result": {}}))]
        );
        assert!(decoder.push(br#"{"method":"done"}"#).is_empty());
        assert_eq!(decoder.finish(), vec![Ok(json!({"method": "done"}))]);
    }

    #[test]
    fn push_into_and_finish_into_append_without_clearing_the_caller_buffer() {
        let mut decoder = NdjsonDecoder::new(1_024);
        let mut frames = vec![Ok(json!({"pre": true}))];
        decoder.push_into(b"{\"id\":1}\n{\"id\":2}\n", &mut frames);
        assert_eq!(
            frames,
            vec![
                Ok(json!({"pre": true})),
                Ok(json!({"id": 1})),
                Ok(json!({"id": 2})),
            ]
        );
        decoder.push_into(b"{\"id\":3}", &mut frames);
        decoder.finish_into(&mut frames);
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[3], Ok(json!({"id": 3})));
        // Nothing is left over, so a second finish appends nothing.
        decoder.finish_into(&mut frames);
        assert_eq!(frames.len(), 4);
    }

    #[test]
    fn push_into_reports_the_bytes_that_left_the_line_buffer() {
        let mut decoder = NdjsonDecoder::new(8);
        let mut frames = Vec::new();
        // A split line is held until its newline arrives, then counted whole.
        assert_eq!(decoder.push_into(b"{\"a\":", &mut frames), 0);
        assert_eq!(decoder.push_into(b"1}\n\n{}\n{", &mut frames), 12);
        assert_eq!(frames, vec![Ok(json!({"a": 1})), Ok(json!({}))]);
        assert_eq!(decoder.push_into(b"}\n", &mut frames), 3);
        // An oversize line leaves the buffer too.
        assert_eq!(decoder.push_into(b"123456789\n{", &mut frames), 10);
        assert_eq!(decoder.finish_into(&mut frames), 1);
        assert_eq!(decoder.finish_into(&mut frames), 0);
        assert_eq!(
            frames[2..],
            [
                Ok(json!({})),
                Err(FrameError::TooLarge { limit: 8 }),
                Err(FrameError::InvalidJson)
            ]
        );
    }

    #[test]
    fn text_fits_a_frame_only_when_its_escaped_form_is_below_the_bound() {
        assert!(text_fits_frame("abc", 6));
        // `"abc"` is five bytes: it reaches a five-byte bound, so no frame fits it.
        assert!(!text_fits_frame("abc", 5));
        // Escaping counts: a newline is two bytes inside a JSON string.
        assert!(!text_fits_frame("\n\n", 6));
        assert!(text_fits_frame("\n\n", 7));
        // Necessary, not sufficient: the envelope can still push fitting text over.
        let prompt = "x".repeat(1_000);
        let frame = serde_json::json!({"params": {"text": prompt}});
        assert!(text_fits_frame(&prompt, 1_010));
        assert!(encode_frame(&frame, 1_010).is_err());
        assert!(!text_fits_frame(&prompt, 1_002));
    }

    #[test]
    fn oversized_line_is_dropped_and_the_next_frame_survives() {
        let mut decoder = NdjsonDecoder::new(8);
        let frames = decoder.push(b"{\"too\":\"long\"}\n{}\n");
        assert_eq!(
            frames,
            vec![Err(FrameError::TooLarge { limit: 8 }), Ok(json!({}))]
        );
    }

    #[test]
    fn complete_lines_in_one_chunk_still_honor_the_byte_bound() {
        let mut decoder = NdjsonDecoder::new(8);
        assert_eq!(
            decoder.push(b"123456789\n{}\n{\"a\":1}\n"),
            vec![
                Err(FrameError::TooLarge { limit: 8 }),
                Ok(json!({})),
                Ok(json!({"a": 1})),
            ]
        );
        let mut decoder = NdjsonDecoder::new(1_024);
        assert_eq!(
            decoder.push(b"{\"a\":1}\n{\"b\":2}\n{\"c\":"),
            vec![Ok(json!({"a": 1})), Ok(json!({"b": 2}))]
        );
        assert_eq!(decoder.push(b"3}\n"), vec![Ok(json!({"c": 3}))]);
    }

    #[tokio::test]
    async fn responses_route_by_id_even_without_jsonrpc_member() {
        let router = PendingRouter::new(4, 1_024);
        let request = router
            .prepare_request("thread/start", Some(json!({"cwd": "/tmp"})))
            .unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.mark_frame_accepted(&request.id));
        assert_eq!(
            router.route(json!({"id": 1, "result": {"thread": {"id": "t-1"}}})),
            RouteEvent::ResponseSettled {
                id: RpcId::Number(1)
            }
        );
        assert_eq!(
            request.completion.outcome().await,
            RpcCallOutcome::Response(json!({"thread": {"id": "t-1"}}))
        );
    }

    #[tokio::test]
    async fn transport_failure_preserves_unsent_vs_unknown_outcome() {
        let router = PendingRouter::new(4, 1_024);
        let unsent = router.prepare_request("turn/start", None).unwrap();
        let unknown = router.prepare_request("turn/steer", None).unwrap();
        assert!(router.mark_write_started(&unknown.id));
        assert!(router.mark_frame_accepted(&unknown.id));
        router.transport_failed("fake server closed");
        assert!(matches!(
            unsent.completion.outcome().await,
            RpcCallOutcome::DefinitelyUnsent(_)
        ));
        assert!(matches!(
            unknown.completion.outcome().await,
            RpcCallOutcome::OutcomeUnknown(_)
        ));
    }

    #[tokio::test]
    async fn synchronous_writer_rejection_is_definitely_unsent() {
        let router = PendingRouter::new(2, 1_024);
        let request = router.prepare_request("turn/start", None).unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.fail_before_acceptance(&request.id, "fake writer refused"));
        assert!(matches!(
            request.completion.outcome().await,
            RpcCallOutcome::DefinitelyUnsent(_)
        ));
    }

    #[tokio::test]
    async fn accepted_frame_cannot_be_reclassified_as_definitely_unsent() {
        let router = PendingRouter::new(2, 1_024);
        let request = router.prepare_request("turn/start", None).unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.mark_frame_accepted(&request.id));
        assert!(!router.fail_before_acceptance(&request.id, "late writer error"));
        router.transport_failed("fake server closed before a response");
        assert!(matches!(
            request.completion.outcome().await,
            RpcCallOutcome::OutcomeUnknown(_)
        ));
    }

    #[tokio::test]
    async fn concurrent_acceptance_and_writer_failure_settle_exactly_one_way() {
        const REQUESTS: usize = 20_000;
        let router = PendingRouter::new(REQUESTS, 1_024);
        let mut requests = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let request = router.prepare_request("turn/start", None).unwrap();
            assert!(router.mark_write_started(&request.id));
            requests.push(request);
        }
        let ids = requests
            .iter()
            .map(|request| request.id.clone())
            .collect::<Vec<_>>();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let accepting = {
            let router = router.clone();
            let ids = ids.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                ids.iter()
                    .map(|id| {
                        barrier.wait();
                        router.mark_frame_accepted(id)
                    })
                    .collect::<Vec<_>>()
            })
        };
        let failing = {
            let router = router.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                ids.iter()
                    .map(|id| {
                        barrier.wait();
                        router.fail_before_acceptance(id, "fake writer refused")
                    })
                    .collect::<Vec<_>>()
            })
        };
        let accepted = accepting.join().unwrap();
        let unsent = failing.join().unwrap();
        router.transport_failed("fake server closed before a response");
        for (index, request) in requests.into_iter().enumerate() {
            assert!(
                accepted[index] != unsent[index],
                "request {index}: acceptance and writer rejection both claimed the request"
            );
            let outcome = request.completion.outcome().await;
            if accepted[index] {
                assert!(matches!(outcome, RpcCallOutcome::OutcomeUnknown(_)));
            } else {
                assert!(matches!(outcome, RpcCallOutcome::DefinitelyUnsent(_)));
            }
        }
    }

    #[tokio::test]
    async fn idless_error_settles_the_only_possible_request() {
        let router = PendingRouter::new(4, 1_024);
        let request = router.prepare_request("no/such/method", None).unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.mark_frame_accepted(&request.id));
        assert_eq!(
            router.route(json!({"error": {"code": -32600, "message": "unknown method"}})),
            RouteEvent::ResponseSettled {
                id: RpcId::Number(1)
            }
        );
        assert!(matches!(
            request.completion.outcome().await,
            RpcCallOutcome::Rejected(RpcRemoteError { code: -32600, .. })
        ));
    }

    #[tokio::test]
    async fn response_payloads_move_out_of_the_owned_object() {
        let router = PendingRouter::new(4, 64 * 1024);
        let request = router.prepare_request("thread/read", None).unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.mark_frame_accepted(&request.id));
        let payload = json!({"turns": [{"items": [{"text": "hello"}, {"text": "world"}]}]});
        assert_eq!(
            router.route(json!({"id": 1, "result": payload.clone()})),
            RouteEvent::ResponseSettled {
                id: RpcId::Number(1)
            }
        );
        assert_eq!(
            request.completion.outcome().await,
            RpcCallOutcome::Response(payload)
        );

        let rejected = router.prepare_request("thread/read", None).unwrap();
        assert!(router.mark_write_started(&rejected.id));
        assert!(router.mark_frame_accepted(&rejected.id));
        assert_eq!(
            router.route(json!({
                "id": 2,
                "error": {"code": -32000, "message": "nope", "data": {"hint": 1}}
            })),
            RouteEvent::ResponseSettled {
                id: RpcId::Number(2)
            }
        );
        assert_eq!(
            rejected.completion.outcome().await,
            RpcCallOutcome::Rejected(RpcRemoteError {
                code: -32_000,
                message: "thread/read: nope".into(),
                data: Some(json!({"hint": 1})),
            })
        );
    }

    #[test]
    fn notifications_take_the_method_string_from_the_object() {
        let router = PendingRouter::new(4, 1_024);
        assert_eq!(
            router.route(json!({
                "method": "item/agentMessage/delta",
                "params": {"delta": "x"}
            })),
            RouteEvent::Notification(RpcNotification {
                method: "item/agentMessage/delta".into(),
                params: Some(json!({"delta": "x"})),
            })
        );
    }

    #[test]
    fn server_answers_are_newline_delimited_jsonrpc_objects() {
        let encoded =
            server_result_frame(&RpcId::String("approval-1".into()), Value::Null, 512).unwrap();
        assert_eq!(encoded.last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded[..encoded.len() - 1]).unwrap(),
            json!({"jsonrpc": "2.0", "id": "approval-1", "result": null})
        );
    }
}
