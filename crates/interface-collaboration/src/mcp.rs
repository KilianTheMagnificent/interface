//! Owner-private MCP transport for the frozen collaboration broker.
//!
//! This is intentionally a small MCP server, not a general SDK. It accepts
//! newline or `Content-Length` JSON-RPC frames, serves exactly the thirteen
//! Interface tools, and keeps the connection capability out of model input.

use std::{
    collections::HashMap,
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::COLLABORATION_MCP_TOOL_NAMES, orchestration::UNTRUSTED_WORKER_PREFIX,
};
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{Mutex, Notify, Semaphore, oneshot},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    broker::CollaborationModelAuthority,
    state::{
        CLIENT_REQUEST_ID_BYTES_MAX, CollaborationEngineId, CollaborationErrorCode,
        CollaborationMode, CollaborationReceiptV1, IDENTIFIER_BYTES_MAX, OBJECTIVE_BYTES_MAX,
        OBJECTIVE_CHARACTERS_MAX, ORIGIN_CAPABILITY_BYTES_MAX, REQUEST_TOOL_TIMEOUT_MS,
        SOURCE_SESSION_KEY_BYTES_MAX,
    },
};

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const MCP_SERVER_NAME: &str = "iface_collaboration";
const MCP_SERVER_WIRE_NAME: &str = "iface-collaboration";
const DEFAULT_HANDOFF_OBJECTIVE: &str =
    "Continue the project from its current state and complete the remaining work.";

