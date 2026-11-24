//! The owner-only Unix-socket MCP transport the voice assistant and the pane
//! tools share: socket binding, newline-delimited JSON-RPC framing, and the
//! three methods a CLI needs — initialize, tools/list, tools/call. What the
//! tools do is the backend's business; which pane a connection belongs to is
//! answered from the kernel's peer pid, never from model input.

use std::{
    collections::HashMap,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use engine_claude::McpServer;
use futures::StreamExt;
use interface_common::sync::lock;
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{Mutex as AsyncMutex, Semaphore},
};
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    sync::CancellationToken,
};

pub(crate) use interface_collaboration::mcp::MCP_PROTOCOL_VERSION;
pub(crate) const NC: &str = "/usr/bin/nc";
pub(crate) const MAX_UNIX_SOCKET_PATH_BYTES: usize = 103;
/// One JSON-RPC line; the same 1 MiB cap the other host sockets use.
const MAX_REQUEST_LINE_BYTES: usize = 1 << 20;
/// Requests one connection may have running at once. Reading pauses while
/// the connection is at the limit, so a misbehaving client cannot fan out
/// unbounded tasks. Generous because `worker_collect` may wait 30 s.
const MAX_IN_FLIGHT_REQUESTS: usize = 16;
/// Pause after a failed accept before the next one.
const ACCEPT_RETRY_BACKOFF: Duration = Duration::from_millis(50);

pub(crate) enum ToolOutput {
    Text(String),
    Image { png_base64: String, text: String },
}

/// What the kernel says about the connecting process. `pid` is `None` where
/// the platform cannot report it; a backend that needs it then refuses.
#[derive(Clone, Debug, Default)]
pub(crate) struct Peer {
    pub(crate) pid: Option<u32>,
    /// When that process started, read as it connected, for a backend that
    /// must tell it from a later process that reused its pid
    /// ([`ToolBackend::admit`] fills it); `None` otherwise.
    pub(crate) start: Option<Arc<str>>,
}

#[async_trait]
pub(crate) trait ToolBackend: Send + Sync + 'static {
    /// Called once per accepted connection, before any of its requests is
    /// read: the peer every call on it carries, or `None` to close it
    /// unanswered. By default the kernel's peer, as it is.
    async fn admit(&self, peer: Peer) -> Option<Peer> {
        Some(peer)
    }

    async fn call(&self, name: &str, input: Value, peer: Peer) -> Result<ToolOutput, String>;

    /// The same call, with `gone`: cancelled once nobody can take its
    /// answer (the connection closed, the server stopped, or the client
    /// sent `notifications/cancelled` for it). Only a call that waits on the
    /// owner needs it; by default it is ignored and the call runs to its end
    /// as it always has.
    async fn call_until_gone(
        &self,
        name: &str,
        input: Value,
        peer: Peer,
        gone: CancellationToken,
    ) -> Result<ToolOutput, String> {
        let _ = gone;
        self.call(name, input, peer).await
    }
}

/// One server's identity and tool table, shared by every connection to it.
pub(crate) struct McpServerSpec {
    pub(crate) wire_name: &'static str,
    pub(crate) title: &'static str,
    pub(crate) instructions: &'static str,
    /// The `tools/list` catalogue.
    pub(crate) tools: Vec<Value>,
    /// Old names remain callable for connected clients without advertising
    /// duplicate tools after they move to a dedicated server.
    pub(crate) compatibility_tools: &'static [&'static str],
    /// The longest request line this server reads; `None` is
    /// [`MAX_REQUEST_LINE_BYTES`]. A longer line closes the connection.
    pub(crate) max_request_line_bytes: Option<usize>,
}

impl McpServerSpec {
    fn allows(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool["name"] == name)
            || self.compatibility_tools.contains(&name)
    }
}

/// The stdio bridge descriptor a CLI launches to reach `path`.
pub(crate) fn descriptor(path: &Path) -> McpServer {
    McpServer {
        command: PathBuf::from(NC),
        args: vec!["-U".into(), path.to_string_lossy().into_owned()],
    }
}

/// A socket this process bound, with the (device, inode) that proves it is
/// still the same file when the owner removes it.
pub(crate) struct BoundSocket {
    pub(crate) listener: std::os::unix::net::UnixListener,
    pub(crate) identity: (u64, u64),
}

