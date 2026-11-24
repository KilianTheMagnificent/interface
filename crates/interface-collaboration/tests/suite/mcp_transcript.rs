//! Byte-level pin for the owner-private collaboration MCP server.
//!
//! `mcp_frames_keep_their_pinned_bytes` drives one scripted session per source
//! engine through the public socket: initialize gating, the tool list and its
//! schemas, orchestrator refusals, every tool's success, refusal and deadline
//! payload, duplicate ids, the in-flight limit, client cancellation, parse and
//! request errors in both framings, and the fatal framing error that ends a
//! connection. It records every byte the server writes, in the order it writes
//! them, plus what the host ingress was asked, and compares the text with
//! `tests/golden/mcp-transcript.txt`. Only frames that race each other by
//! construction (two requests reaching their deadline together) are sorted.
//!
//! On a mismatch the actual text is written to `$CARGO_TARGET_TMPDIR` and the
//! failure names it.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use interface_collaboration::mcp::MCP_PROTOCOL_VERSION;
use interface_collaboration::state::{
    CollaborationEngineId, CollaborationErrorCode, CollaborationErrorV1, CollaborationMode,
    CollaborationReceiptV1, CollaborationState,
};
use interface_collaboration::{
    CollaborationMcpAccessRequest, CollaborationMcpIntent, CollaborationMcpRequestIngress,
    CollaborationMcpSocket, CollaborationMcpSocketOptions, IngressCancellation, McpIngressError,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

const GOLDEN: &str = "mcp-transcript.txt";
/// Short enough to keep the test quick, long enough that an instant host
/// answer never races it.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_IN_FLIGHT: usize = 2;

/// A host ingress whose answer depends only on its input. Requests named
/// `slow` wait for cancellation (the deadline or the client), `refused` fail
/// with a collaboration id, anything else succeeds.
#[derive(Default)]
struct PinIngress {
    calls: Mutex<Vec<String>>,
}

impl PinIngress {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn take_calls(&self) -> Vec<String> {
        let mut calls = std::mem::take(&mut *self.calls.lock().unwrap());
        calls.sort();
        calls
    }

    async fn answer(
        key: &str,
        mode: CollaborationMode,
        target: CollaborationEngineId,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        match key {
            "slow" => {
                cancellation.cancelled().await;
                Err(McpIngressError::new(
                    CollaborationErrorCode::BrokerUnavailable,
                ))
            }
            "refused" => Err(McpIngressError {
                code: CollaborationErrorCode::SourceNotIdle,
                collaboration_id: Some("collab-refused".into()),
                retryable: true,
            }),
            "bad id" => Err(McpIngressError {
                code: CollaborationErrorCode::IdempotencyConflict,
                collaboration_id: Some("line\nbreak".into()),
                retryable: false,
            }),
            _ => Ok(receipt(key, mode, target)),
        }
    }
}

fn receipt(
    key: &str,
    mode: CollaborationMode,
    target: CollaborationEngineId,
) -> CollaborationReceiptV1 {
    CollaborationReceiptV1 {
        version: 1,
        collaboration_id: format!("collab-{key}"),
        revision: 3,
        state: CollaborationState::AwaitingApproval,
        mode,
        source_engine: CollaborationEngineId::Claude,
        target_engine: target,
        approval_required: true,
        cancellation_safe: true,
        created_at: "2026-08-27T12:00:00.000Z".into(),
        updated_at: "2026-08-27T12:00:01.000Z".into(),
        error: (key == "with-error").then(|| CollaborationErrorV1 {
            version: 1,
            code: CollaborationErrorCode::ApprovalRequired,
            collaboration_id: Some("collab-with-error".into()),
            safe_next_action: CollaborationErrorCode::ApprovalRequired
                .safe_next_action()
                .into(),
            retryable: false,
        }),
    }
}

#[async_trait]
impl CollaborationMcpRequestIngress for PinIngress {
    async fn submit(
        &self,
        origin_connection_id: &str,
        intent: CollaborationMcpIntent,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.record(format!("submit {origin_connection_id} {intent:?}"));
        Self::answer(
            &intent.client_request_id,
            intent.mode,
            intent.target_engine,
            cancellation,
        )
        .await
    }

    async fn status(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.record(format!("status {origin_connection_id} {request:?}"));
        Self::answer(
            &request.collaboration_id,
            CollaborationMode::Consult,
            CollaborationEngineId::Codex,
            cancellation,
        )
        .await
    }

    async fn cancel(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.record(format!("cancel {origin_connection_id} {request:?}"));
        Self::answer(
            &request.collaboration_id,
            CollaborationMode::Handoff,
            CollaborationEngineId::Codex,
            cancellation,
        )
        .await
    }
}

struct Session {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    ingress: Arc<PinIngress>,
    text: String,
}

impl Session {
    /// Sends `bytes` in one write and records the next `frames` frames. The
    /// last `unordered` of them race by construction and are sorted.
    async fn step(&mut self, label: &str, bytes: &[u8], frames: usize, unordered: usize) {
        writeln!(self.text, "-> {label}").unwrap();
        self.writer.write_all(bytes).await.unwrap();
        self.writer.flush().await.unwrap();
        let mut received = Vec::with_capacity(frames);
        for _ in 0..frames {
            received.push(self.frame().await.unwrap_or_else(|| "EOF".into()));
        }
        received[frames - unordered..].sort();
        for frame in received {
            writeln!(self.text, "<- {frame}").unwrap();
        }
        // Let a host call the client already cancelled record itself.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        for call in self.ingress.take_calls() {
            writeln!(self.text, "   host {call}").unwrap();
        }
    }

    async fn call(&mut self, label: &str, value: &Value) {
        self.step(label, &line(value), 1, 0).await;
    }

    /// One raw frame exactly as written, `\r` and `\n` spelled out.
    async fn frame(&mut self) -> Option<String> {
        timeout(Duration::from_secs(5), async {
            let mut raw = Vec::new();
            let header = self.reader.fill_buf().await.unwrap();
            if header.is_empty() {
                return None;
            }
            if header.starts_with(b"Content-Length:") {
                let mut length = 0;
                loop {
                    let mut header_line = Vec::new();
                    self.reader
                        .read_until(b'\n', &mut header_line)
                        .await
                        .unwrap();
                    raw.extend_from_slice(&header_line);
                    if header_line == b"\r\n" {
                        break;
                    }
                    let text = std::str::from_utf8(&header_line).unwrap();
                    length = text["Content-Length:".len()..].trim().parse().unwrap();
                }
                let mut body = vec![0; length];
                self.reader.read_exact(&mut body).await.unwrap();
                raw.extend_from_slice(&body);
            } else {
                self.reader.read_until(b'\n', &mut raw).await.unwrap();
            }
            Some(String::from_utf8(raw).unwrap().escape_debug().to_string())
        })
        .await
        .expect("server frame within five seconds")
    }
}