const FRAME_BYTES: usize = 64 * 1_024;
const HEADER_BYTES: usize = 8 * 1_024;
const MAX_IN_FLIGHT: usize = 16;
const MAX_CONNECTIONS: usize = 32;
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 103;
/// Graceful stop waits at most this long for in-flight host calls to settle.
/// A call that may already have committed a provider effect is never cancelled
/// to meet the budget: it keeps running detached while the listener and the
/// socket file are torn down.
const STOP_DRAIN_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollaborationMcpIntent {
    pub mode: CollaborationMode,
    pub source_session_key: Option<String>,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollaborationMcpAccessRequest {
    pub collaboration_id: String,
}

/// Cloneable cancellation signal passed to host ingress. It fires on client
/// cancellation, disconnect, or the bounded transport deadline.
#[derive(Clone, Debug)]
pub struct IngressCancellation(CancellationToken);

impl IngressCancellation {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

pub type McpIngressErrorCode = CollaborationErrorCode;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("collaboration ingress failed")]
pub struct McpIngressError {
    pub code: McpIngressErrorCode,
    pub collaboration_id: Option<String>,
    pub retryable: bool,
}

impl McpIngressError {
    #[must_use]
    pub const fn new(code: McpIngressErrorCode) -> Self {
        Self {
            code,
            collaboration_id: None,
            retryable: false,
        }
    }
}

/// The durable service seam. Authority has already been resolved from the
/// listener's out-of-band capability; model input can never choose it.
#[async_trait]
pub trait CollaborationModelIngress: Send + Sync {
    async fn submit(
        &self,
        intent: CollaborationMcpIntent,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;

    async fn status(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;

    async fn cancel(
        &self,
        request: CollaborationMcpAccessRequest,
        authority: CollaborationModelAuthority,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrchestratorToolName {
    Workers,
    SpawnWorker,
    Delegate,
    Await,
    StopWorker,
    SubmitWorker,
    IntegrateWorker,
    RebaseWorker,
    Models,
}

impl OrchestratorToolName {
    /// The nine names a delegation host once served. They stay in
    /// `COLLABORATION_MCP_TOOL_NAMES` so a session working from stale
    /// instructions gets a bounded refusal instead of an unknown tool.
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "iface_workers" => Self::Workers,
            "iface_spawn_worker" => Self::SpawnWorker,
            "iface_delegate" => Self::Delegate,
            "iface_await" => Self::Await,
            "iface_stop_worker" => Self::StopWorker,
            "iface_submit_worker" => Self::SubmitWorker,
            "iface_integrate_worker" => Self::IntegrateWorker,
            "iface_rebase_worker" => Self::RebaseWorker,
            "iface_models" => Self::Models,
            _ => return None,
        })
    }
}

/// No run has an orchestrator host, so every orchestrator tool answers with
/// the refusal the host used to return for an undesignated session. Tools
/// that would carry worker text keep the untrusted framing, so a caller sees
/// exactly the bytes it always saw.
fn orchestrator_refusal(tool: OrchestratorToolName) -> Value {
    let untrusted = matches!(
        tool,
        OrchestratorToolName::Workers
            | OrchestratorToolName::Await
            | OrchestratorToolName::SubmitWorker
            | OrchestratorToolName::IntegrateWorker
    );
    let text =
        "not_an_orchestrator: Ask the human to enable an orchestrator policy for this session.";
    let mut structured = json!({
        "version": 1,
        "ok": false,
        "code": "not_an_orchestrator",
        "message": "This session is not a human-designated orchestrator.",
        "safeAction": "Ask the human to enable an orchestrator policy for this session."
    });
    if untrusted {
        let object = structured.as_object_mut().expect("object");
        object.insert(
            "untrustedNotice".into(),
            Value::String(UNTRUSTED_WORKER_PREFIX.into()),
        );
        object.insert("untrusted".into(), Value::Bool(true));
    }
    json!({
        "content": [{"type": "text", "text": if untrusted {
            format!("{UNTRUSTED_WORKER_PREFIX}\n{text}")
        } else { text.to_owned() }}],
        "structuredContent": structured,
        "isError": true
    })
}

/// Internal host-bound ingress consumed by the wire server. It is public for
/// adversarial transport tests but never crosses renderer IPC.
#[async_trait]
pub trait CollaborationMcpRequestIngress: Send + Sync {
    async fn submit(
        &self,
        origin_connection_id: &str,
        intent: CollaborationMcpIntent,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;

    async fn status(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;

    async fn cancel(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError>;
}

pub struct CollaborationMcpSocketOptions {
    pub socket_path: PathBuf,
    pub source_engine: CollaborationEngineId,
    pub origin_connection_id: String,
    pub ingress: Arc<dyn CollaborationMcpRequestIngress>,
    pub request_timeout: Duration,
    pub stop_drain_timeout: Duration,
    pub max_in_flight_per_connection: usize,
    pub max_connections: usize,
}

impl CollaborationMcpSocketOptions {
    #[must_use]
    pub fn frozen(
        socket_path: PathBuf,
        source_engine: CollaborationEngineId,
        origin_connection_id: String,
        ingress: Arc<dyn CollaborationMcpRequestIngress>,
    ) -> Self {
        Self {
            socket_path,
            source_engine,
            origin_connection_id,
            ingress,
            request_timeout: Duration::from_millis(REQUEST_TOOL_TIMEOUT_MS),
            stop_drain_timeout: Duration::from_millis(STOP_DRAIN_TIMEOUT_MS),
            max_in_flight_per_connection: MAX_IN_FLIGHT,
            max_connections: MAX_CONNECTIONS,
        }
    }
}

pub(crate) use interface_common::fsid::FileIdentity;

pub struct CollaborationMcpSocket {
    socket_path: PathBuf,
    parent_path: PathBuf,
    parent_identity: FileIdentity,
    socket_identity: FileIdentity,
    stop: CancellationToken,
    task: StdMutex<Option<JoinHandle<io::Result<()>>>>,
    ingress: Arc<IngressTracker>,
    stop_drain_timeout: Duration,
    stop_lock: Mutex<()>,
    stopped: AtomicBool,
}

impl CollaborationMcpSocket {
    #[allow(clippy::unused_async)]
    pub async fn listen(options: CollaborationMcpSocketOptions) -> io::Result<Arc<Self>> {
        validate_socket_options(&options)?;
        let (parent_path, parent_identity) = prepare_private_parent(
            options
                .socket_path
                .parent()
                .ok_or_else(|| invalid_input("collaboration socket has no parent"))?,
        )?;
        let file_name = options
            .socket_path
            .file_name()
            .ok_or_else(|| invalid_input("collaboration socket has no file name"))?;
        let socket_path = parent_path.join(file_name);
        validate_socket_path(&socket_path)?;
        if std::fs::symlink_metadata(&socket_path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "collaboration socket path already exists",
            ));
        }
        let listener = UnixListener::bind(&socket_path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "unable to start collaboration MCP socket",
            )
        })?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        let metadata = std::fs::symlink_metadata(&socket_path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != std::fs::symlink_metadata(&parent_path)?.uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "collaboration socket is not owner-only",
            ));
        }
        verify_private_parent(&parent_path, parent_identity)?;
        let socket_identity = FileIdentity::of(&metadata);
        let stop = CancellationToken::new();
        let ingress = IngressTracker::root();
        let task_stop = stop.clone();
        let task_ingress = Arc::clone(&ingress);
        let settings = ConnectionSettings {
            source_engine: options.source_engine,
            origin: options.origin_connection_id,
            ingress: options.ingress,
            request_timeout: options.request_timeout,
            max_in_flight: options.max_in_flight_per_connection,
        };
        let expected_uid = metadata.uid();
        let stop_drain_timeout = options.stop_drain_timeout;
        let connection_limit = Arc::new(Semaphore::new(options.max_connections));
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    () = task_stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let (stream, _) = match accepted {
                            Ok(value) => value,
                            Err(_error) if task_stop.is_cancelled() => break,
                            Err(_error) => {
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                continue;
                            }
                        };
                        if !stream.peer_cred().is_ok_and(|cred| cred.uid() == expected_uid) {
                            continue;
                        }
                        let Ok(permit) = Arc::clone(&connection_limit).try_acquire_owned()
                        else {
                            continue;
                        };
                        let connection_stop = task_stop.child_token();
                        let connection_ingress = Arc::clone(&task_ingress);
                        let connection_settings = settings.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            Box::pin(serve_connection(
                                stream,
                                connection_settings,
                                connection_ingress,
                                connection_stop,
                            ))
                            .await;
                        });
                    }
                    joined = connections.join_next(), if !connections.is_empty() => {
                        let _ = joined;
                    }
                }
            }
            while connections.join_next().await.is_some() {}
            Ok(())
        });
        Ok(Arc::new(Self {
            socket_path,
            parent_path,
            parent_identity,
            socket_identity,
            stop,
            task: StdMutex::new(Some(task)),
            ingress,
            stop_drain_timeout,
            stop_lock: Mutex::new(()),
            stopped: AtomicBool::new(false),
        }))
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn drain(&self) {
        self.ingress.drain().await;
    }

    pub async fn stop(&self) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.stop_lock.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = self.stop_once().await;
        if result.is_ok() {
            self.stopped.store(true, Ordering::Release);
        }
        result
    }

    async fn stop_once(&self) -> io::Result<()> {
        // Stop serving before verifying identity: a refused unlink must not
        // leave the accept task, the listener fd and the socket file alive.
        self.stop.cancel();
        let task = lock_std(&self.task).take();
        // Bounded drain: an in-flight host call may already have committed a
        // provider effect, so it is never cancelled here. Once the budget is
        // spent the accept task and those calls keep running detached, and
        // teardown continues so the listener fd and socket file always go.
        let settled = timeout(self.stop_drain_timeout, async move {
            if let Some(task) = task {
                task.await
                    .map_err(|_| io::Error::other("collaboration socket task failed"))??;
            }
            self.ingress.drain().await;
            Ok::<(), io::Error>(())
        })
        .await;
        if let Ok(result) = settled {
            result?;
        }
        verify_socket_for_close(
            &self.parent_path,
            self.parent_identity,
            &self.socket_path,
            self.socket_identity,
        )?;
        unlink_matching_socket(&self.socket_path, self.socket_identity)
    }
}