/// Binds `path` inside `root` as an owner-only socket. The listener is
/// returned bound so a caller can fail loudly before spawning anything.
///
/// The root must be a real directory (never a symlink); it is tightened to
/// 0700 and then verified as owner-private. A leftover socket from an earlier
/// run is removed only when it is a socket owned by the root's owner; any
/// other file at `path` is refused rather than unlinked.
///
/// # Errors
///
/// Returns the bind, permission or unsafe-path error.
pub(crate) fn bind(root: &Path, path: &Path) -> std::io::Result<BoundSocket> {
    std::fs::create_dir_all(root)?;
    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::other(
            "MCP socket directory is not a real directory",
        ));
    }
    // An older build, or another writer using the umask, may have created
    // the directory; chmod succeeds only for its owner.
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    interface_storage::PrivateDirectory::open(root).map_err(std::io::Error::other)?;
    let owner_uid = metadata.uid();
    match std::fs::symlink_metadata(path) {
        Ok(existing) if existing.file_type().is_socket() && existing.uid() == owner_uid => {
            std::fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "MCP socket path holds something other than a stale socket",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    // Re-verified after the chmod, as the orchestration socket is: a real
    // socket, ours, and owner-private, or it is removed and refused.
    let bound = std::fs::symlink_metadata(path)?;
    if !bound.file_type().is_socket() || bound.uid() != owner_uid || bound.mode() & 0o777 != 0o600 {
        let _ = std::fs::remove_file(path);
        return Err(std::io::Error::other(
            "MCP socket did not become owner-private",
        ));
    }
    Ok(BoundSocket {
        listener,
        identity: (bound.dev(), bound.ino()),
    })
}

/// Removes `path` only while it is still the socket [`bind`] created.
pub(crate) fn remove_bound_socket(path: &Path, identity: (u64, u64)) {
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == identity
    }) {
        let _ = std::fs::remove_file(path);
    }
}

/// Spawns the accept loop for a bound listener on the current runtime.
pub(crate) fn serve(
    listener: std::os::unix::net::UnixListener,
    path: &Path,
    spec: Arc<McpServerSpec>,
    backend: Arc<dyn ToolBackend>,
    stop: CancellationToken,
) {
    // The socket is 0600 in a 0700 directory, so only its owner can connect;
    // the uid check below is the second lock on the same door.
    let owner_uid = std::fs::metadata(path).map(|metadata| metadata.uid()).ok();
    tokio::runtime::Handle::current().spawn(accept_loop(listener, owner_uid, spec, backend, stop));
}

async fn accept_loop(
    listener: std::os::unix::net::UnixListener,
    owner_uid: Option<u32>,
    spec: Arc<McpServerSpec>,
    backend: Arc<dyn ToolBackend>,
    stop: CancellationToken,
) {
    let listener = match UnixListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, server = spec.wire_name, "MCP socket could not join the runtime");
            return;
        }
    };
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let peer = match stream.peer_cred() {
                        Ok(credentials) => {
                            if owner_uid.is_some_and(|uid| uid != credentials.uid()) {
                                tracing::warn!(server = spec.wire_name, "dropped an MCP connection from another user");
                                continue;
                            }
                            Peer {
                                pid: credentials.pid().and_then(|pid| u32::try_from(pid).ok()),
                                start: None,
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, server = spec.wire_name, "dropped an MCP connection without peer credentials");
                            continue;
                        }
                    };
                    tokio::spawn(admit_then_serve(
                        stream,
                        peer,
                        Arc::clone(&spec),
                        Arc::clone(&backend),
                        stop.child_token(),
                    ));
                }
                Err(error) => {
                    // A transient failure (EMFILE under descriptor pressure,
                    // ECONNABORTED) must not retire a socket every pane uses;
                    // only the stop token ends this loop.
                    tracing::warn!(%error, server = spec.wire_name, "MCP socket accept failed");
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(ACCEPT_RETRY_BACKOFF) => {}
                    }
                }
            },
        }
    }
}

/// Serves a connection once its backend admits it ([`ToolBackend::admit`]);
/// one it refuses is closed unanswered.
async fn admit_then_serve(
    stream: UnixStream,
    peer: Peer,
    spec: Arc<McpServerSpec>,
    backend: Arc<dyn ToolBackend>,
    stop: CancellationToken,
) {
    let peer = tokio::select! {
        () = stop.cancelled() => return,
        admitted = backend.admit(peer) => match admitted {
            Some(peer) => peer,
            None => return,
        },
    };
    serve_connection(stream, peer, spec, backend, stop).await;
}