fn line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn lines(values: &[Value]) -> Vec<u8> {
    values.iter().flat_map(line).collect()
}

fn content_length(header: &str, separator: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("{header}: {}{separator}", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn tool_call(id: i64, name: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
}

fn consult(id: i64, target: &str, client_request_id: &str) -> Value {
    tool_call(
        id,
        "iface_consult",
        &json!({
            "targetEngine": target,
            "objective": "  Review the parser.  ",
            "clientRequestId": client_request_id
        }),
    )
}

fn access(id: i64, name: &str, collaboration_id: &str) -> Value {
    tool_call(id, name, &json!({"collaborationId": collaboration_id}))
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "transcript", "version": "1"}
    })
}

#[allow(clippy::too_many_lines)]
async fn transcript(engine: CollaborationEngineId) -> String {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let ingress = Arc::new(PinIngress::default());
    let mut options = CollaborationMcpSocketOptions::frozen(
        directory.path().join("private").join("broker.sock"),
        engine,
        "origin-capability".into(),
        Arc::clone(&ingress) as Arc<dyn CollaborationMcpRequestIngress>,
    );
    options.request_timeout = REQUEST_TIMEOUT;
    options.max_in_flight_per_connection = MAX_IN_FLIGHT;
    let socket = CollaborationMcpSocket::listen(options).await.unwrap();
    let (reader, writer) = UnixStream::connect(socket.socket_path())
        .await
        .unwrap()
        .into_split();
    let mut session = Session {
        reader: BufReader::new(reader),
        writer,
        ingress,
        text: format!("## source {}\n", engine.as_str()),
    };
    let other = if engine == CollaborationEngineId::Codex {
        "claude"
    } else {
        "codex"
    };

    // Initialize gating.
    session
        .call(
            "tools/list before initialize",
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;
    session
        .call(
            "ping without params",
            &json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        )
        .await;
    session
        .call(
            "ping with a string id and _meta",
            &json!({"jsonrpc":"2.0","id":"p-3","method":"ping","params":{"_meta":{}}}),
        )
        .await;
    session
        .call(
            "ping with an unknown param",
            &json!({"jsonrpc":"2.0","id":4,"method":"ping","params":{"x":1}}),
        )
        .await;
    session
        .call(
            "initialize without clientInfo",
            &json!({"jsonrpc":"2.0","id":5,"method":"initialize","params":{
                "protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {}}}),
        )
        .await;
    session
        .call(
            "initialize",
            &json!({"jsonrpc":"2.0","id":6,"method":"initialize","params":initialize_params()}),
        )
        .await;
    session
        .call(
            "initialize again",
            &json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":initialize_params()}),
        )
        .await;
    session
        .call(
            "tools/call before notifications/initialized",
            &access(8, "iface_collaboration_status", "collab-1"),
        )
        .await;
    session
        .step(
            "notifications/initialized then tools/list",
            &lines(&[
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
                json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}}),
            ]),
            1,
            0,
        )
        .await;
    session
        .call(
            "tools/list with a cursor",
            &json!({"jsonrpc":"2.0","id":10,"method":"tools/list","params":{"cursor":"x"}}),
        )
        .await;

    // Tool calls.
    session
        .call("unknown tool", &tool_call(11, "iface_nope", &json!({})))
        .await;
    session
        .call(
            "tools/call without a name",
            &json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"arguments":{}}}),
        )
        .await;
    session
        .call(
            "retired untrusted orchestrator tool",
            &tool_call(13, "iface_workers", &json!({})),
        )
        .await;
    session
        .call(
            "retired orchestrator tool",
            &tool_call(14, "iface_models", &json!({})),
        )
        .await;
    session
        .call(
            "consult targeting this engine",
            &consult(15, engine.as_str(), "self"),
        )
        .await;
    session
        .call("consult", &consult(16, other, "consult-ok"))
        .await;
    session
        .call(
            "consult with a source session key",
            &tool_call(
                17,
                "iface_consult",
                &json!({
                    "sourceSessionKey": "codex#thread-1",
                    "targetEngine": "claude",
                    "objective": "Check the fix.",
                    "clientRequestId": "keyed"
                }),
            ),
        )
        .await;
    session
        .call(
            "handoff without an objective",
            &tool_call(
                18,
                "iface_handoff",
                &json!({"targetEngine": other, "clientRequestId": "handoff-ok"}),
            ),
        )
        .await;
    session
        .call(
            "consult without an objective",
            &tool_call(
                19,
                "iface_consult",
                &json!({"targetEngine": other, "clientRequestId": "no-objective"}),
            ),
        )
        .await;
    session
        .call(
            "consult with an unknown target",
            &consult(20, "acp:qwen", "qwen"),
        )
        .await;
    session
        .call(
            "status",
            &access(21, "iface_collaboration_status", "collab-1"),
        )
        .await;
    session
        .call(
            "status whose receipt carries an error",
            &access(22, "iface_collaboration_status", "with-error"),
        )
        .await;
    session
        .call(
            "cancel",
            &access(23, "iface_collaboration_cancel", "collab-1"),
        )
        .await;
    session
        .call(
            "status refused",
            &access(24, "iface_collaboration_status", "refused"),
        )
        .await;
    session
        .call("consult refused", &consult(25, other, "refused"))
        .await;
    session
        .call(
            "cancel refused with an unsafe collaboration id",
            &access(26, "iface_collaboration_cancel", "bad id"),
        )
        .await;
    session
        .call(
            "status with an extra argument",
            &tool_call(
                27,
                "iface_collaboration_status",
                &json!({"collaborationId": "collab-1", "extra": true}),
            ),
        )
        .await;

    // Deadlines: a call that may have committed never reads as retryable.
    session
        .call("consult past its deadline", &consult(28, other, "slow"))
        .await;
    session
        .call(
            "status past its deadline",
            &access(29, "iface_collaboration_status", "slow"),
        )
        .await;
    session
        .call(
            "cancel past its deadline",
            &access(30, "iface_collaboration_cancel", "slow"),
        )
        .await;

    // Concurrency on one connection.
    session
        .step(
            "duplicate request id while the first is in flight",
            &lines(&[
                consult(31, other, "slow"),
                json!({"jsonrpc":"2.0","id":31,"method":"ping"}),
            ]),
            2,
            0,
        )
        .await;
    session
        .step(
            "third request past the in-flight limit",
            &lines(&[
                consult(32, other, "slow"),
                access(33, "iface_collaboration_status", "slow"),
                json!({"jsonrpc":"2.0","id":34,"method":"ping"}),
            ]),
            3,
            2,
        )
        .await;
    session
        .step(
            "client cancellation answers nothing",
            &lines(&[
                consult(35, other, "slow"),
                json!({"jsonrpc":"2.0","method":"notifications/cancelled",
                    "params":{"requestId":35}}),
                json!({"jsonrpc":"2.0","id":36,"method":"ping"}),
            ]),
            1,
            0,
        )
        .await;

    // Parse and request errors, newline framing.
    session.step("invalid JSON", b"not json\n", 1, 0).await;
    session.step("a non-object", b"[1]\n", 1, 0).await;
    session
        .call(
            "wrong jsonrpc version",
            &json!({"jsonrpc":"1.0","id":40,"method":"ping"}),
        )
        .await;
    session
        .call(
            "invalid id",
            &json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        )
        .await;
    session
        .step(
            "unknown notification then unknown method",
            &lines(&[
                json!({"jsonrpc":"2.0","method":"nope"}),
                json!({"jsonrpc":"2.0","id":41,"method":"nope"}),
            ]),
            1,
            0,
        )
        .await;
    let mut blank_prefixed = b"\n\r\n".to_vec();
    blank_prefixed.extend(line(&json!({"jsonrpc":"2.0","id":42,"method":"ping"})));
    session
        .step("blank lines before a frame", &blank_prefixed, 1, 0)
        .await;

    // Content-Length framing.
    session
        .step(
            "Content-Length ping",
            &content_length(
                "Content-Length",
                "\r\n\r\n",
                &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":50,"method":"ping"})).unwrap(),
            ),
            1,
            0,
        )
        .await;
    session
        .step(
            "lowercase header with LF separators",
            &content_length(
                "content-length",
                "\n\n",
                &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":51,"method":"tools/list"}))
                    .unwrap(),
            ),
            1,
            0,
        )
        .await;
    session
        .step(
            "Content-Length invalid JSON",
            &content_length("Content-Length", "\r\n\r\n", b"not json"),
            1,
            0,
        )
        .await;
    session
        .step(
            "non-numeric Content-Length ends the connection",
            b"Content-Length: abc\r\n\r\n",
            2,
            0,
        )
        .await;

    drop(session.writer);
    socket.stop().await.unwrap();
    session.text
}

#[tokio::test]
async fn mcp_frames_keep_their_pinned_bytes() {
    let mut text = String::new();
    for engine in [
        CollaborationEngineId::Claude,
        CollaborationEngineId::Codex,
        CollaborationEngineId::AcpGrok,
    ] {
        text.push_str(&transcript(engine).await);
    }
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(GOLDEN);
    let expected = std::fs::read_to_string(&golden).unwrap_or_default();
    if expected != text {
        let written = Path::new(env!("CARGO_TARGET_TMPDIR")).join(GOLDEN);
        std::fs::write(&written, &text).expect("write actual");
        panic!(
            "{GOLDEN} changed; the actual text is in {}",
            written.display()
        );
    }
}