struct IngressTracker {
    active: AtomicUsize,
    changed: Notify,
    aggregate: Option<Arc<Self>>,
}

impl IngressTracker {
    fn root() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicUsize::new(0),
            changed: Notify::new(),
            aggregate: None,
        })
    }

    fn child(aggregate: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicUsize::new(0),
            changed: Notify::new(),
            aggregate: Some(aggregate),
        })
    }

    fn spawn<T: Send + 'static>(
        self: &Arc<Self>,
        future: impl Future<Output = T> + Send + 'static,
    ) -> oneshot::Receiver<T> {
        self.active.fetch_add(1, Ordering::AcqRel);
        if let Some(aggregate) = &self.aggregate {
            aggregate.active.fetch_add(1, Ordering::AcqRel);
        }
        let (sender, receiver) = oneshot::channel();
        let tracker = Arc::clone(self);
        tokio::spawn(async move {
            let result = future.await;
            let _ = sender.send(result);
            tracker.active.fetch_sub(1, Ordering::AcqRel);
            tracker.changed.notify_waiters();
            if let Some(aggregate) = &tracker.aggregate {
                aggregate.active.fetch_sub(1, Ordering::AcqRel);
                aggregate.changed.notify_waiters();
            }
        });
        receiver
    }

    async fn drain(&self) {
        loop {
            // Register before checking the count: notify_waiters stores no
            // permit, so a completion between the load and the registration
            // would otherwise be missed.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

use std::future::Future;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Framing {
    Newline,
    ContentLength,
}

#[derive(Debug)]
struct DecodedFrame {
    framing: Framing,
    payload: Vec<u8>,
}

#[derive(Clone)]
struct PendingRequest {
    cancellation: CancellationToken,
    method: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum JsonRpcId {
    String(String),
    Number(i64),
}

impl JsonRpcId {
    fn parse(value: &Value) -> Option<Self> {
        match value {
            Value::String(value)
                if !value.is_empty() && value.len() <= 256 && !has_control(value) =>
            {
                Some(Self::String(value.clone()))
            }
            Value::Number(value) => value.as_i64().map(Self::Number),
            _ => None,
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::String(value) => Value::String(value.clone()),
            Self::Number(value) => Value::from(*value),
        }
    }
}

#[derive(Debug)]
enum FrameDecode {
    NeedMore,
    Frame(DecodedFrame, usize),
    Recoverable {
        framing: Framing,
        code: &'static str,
        consumed: usize,
    },
    Fatal {
        framing: Framing,
        code: &'static str,
    },
}

/// What every connection on one listener shares: the listener's out-of-band
/// authority and its limits.
#[derive(Clone)]
struct ConnectionSettings {
    source_engine: CollaborationEngineId,
    origin: String,
    ingress: Arc<dyn CollaborationMcpRequestIngress>,
    request_timeout: Duration,
    max_in_flight: usize,
}

/// One client connection's state, shared by its reader loop and every request
/// task it spawns.
struct Connection {
    settings: ConnectionSettings,
    /// This connection's own settlement count. It also feeds the socket's
    /// aggregate count, which owns drain semantics.
    tracker: Arc<IngressTracker>,
    writer: Mutex<OwnedWriteHalf>,
    pending: Mutex<HashMap<JsonRpcId, PendingRequest>>,
    initialized: AtomicBool,
    client_ready: AtomicBool,
    initialize_pending: AtomicBool,
}

async fn serve_connection(
    stream: UnixStream,
    settings: ConnectionSettings,
    socket_tracker: Arc<IngressTracker>,
    server_stop: CancellationToken,
) {
    let (mut reader, writer) = stream.into_split();
    // The socket tracker owns aggregate drain semantics; each connection keeps
    // its own settlement count so one busy client cannot consume another
    // client's frozen 16-request allowance.
    let connection = Arc::new(Connection {
        settings,
        tracker: IngressTracker::child(socket_tracker),
        writer: Mutex::new(writer),
        pending: Mutex::new(HashMap::new()),
        initialized: AtomicBool::new(false),
        client_ready: AtomicBool::new(false),
        initialize_pending: AtomicBool::new(false),
    });
    let mut tasks = JoinSet::new();
    let mut input = Vec::new();
    let mut read_buffer = [0_u8; 16 * 1_024];
    let mut disconnected = false;

    'connection: loop {
        tokio::select! {
            () = server_stop.cancelled() => break,
            joined = tasks.join_next(), if !tasks.is_empty() => {
                let _ = joined;
            }
            read = reader.read(&mut read_buffer) => {
                let count = match read {
                    Ok(0) | Err(_) => {
                        disconnected = true;
                        break;
                    }
                    Ok(count) => count,
                };
                input.extend_from_slice(&read_buffer[..count]);
                loop {
                    let mut skip = 0;
                    while input[skip..].starts_with(b"\n") {
                        skip += 1;
                    }
                    while input[skip..].starts_with(b"\r\n") {
                        skip += 2;
                    }
                    if skip > 0 {
                        input.drain(..skip);
                    }
                    if input.is_empty() {
                        break;
                    }
                    match decode_next_frame(&input) {
                        FrameDecode::NeedMore => break,
                        FrameDecode::Frame(frame, consumed) => {
                            input.drain(..consumed);
                            handle_frame(frame, &connection, &mut tasks).await;
                        }
                        FrameDecode::Recoverable { framing, code, consumed } => {
                            input.drain(..consumed);
                            write_rpc_error(&connection.writer, Value::Null, framing, -32_700,
                                "Parse error", code).await;
                        }
                        FrameDecode::Fatal { framing, code } => {
                            write_rpc_error(&connection.writer, Value::Null, framing, -32_700,
                                "Parse error", code).await;
                            break 'connection;
                        }
                    }
                }
            }
        }
    }

    let snapshot = connection
        .pending
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    if disconnected {
        for request in snapshot {
            request.cancellation.cancel();
        }
    }
    while tasks.join_next().await.is_some() {}
    connection.tracker.drain().await;
    let _ = connection.writer.lock().await.shutdown().await;
}

async fn handle_frame(frame: DecodedFrame, connection: &Arc<Connection>, tasks: &mut JoinSet<()>) {
    let framing = frame.framing;
    let writer = &connection.writer;
    let Ok(value) = serde_json::from_slice::<Value>(&frame.payload) else {
        write_rpc_error(
            writer,
            Value::Null,
            framing,
            -32_700,
            "Parse error",
            "PARSE_ERROR",
        )
        .await;
        return;
    };
    let Some(object) = value.as_object() else {
        write_rpc_error(
            writer,
            Value::Null,
            framing,
            -32_600,
            "Invalid Request",
            "INVALID_REQUEST",
        )
        .await;
        return;
    };
    let method = object.get("method").and_then(Value::as_str);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || method.is_none_or(|method| method.is_empty() || method.len() > 128)
    {
        let id = object
            .get("id")
            .and_then(JsonRpcId::parse)
            .map_or(Value::Null, |id| id.value());
        write_rpc_error(
            writer,
            id,
            framing,
            -32_600,
            "Invalid Request",
            "INVALID_REQUEST",
        )
        .await;
        return;
    }
    let method = method.expect("checked").to_owned();
    if method == "notifications/cancelled" && !object.contains_key("id") {
        if let Some(request_id) = object
            .get("params")
            .and_then(Value::as_object)
            .and_then(|params| params.get("requestId"))
            .and_then(JsonRpcId::parse)
        {
            if let Some(request) = connection.pending.lock().await.get(&request_id) {
                if request.method != "initialize" {
                    request.cancellation.cancel();
                }
            }
        }
        return;
    }
    if method == "notifications/initialized" && !object.contains_key("id") {
        if connection.initialized.load(Ordering::Acquire) && valid_meta_only(object.get("params")) {
            connection.client_ready.store(true, Ordering::Release);
        }
        return;
    }
    let Some(id) = object.get("id").and_then(JsonRpcId::parse) else {
        if object.contains_key("id") {
            write_rpc_error(
                writer,
                Value::Null,
                framing,
                -32_600,
                "Invalid Request",
                "INVALID_REQUEST_ID",
            )
            .await;
        }
        return;
    };
    if method == "initialize"
        && (connection.initialized.load(Ordering::Acquire)
            || connection.initialize_pending.swap(true, Ordering::AcqRel))
    {
        write_rpc_error(
            writer,
            id.value(),
            framing,
            -32_602,
            "Invalid params",
            "INVALID_INITIALIZE",
        )
        .await;
        return;
    }
    let cancellation = CancellationToken::new();
    {
        let mut pending_guard = connection.pending.lock().await;
        if pending_guard.contains_key(&id) {
            drop(pending_guard);
            if method == "initialize" {
                connection
                    .initialize_pending
                    .store(false, Ordering::Release);
            }
            write_rpc_error(
                writer,
                id.value(),
                framing,
                -32_600,
                "Invalid Request",
                "DUPLICATE_REQUEST_ID",
            )
            .await;
            return;
        }
        let max_in_flight = connection.settings.max_in_flight;
        if pending_guard.len() >= max_in_flight
            || connection.tracker.active.load(Ordering::Acquire) >= max_in_flight
        {
            drop(pending_guard);
            if method == "initialize" {
                connection
                    .initialize_pending
                    .store(false, Ordering::Release);
            }
            write_rpc_error(
                writer,
                id.value(),
                framing,
                -32_001,
                "Request limit exceeded",
                "LIMIT_EXCEEDED",
            )
            .await;
            return;
        }
        pending_guard.insert(
            id.clone(),
            PendingRequest {
                cancellation: cancellation.clone(),
                method: method.clone(),
            },
        );
    }
    let params = object.get("params").cloned();
    let connection = Arc::clone(connection);
    tasks.spawn(async move {
        let response = run_request(&method, params, &connection, cancellation.clone()).await;
        if !cancellation.is_cancelled() {
            if response.set_initialized {
                connection.initialized.store(true, Ordering::Release);
            }
            write_rpc_response(&connection.writer, id.value(), framing, response.body).await;
        }
        if method == "initialize" {
            connection
                .initialize_pending
                .store(false, Ordering::Release);
        }
        connection.pending.lock().await.remove(&id);
    });
}

struct RequestResponse {
    body: Value,
    set_initialized: bool,
}

async fn run_request(
    method: &str,
    params: Option<Value>,
    connection: &Connection,
    cancellation: CancellationToken,
) -> RequestResponse {
    let initialized = connection.initialized.load(Ordering::Acquire);
    let client_ready = connection.client_ready.load(Ordering::Acquire);
    let source_engine = connection.settings.source_engine;
    match method {
        "ping" if valid_meta_only(params.as_ref()) => success(json!({})),
        "initialize" if !initialized && valid_initialize(params.as_ref()) => RequestResponse {
            body: json!({"result": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {
                    "name": MCP_SERVER_WIRE_NAME,
                    "title": "Interface Collaboration Broker",
                    "version": "1.0.0"
                },
                "instructions": collaboration_server_instructions(source_engine)
            }}),
            set_initialized: true,
        },
        "initialize" => failure(-32_602, "Invalid params", "INVALID_INITIALIZE"),
        "tools/list" | "tools/call" if !client_ready => failure(
            -32_002,
            "Server initialization is incomplete",
            "SERVER_NOT_READY",
        ),
        "tools/list" if valid_meta_only(params.as_ref()) => success(json!({
            "tools": collaboration_mcp_tools(source_engine)
        })),
        "tools/list" | "ping" => failure(-32_602, "Invalid params", "INVALID_ARGUMENT"),
        "tools/call" => {
            let Some((name, arguments)) = parse_tool_call(params.as_ref()) else {
                return failure(-32_602, "Invalid params", "INVALID_ARGUMENT");
            };
            if !COLLABORATION_MCP_TOOL_NAMES.contains(&name.as_str()) {
                return failure(-32_602, "Unknown tool", "TOOL_NOT_FOUND");
            }
            match invoke_tool(&name, arguments, connection, cancellation).await {
                Some(value) => success(value),
                None => failure(-32_602, "Invalid params", "INVALID_ARGUMENT"),
            }
        }
        _ => failure(-32_601, "Method not found", "METHOD_NOT_FOUND"),
    }
}

fn success(result: Value) -> RequestResponse {
    RequestResponse {
        body: json!({"result": result}),
        set_initialized: false,
    }
}

fn failure(code: i64, message: &str, stable_code: &str) -> RequestResponse {
    RequestResponse {
        body: json!({"error": {
            "code": code,
            "message": message,
            "data": {"code": stable_code}
        }}),
        set_initialized: false,
    }
}

async fn write_rpc_response(
    writer: &Mutex<OwnedWriteHalf>,
    id: Value,
    framing: Framing,
    body: Value,
) {
    let mut response = Map::new();
    response.insert("jsonrpc".into(), Value::String("2.0".into()));
    response.insert("id".into(), id);
    if let Value::Object(body) = body {
        response.extend(body);
    }
    write_message(writer, Value::Object(response), framing).await;
}

async fn write_rpc_error(
    writer: &Mutex<OwnedWriteHalf>,
    id: Value,
    framing: Framing,
    code: i64,
    message: &str,
    stable_code: &str,
) {
    write_message(
        writer,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message, "data": {"code": stable_code}}
        }),
        framing,
    )
    .await;
}