async fn serve_connection(
    stream: UnixStream,
    peer: Peer,
    spec: Arc<McpServerSpec>,
    backend: Arc<dyn ToolBackend>,
    stop: CancellationToken,
) {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(AsyncMutex::new(writer));
    let mut lines = FramedRead::new(
        reader,
        LinesCodec::new_with_max_length(
            spec.max_request_line_bytes
                .unwrap_or(MAX_REQUEST_LINE_BYTES),
        ),
    );
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS));
    // Cancelled when this loop ends (the client closed, an unreadable line,
    // or the server stopped): every request still running is then gone.
    let gone = CancellationToken::new();
    let _gone_when_closed = gone.clone().drop_guard();
    // Each running request's own signal, by its JSON id, for the client's
    // `notifications/cancelled`.
    let running: Arc<Mutex<HashMap<String, (u64, CancellationToken)>>> = Arc::default();
    let mut sequence: u64 = 0;
    loop {
        let permit = tokio::select! {
            () = stop.cancelled() => break,
            permit = Arc::clone(&in_flight).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        let line = tokio::select! {
            () = stop.cancelled() => break,
            line = lines.next() => match line {
                Some(Ok(line)) => line,
                Some(Err(error)) => {
                    tracing::warn!(%error, server = spec.wire_name, "closed an MCP connection after an unreadable request");
                    break;
                }
                None => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(&line) else {
            write_message(
                &writer,
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32_700, "message": "Parse error"}}),
            )
            .await;
            continue;
        };
        // A request without an id is a notification and gets no reply.
        let Some(id) = message.get("id").cloned() else {
            if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
                && let Some(request) = message
                    .get("params")
                    .and_then(|params| params.get("requestId"))
                && let Some((_, cancelled)) = lock(&running).get(&request.to_string())
            {
                cancelled.cancel();
            }
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = message.get("params").cloned();
        let writer = Arc::clone(&writer);
        let spec = Arc::clone(&spec);
        let backend = Arc::clone(&backend);
        let request = id.to_string();
        let request_gone = gone.child_token();
        let peer = peer.clone();
        sequence += 1;
        let this = sequence;
        lock(&running).insert(request.clone(), (this, request_gone.clone()));
        let running = Arc::clone(&running);
        tokio::spawn(async move {
            let _permit = permit;
            let body = run_request(
                &method,
                params,
                &spec,
                backend.as_ref(),
                peer,
                request_gone.clone(),
            )
            .await;
            {
                let mut running = lock(&running);
                // A later request may have reused the id; only this one's goes.
                if running.get(&request).is_some_and(|(seq, _)| *seq == this) {
                    running.remove(&request);
                }
            }
            let mut response = serde_json::Map::new();
            response.insert("jsonrpc".into(), Value::String("2.0".into()));
            response.insert("id".into(), id);
            if let Value::Object(body) = body {
                response.extend(body);
            }
            write_message(&writer, Value::Object(response)).await;
        });
    }
}

async fn run_request(
    method: &str,
    params: Option<Value>,
    spec: &McpServerSpec,
    backend: &dyn ToolBackend,
    peer: Peer,
    gone: CancellationToken,
) -> Value {
    match method {
        "initialize" => json!({"result": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {
                "name": spec.wire_name,
                "title": spec.title,
                "version": "1.0.0"
            },
            "instructions": spec.instructions
        }}),
        "ping" => json!({"result": {}}),
        "interface/worker-notice" if spec.wire_name == "iface" => {
            match backend
                .call("__worker_notice", params.unwrap_or_else(|| json!({})), peer)
                .await
            {
                Ok(output) => json!({"result": tool_result(output)}),
                Err(message) => {
                    json!({"result":{"isError":true,"content":[{"type":"text","text":message}]}})
                }
            }
        }
        "tools/list" => json!({"result": {"tools": spec.tools}}),
        "tools/call" => {
            let Some((name, arguments)) = parse_tool_call(params.as_ref()) else {
                return failure(-32_602, "Invalid params");
            };
            if !spec.allows(&name) {
                return failure(-32_602, "Unknown tool");
            }
            match backend.call_until_gone(&name, arguments, peer, gone).await {
                Ok(output) => json!({"result": tool_result(output)}),
                Err(message) => json!({"result": {
                    "content": [{"type": "text", "text": message}],
                    "isError": true
                }}),
            }
        }
        _ => failure(-32_601, "Method not found"),
    }
}