async fn write_message(writer: &Mutex<OwnedWriteHalf>, value: Value, framing: Framing) {
    let Ok(body) = serde_json::to_vec(&value) else {
        return;
    };
    let mut writer = writer.lock().await;
    match framing {
        Framing::Newline => {
            let _ = writer.write_all(&body).await;
            let _ = writer.write_all(b"\n").await;
        }
        Framing::ContentLength => {
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            let _ = writer.write_all(header.as_bytes()).await;
            let _ = writer.write_all(&body).await;
        }
    }
    let _ = writer.flush().await;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IngressWaitError {
    Deadline,
    Cancelled,
    Closed,
}

/// A validated tool call bound for the host ingress.
enum ToolOperation {
    Submit(CollaborationMcpIntent),
    Status(CollaborationMcpAccessRequest),
    Cancel(CollaborationMcpAccessRequest),
}

/// `None` means the arguments were invalid; the caller answers `INVALID_ARGUMENT`.
async fn invoke_tool(
    name: &str,
    arguments: Value,
    connection: &Connection,
    cancellation: CancellationToken,
) -> Option<Value> {
    if let Some(tool) = OrchestratorToolName::parse(name) {
        return Some(orchestrator_refusal(tool));
    }

    let source_engine = connection.settings.source_engine;
    let operation = match name {
        "iface_consult" => ToolOperation::Submit(parse_intent(
            CollaborationMode::Consult,
            arguments,
            source_engine,
        )?),
        "iface_handoff" => ToolOperation::Submit(parse_intent(
            CollaborationMode::Handoff,
            arguments,
            source_engine,
        )?),
        "iface_collaboration_status" => ToolOperation::Status(parse_access_request(arguments)?),
        _ => ToolOperation::Cancel(parse_access_request(arguments)?),
    };
    // Only a status read can never have committed anything.
    let may_have_committed = !matches!(operation, ToolOperation::Status(_));
    let origin = connection.settings.origin.clone();
    let host = Arc::clone(&connection.settings.ingress);
    let host_cancellation = cancellation.child_token();
    let host_signal = host_cancellation.clone();
    let operation = connection.tracker.spawn(async move {
        let cancellation = IngressCancellation(host_cancellation);
        match operation {
            ToolOperation::Submit(intent) => host.submit(&origin, intent, cancellation).await,
            ToolOperation::Status(access) => host.status(&origin, access, cancellation).await,
            ToolOperation::Cancel(access) => host.cancel(&origin, access, cancellation).await,
        }
    });
    let result = wait_ingress(
        operation,
        cancellation,
        host_signal,
        connection.settings.request_timeout,
    )
    .await;
    Some(match result {
        Ok(Ok(receipt)) => tool_success(name, &receipt),
        Ok(Err(error)) => tool_failure(&error, false, false),
        Err(IngressWaitError::Deadline) => tool_failure(
            &McpIngressError::new(if may_have_committed {
                CollaborationErrorCode::RecoveryRequired
            } else {
                CollaborationErrorCode::BrokerUnavailable
            }),
            true,
            may_have_committed,
        ),
        Err(IngressWaitError::Cancelled | IngressWaitError::Closed) => tool_failure(
            &McpIngressError::new(CollaborationErrorCode::BrokerUnavailable),
            false,
            false,
        ),
    })
}

async fn wait_ingress<T>(
    mut receiver: oneshot::Receiver<T>,
    request_cancellation: CancellationToken,
    host_cancellation: CancellationToken,
    deadline: Duration,
) -> Result<T, IngressWaitError> {
    tokio::select! {
        biased;
        () = request_cancellation.cancelled() => Err(IngressWaitError::Cancelled),
        result = timeout(deadline, &mut receiver) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(IngressWaitError::Closed),
            Err(_) => {
                host_cancellation.cancel();
                Err(IngressWaitError::Deadline)
            }
        }
    }
}

fn tool_success(name: &str, receipt: &CollaborationReceiptV1) -> Value {
    let verb = match name {
        "iface_consult" | "iface_handoff" => "intent recorded",
        "iface_collaboration_status" => "status",
        _ => "cancel requested",
    };
    let error = receipt.error.as_ref().map_or_else(String::new, |error| {
        format!(" error {}", error.code.as_str())
    });
    let mode = match receipt.mode {
        CollaborationMode::Consult => "consult",
        CollaborationMode::Handoff => "handoff",
    };
    let summary = format!(
        "{} {verb}: {} rev {} state {}{error}",
        mode, receipt.collaboration_id, receipt.revision, receipt.state
    );
    json!({
        "content": [{"type": "text", "text": summary}],
        "structuredContent": receipt,
        "isError": false
    })
}

fn tool_failure(
    error: &McpIngressError,
    deadline: bool,
    deadline_may_have_committed: bool,
) -> Value {
    let code = if deadline_may_have_committed {
        CollaborationErrorCode::RecoveryRequired
    } else {
        error.code
    };
    let retryable = if deadline && !deadline_may_have_committed {
        true
    } else {
        error.retryable
    };
    let mut structured = json!({
        "version": 1,
        "code": code,
        "safeNextAction": code.safe_next_action(),
        "retryable": retryable
    });
    if let Some(identifier) = error
        .collaboration_id
        .as_ref()
        .filter(|identifier| bounded_identifier(identifier, IDENTIFIER_BYTES_MAX))
    {
        structured
            .as_object_mut()
            .expect("object")
            .insert("collaborationId".into(), Value::String(identifier.clone()));
    }
    let code_text = code.as_str();
    json!({
        "content": [{"type": "text", "text": format!(
            "{code_text}: {}", code.safe_next_action())}],
        "structuredContent": structured,
        "isError": true
    })
}