fn failure(code: i64, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

fn tool_result(output: ToolOutput) -> Value {
    match output {
        ToolOutput::Text(text) => json!({"content": [{"type": "text", "text": text}]}),
        ToolOutput::Image { png_base64, text } => json!({"content": [
            {"type": "image", "data": png_base64, "mimeType": "image/png"},
            {"type": "text", "text": text}
        ]}),
    }
}

fn parse_tool_call(value: Option<&Value>) -> Option<(String, Value)> {
    let object = value?.as_object()?;
    let name = object.get("name")?.as_str()?;
    if name.is_empty() || name.len() > 128 {
        return None;
    }
    let arguments = object
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    arguments.is_object().then(|| (name.to_owned(), arguments))
}

async fn write_message(writer: &AsyncMutex<OwnedWriteHalf>, value: Value) {
    let Ok(mut body) = serde_json::to_vec(&value) else {
        return;
    };
    body.push(b'\n');
    let mut writer = writer.lock().await;
    let _ = writer.write_all(&body).await;
    let _ = writer.flush().await;
}

/// One `tools/list` entry with a closed input schema and honest annotations.
pub(crate) fn tool(
    name: &str,
    description: &str,
    properties: &Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        },
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": read_only,
            "openWorldHint": false
        }
    })
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixStream,
    };

    use super::*;

    struct PeerEcho;

    #[async_trait]
    impl ToolBackend for PeerEcho {
        async fn call(&self, _name: &str, _input: Value, peer: Peer) -> Result<ToolOutput, String> {
            Ok(ToolOutput::Text(
                peer.pid.map_or("none".to_owned(), |pid| pid.to_string()),
            ))
        }
    }

    #[tokio::test]
    async fn the_backend_sees_the_connecting_process_id() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("peer.sock");
        let BoundSocket { listener, .. } = bind(temporary.path(), &path).unwrap();
        let stop = CancellationToken::new();
        serve(
            listener,
            &path,
            Arc::new(McpServerSpec {
                wire_name: "peer-test",
                title: "Peer",
                instructions: "",
                tools: vec![tool("whoami", "the caller's pid", &json!({}), &[], true)],
                compatibility_tools: &[],
                max_request_line_bytes: None,
            }),
            Arc::new(PeerEcho),
            stop.clone(),
        );
        let stream = UnixStream::connect(&path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let reply = test_support::exchange(
            &mut writer,
            &mut lines,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "whoami"}}),
        )
        .await;
        // The kernel names the peer: this test process, not anything the
        // request said.
        assert_eq!(
            reply["result"]["content"][0]["text"],
            std::process::id().to_string()
        );
        stop.cancel();
    }

    #[test]
    fn bind_replaces_only_a_stale_socket_and_never_follows_a_symlinked_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("sockets");
        let path = root.join("server.sock");

        // A leftover socket from an earlier run is replaced.
        let first = bind(&root, &path).unwrap();
        drop(first.listener);
        let second = bind(&root, &path).unwrap();
        assert_ne!(first.identity, second.identity);

        // Removal only takes the socket this bind created.
        remove_bound_socket(&path, first.identity);
        assert!(
            path.exists(),
            "a replaced socket must survive a stale removal"
        );
        remove_bound_socket(&path, second.identity);
        assert!(!path.exists());

        // Anything other than a stale socket is refused, not unlinked.
        std::fs::write(&path, b"keep").unwrap();
        assert!(bind(&root, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");

        // A symlinked socket directory is refused.
        let target = temporary.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = temporary.path().join("linked");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(bind(&link, &link.join("server.sock")).is_err());
        assert!(!target.join("server.sock").exists());
    }

    #[tokio::test]
    async fn an_oversized_request_line_closes_the_connection() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("limit.sock");
        let BoundSocket { listener, .. } = bind(temporary.path(), &path).unwrap();
        let stop = CancellationToken::new();
        serve(
            listener,
            &path,
            Arc::new(McpServerSpec {
                wire_name: "limit-test",
                title: "Limit",
                instructions: "",
                tools: Vec::new(),
                compatibility_tools: &[],
                max_request_line_bytes: None,
            }),
            Arc::new(PeerEcho),
            stop.clone(),
        );
        let stream = UnixStream::connect(&path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        // No newline: an unbounded reader would keep buffering this forever.
        let _ = writer
            .write_all(&vec![b'x'; MAX_REQUEST_LINE_BYTES + 1])
            .await;
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let closed = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
            .await
            .expect("the server closes the connection");
        assert!(matches!(closed, Ok(None) | Err(_)));
        stop.cancel();
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;
    use tokio::{
        io::{AsyncWriteExt, BufReader},
        net::unix::{OwnedReadHalf, OwnedWriteHalf},
    };

    /// One request/response round trip over a split client socket.
    pub(crate) async fn exchange(
        writer: &mut OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
        request: Value,
    ) -> Value {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let reply = lines.next_line().await.unwrap().unwrap();
        serde_json::from_str::<Value>(&reply).unwrap()
    }
}