fn parse_intent(
    mode: CollaborationMode,
    value: Value,
    source_engine: CollaborationEngineId,
) -> Option<CollaborationMcpIntent> {
    let object = value.as_object()?;
    let allowed = if source_engine == CollaborationEngineId::Codex {
        &[
            "sourceSessionKey",
            "targetEngine",
            "objective",
            "clientRequestId",
        ][..]
    } else {
        &["targetEngine", "objective", "clientRequestId"][..]
    };
    if !only_keys(object, allowed) {
        return None;
    }
    let source_session_key = if source_engine == CollaborationEngineId::Codex {
        match object.get("sourceSessionKey") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| bounded_text(value, SOURCE_SESSION_KEY_BYTES_MAX, false, true))?
                    .to_owned(),
            ),
        }
    } else {
        None
    };
    let target_engine = CollaborationEngineId::from_wire(object.get("targetEngine")?.as_str()?)?;
    if target_engine == source_engine {
        return None;
    }
    let objective = match object.get("objective").and_then(Value::as_str) {
        Some(value) => value,
        None if mode == CollaborationMode::Handoff => DEFAULT_HANDOFF_OBJECTIVE,
        None => return None,
    };
    if !bounded_text(objective, OBJECTIVE_BYTES_MAX, false, false)
        || objective.chars().count() > OBJECTIVE_CHARACTERS_MAX
    {
        return None;
    }
    let client_request_id = object.get("clientRequestId")?.as_str()?;
    if !bounded_identifier(client_request_id, CLIENT_REQUEST_ID_BYTES_MAX) {
        return None;
    }
    Some(CollaborationMcpIntent {
        mode,
        source_session_key,
        target_engine,
        objective: objective.trim().to_owned(),
        client_request_id: client_request_id.to_owned(),
    })
}

fn parse_access_request(value: Value) -> Option<CollaborationMcpAccessRequest> {
    let object = value.as_object()?;
    if !only_keys(object, &["collaborationId"]) {
        return None;
    }
    let collaboration_id = object.get("collaborationId")?.as_str()?;
    bounded_identifier(collaboration_id, IDENTIFIER_BYTES_MAX).then(|| {
        CollaborationMcpAccessRequest {
            collaboration_id: collaboration_id.to_owned(),
        }
    })
}

fn parse_tool_call(value: Option<&Value>) -> Option<(String, Value)> {
    let object = value?.as_object()?;
    if !only_keys(object, &["name", "arguments", "_meta"])
        || object.get("_meta").is_some_and(|value| !value.is_object())
    {
        return None;
    }
    let name = object.get("name")?.as_str()?;
    if name.is_empty() || name.len() > 128 {
        return None;
    }
    Some((
        name.to_owned(),
        object
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({})),
    ))
}

fn valid_initialize(value: Option<&Value>) -> bool {
    let Some(object) = value.and_then(Value::as_object) else {
        return false;
    };
    object
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty() && value.len() <= 32)
        && object.get("capabilities").is_some_and(Value::is_object)
        && object
            .get("clientInfo")
            .and_then(Value::as_object)
            .is_some_and(|client| {
                client
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|value| bounded_text(value, 128, false, true))
                    && client
                        .get("version")
                        .and_then(Value::as_str)
                        .is_some_and(|value| bounded_text(value, 128, false, true))
            })
}

fn valid_meta_only(value: Option<&Value>) -> bool {
    value.is_none_or(|value| {
        value.as_object().is_some_and(|object| {
            only_keys(object, &["_meta"]) && object.get("_meta").is_none_or(Value::is_object)
        })
    })
}

fn only_keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn bounded_identifier(value: &str, max_bytes: usize) -> bool {
    bounded_text(value, max_bytes, false, true)
}

fn bounded_text(value: &str, max_bytes: usize, allow_empty: bool, single_line: bool) -> bool {
    (allow_empty || !value.trim().is_empty())
        && value.len() <= max_bytes
        && !value.contains('\0')
        && (!single_line || !has_control(value))
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

#[must_use]
pub fn collaboration_mcp_tools(source_engine: CollaborationEngineId) -> Vec<Value> {
    let targets = CollaborationEngineId::ALL
        .into_iter()
        .filter(|candidate| *candidate != source_engine)
        .map(CollaborationEngineId::as_str)
        .collect::<Vec<_>>();
    let mut intent_properties = Map::new();
    if source_engine == CollaborationEngineId::Codex {
        intent_properties.insert(
            "sourceSessionKey".into(),
            json!({
                "type": "string",
                "minLength": 1,
                "maxLength": SOURCE_SESSION_KEY_BYTES_MAX,
                "description": "Optional Codex source key. Omit it when Interface has not supplied one; the host infers identity only when this connection has exactly one bound source."
            }),
        );
    }
    intent_properties.insert(
        "targetEngine".into(),
        json!({"type": "string", "enum": targets}),
    );
    intent_properties.insert(
        "objective".into(),
        json!({
            "type": "string",
            "minLength": 1,
            "maxLength": OBJECTIVE_CHARACTERS_MAX,
            "description": format!("Bounded to {OBJECTIVE_BYTES_MAX} UTF-8 bytes.")
        }),
    );
    intent_properties.insert("clientRequestId".into(), client_request_id_property());

    let access_schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "collaborationId": {"type": "string", "minLength": 1,
                "maxLength": IDENTIFIER_BYTES_MAX}
        },
        "required": ["collaborationId"]
    });

    let tools = vec![
        tool_definition(
            "iface_consult",
            "Consult another model",
            "This records an intent; it does not run the other model. Interface prompts the human. After approval, the peer settles, and the human runs /peer accept, the advisory is injected into a later turn. Do not poll. Mint one UUID per logical request and reuse that clientRequestId; do not retry with a new one. You cannot target this engine.",
            Value::Object(intent_schema(intent_properties.clone(), true)),
            false,
            false,
            true,
            true,
        ),
        tool_definition(
            "iface_handoff",
            "Request a handoff",
            "This records a host-approved handoff intent; it does not transfer control immediately. Do not poll. Mint one UUID per logical request and reuse that clientRequestId; do not retry with a new one. You cannot target this engine.",
            Value::Object(intent_schema(intent_properties, false)),
            false,
            true,
            true,
            true,
        ),
        tool_definition(
            "iface_collaboration_status",
            "Get collaboration status",
            "Recovery only: return a sanitized receipt for a collaboration owned by this connection. Do not poll while waiting for peer text.",
            access_schema.clone(),
            true,
            false,
            true,
            false,
        ),
        tool_definition(
            "iface_collaboration_cancel",
            "Cancel collaboration",
            "Recovery only: request idempotent cancellation before the irreversible boundary.",
            access_schema,
            false,
            true,
            true,
            false,
        ),
    ];
    tools
}

fn intent_schema(properties: Map<String, Value>, require_objective: bool) -> Map<String, Value> {
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("additionalProperties".into(), Value::Bool(false));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert(
        "required".into(),
        json!(if require_objective {
            vec!["targetEngine", "objective", "clientRequestId"]
        } else {
            vec!["targetEngine", "clientRequestId"]
        }),
    );
    schema
}

fn client_request_id_property() -> Value {
    json!({
        "type": "string", "minLength": 1,
        "maxLength": CLIENT_REQUEST_ID_BYTES_MAX,
        "description": "Mint one UUID per logical request and reuse it for every retry."
    })
}

#[allow(clippy::fn_params_excessive_bools)]
fn tool_definition(
    name: &str,
    title: &str,
    description: &str,
    schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": format!("Grok name: {MCP_SERVER_NAME}__{name}. {description}"),
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": idempotent,
            "openWorldHint": open_world
        },
        "execution": {"taskSupport": "forbidden"}
    })
}

fn collaboration_server_instructions(source_engine: CollaborationEngineId) -> String {
    let grok_names = if source_engine == CollaborationEngineId::AcpGrok {
        format!(
            " Grok names them {}.",
            COLLABORATION_MCP_TOOL_NAMES
                .iter()
                .map(|name| format!("{MCP_SERVER_NAME}__{name}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        String::new()
    };
    format!(
        "These tools record host-reviewed asynchronous collaboration intents. They do not run another model or return peer text in-band. Do not poll.{grok_names}"
    )
}

fn validate_socket_options(options: &CollaborationMcpSocketOptions) -> io::Result<()> {
    validate_socket_path(&options.socket_path)?;
    if !bounded_identifier(&options.origin_connection_id, ORIGIN_CAPABILITY_BYTES_MAX) {
        return Err(invalid_input(
            "origin connection identity must be a bounded identifier",
        ));
    }
    if options.request_timeout.is_zero()
        || options.request_timeout > Duration::from_millis(REQUEST_TOOL_TIMEOUT_MS)
        || options.stop_drain_timeout.is_zero()
        || options.stop_drain_timeout > Duration::from_millis(STOP_DRAIN_TIMEOUT_MS)
        || options.max_in_flight_per_connection == 0
        || options.max_in_flight_per_connection > MAX_IN_FLIGHT
        || options.max_connections == 0
        || options.max_connections > MAX_CONNECTIONS
    {
        return Err(invalid_input("invalid collaboration MCP transport limits"));
    }
    Ok(())
}

fn validate_socket_path(socket_path: &Path) -> io::Result<()> {
    if !socket_path.is_absolute()
        || socket_path.as_os_str().as_encoded_bytes().contains(&0)
        || socket_path.file_name().is_none()
        || socket_path.as_os_str().as_encoded_bytes().len() > MAX_UNIX_SOCKET_PATH_BYTES
    {
        return Err(invalid_input(
            "collaboration socket path must be a bounded absolute path",
        ));
    }
    Ok(())
}

fn prepare_private_parent(directory: &Path) -> io::Result<(PathBuf, FileIdentity)> {
    if !directory.is_absolute() || directory.parent().is_none() {
        return Err(invalid_input(
            "collaboration socket parent must be a non-root absolute path",
        ));
    }
    let owner_uid = nearest_existing_ancestor(directory)?.metadata()?.uid();
    std::fs::create_dir_all(directory)?;
    let before = std::fs::symlink_metadata(directory)?;
    if !before.is_dir() || before.file_type().is_symlink() || before.uid() != owner_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration socket parent is not a real owner directory",
        ));
    }
    let canonical = std::fs::canonicalize(directory)?;
    if canonical.parent().is_none() {
        return Err(invalid_input(
            "collaboration socket parent cannot be a filesystem root",
        ));
    }
    let canonical_metadata = std::fs::symlink_metadata(&canonical)?;
    let identity = FileIdentity::of(&before);
    if FileIdentity::of(&canonical_metadata) != identity
        || !canonical_metadata.is_dir()
        || canonical_metadata.file_type().is_symlink()
    {
        return Err(io::Error::other(
            "collaboration socket parent identity changed during resolution",
        ));
    }
    std::fs::set_permissions(&canonical, std::fs::Permissions::from_mode(0o700))?;
    verify_private_parent(&canonical, identity)?;
    Ok((canonical, identity))
}

fn nearest_existing_ancestor(path: &Path) -> io::Result<PathBuf> {
    let mut candidate = path;
    loop {
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => return Ok(candidate.to_path_buf()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(|| {
                    invalid_input("collaboration socket parent has no existing ancestor")
                })?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn verify_private_parent(path: &Path, identity: FileIdentity) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || FileIdentity::of(&metadata) != identity
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration socket parent identity or permissions changed",
        ));
    }
    Ok(())
}

fn verify_socket_for_close(
    parent: &Path,
    parent_identity: FileIdentity,
    socket: &Path,
    socket_identity: FileIdentity,
) -> io::Result<()> {
    verify_private_parent(parent, parent_identity)?;
    let metadata = match std::fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket()
        || FileIdentity::of(&metadata) != socket_identity
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration socket identity changed before close",
        ));
    }
    Ok(())
}

pub(crate) fn unlink_matching_socket(path: &Path, identity: FileIdentity) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_socket() && FileIdentity::of(&metadata) == identity {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn decode_next_frame(buffer: &[u8]) -> FrameDecode {
    let Some(first_newline) = buffer.iter().position(|byte| *byte == b'\n') else {
        return if buffer.len() > FRAME_BYTES {
            FrameDecode::Fatal {
                framing: Framing::Newline,
                code: "MESSAGE_TOO_LARGE",
            }
        } else {
            FrameDecode::NeedMore
        };
    };
    let line_end = if first_newline > 0 && buffer[first_newline - 1] == b'\r' {
        first_newline - 1
    } else {
        first_newline
    };
    let first_line = String::from_utf8_lossy(&buffer[..line_end]);
    if !first_line
        .split_once(':')
        .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
    {
        if first_newline > FRAME_BYTES {
            return FrameDecode::Recoverable {
                framing: Framing::Newline,
                code: "MESSAGE_TOO_LARGE",
                consumed: first_newline + 1,
            };
        }
        return FrameDecode::Frame(
            DecodedFrame {
                framing: Framing::Newline,
                payload: buffer[..line_end].to_vec(),
            },
            first_newline + 1,
        );
    }

    let Some((header_end, body_offset)) = find_header_end(buffer) else {
        return if buffer.len() > HEADER_BYTES {
            FrameDecode::Fatal {
                framing: Framing::ContentLength,
                code: "INVALID_FRAMING",
            }
        } else {
            FrameDecode::NeedMore
        };
    };
    if body_offset > HEADER_BYTES {
        return FrameDecode::Fatal {
            framing: Framing::ContentLength,
            code: "INVALID_FRAMING",
        };
    }
    let header = String::from_utf8_lossy(&buffer[..header_end]);
    let lengths = header
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .collect::<Vec<_>>();
    if lengths.len() != 1
        || lengths[0].is_empty()
        || !lengths[0].bytes().all(|b| b.is_ascii_digit())
    {
        return FrameDecode::Fatal {
            framing: Framing::ContentLength,
            code: "INVALID_FRAMING",
        };
    }
    let Ok(length) = lengths[0].parse::<usize>() else {
        return FrameDecode::Fatal {
            framing: Framing::ContentLength,
            code: "MESSAGE_TOO_LARGE",
        };
    };
    if length > FRAME_BYTES {
        return FrameDecode::Fatal {
            framing: Framing::ContentLength,
            code: "MESSAGE_TOO_LARGE",
        };
    }
    if buffer.len() < body_offset + length {
        return FrameDecode::NeedMore;
    }
    FrameDecode::Frame(
        DecodedFrame {
            framing: Framing::ContentLength,
            payload: buffer[body_offset..body_offset + length].to_vec(),
        },
        body_offset + length,
    )
}

fn find_header_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    match (crlf, lf) {
        (Some(crlf), Some(lf)) if crlf <= lf => Some((crlf, crlf + 4)),
        (Some(_) | None, Some(lf)) => Some((lf, lf + 2)),
        (Some(crlf), None) => Some((crlf, crlf + 4)),
        (None, None) => None,
    }
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn lock_std<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
