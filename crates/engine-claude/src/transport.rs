use std::{
    collections::HashMap,
    ffi::OsStr,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use interface_engine::provider_search_path;
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, Command},
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tracing::debug;
use uuid::Uuid;

use crate::{
    Initialization, LaunchOptions,
    protocol::{DEFAULT_MAX_FRAME_BYTES, HostRequest, NdjsonDecoder, ProviderFrame, UserMessage},
};

const EVENT_BUFFER: usize = 5_000;
const WRITE_BUFFER: usize = 128;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
const GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(2);

type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, RequestError>>>>>;
type PendingHostRequests = Arc<Mutex<HashMap<String, PendingHostRequest>>>;
type LatestCommands = Arc<Mutex<Option<Value>>>;

#[derive(Clone)]
struct PendingHostRequest {
    request: Arc<HostRequest>,
    responding: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryCertainty {
    DefinitelyUnsent,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{message}")]
pub struct DeliveryError {
    pub certainty: DeliveryCertainty,
    pub message: String,
}

impl DeliveryError {
    fn definitely_unsent(message: impl Into<String>) -> Self {
        Self {
            certainty: DeliveryCertainty::DefinitelyUnsent,
            message: message.into(),
        }
    }

    fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            certainty: DeliveryCertainty::OutcomeUnknown,
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error(transparent)]
    Delivery(#[from] DeliveryError),
    #[error("Claude rejected the control request: {0}")]
    Provider(String),
    #[error("Claude closed before answering the control request")]
    Closed,
    #[error(
        "Claude did not answer the control request before its deadline; its outcome is unknown"
    )]
    TimedOut,
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("invalid Claude launch: {0}")]
    Invalid(#[from] crate::launch::LaunchSpecError),
    #[error("could not start Claude: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Claude did not initialize: {0}")]
    Initialize(#[source] RequestError),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeEvent {
    Provider(Value),
    HostRequest(Arc<HostRequest>),
    HostRequestCancelled { request_id: String },
    TransportError { message: String },
    ProcessExited { success: bool, code: Option<i32> },
}

#[derive(Debug, Error)]
pub enum ReceiveError {
    #[error("Claude's event stream closed")]
    Closed,
    #[error("the consumer fell behind and {skipped} older Claude events were discarded")]
    Lagged { skipped: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostResponse {
    Allow { updated_input: Option<Value> },
    Deny { message: String },
}

enum WriterCommand {
    Write {
        bytes: Vec<u8>,
        receipt: oneshot::Sender<Result<(), DeliveryError>>,
    },
    Close {
        receipt: oneshot::Sender<()>,
    },
}

/// The single consumer's bounded event queue. A full queue never blocks the
/// reader (it must keep answering control requests); the frames it could not
/// queue are counted, and the consumer is told it lagged.
#[derive(Clone)]
struct EventSender {
    queue: mpsc::Sender<ClaudeEvent>,
    skipped: Arc<AtomicU64>,
}

impl EventSender {
    fn send(&self, event: ClaudeEvent) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.queue.try_send(event) {
            self.skipped.fetch_add(1, Ordering::AcqRel);
        }
    }
}

pub struct ClaudeSession {
    session_id: String,
    writer: mpsc::Sender<WriterCommand>,
    events: mpsc::Receiver<ClaudeEvent>,
    skipped_events: Arc<AtomicU64>,
    pending_requests: PendingRequests,
    pending_host_requests: PendingHostRequests,
    latest_commands: LatestCommands,
    closed: Arc<AtomicBool>,
    shutdown: Option<oneshot::Sender<()>>,
    tasks: Vec<JoinHandle<()>>,
    /// The `claude` child's pid as spawned (`None` if it had already exited).
    process_id: Option<u32>,
}

impl ClaudeSession {
    /// The `claude` process's pid as spawned, for naming it to a team tools
    /// binding.
    #[must_use]
    pub const fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Spawns a control-only or live session and retains Claude's initialize
    /// response for model/command catalogue discovery.
    ///
    /// # Errors
    ///
    /// Returns an error when launch validation, process spawn, or provider
    /// initialization fails.
    pub async fn launch_with_initialization(
        options: LaunchOptions,
    ) -> Result<(Self, Value), LaunchError> {
        let initialization = options.initialization.clone();
        let mut session = Self::spawn_uninitialized(&options)?;
        match session.initialize(&initialization).await {
            Ok(response) => Ok((session, response)),
            Err(error) => {
                session.close().await;
                Err(LaunchError::Initialize(error))
            }
        }
    }

    fn spawn_uninitialized(options: &LaunchOptions) -> Result<Self, LaunchError> {
        let spec = options.build_spec()?;
        let mut command = Command::new(spec.program());
        command
            .args(spec.arguments())
            .current_dir(spec.cwd())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_claude_environment(&mut command, spec.program()).map_err(|error| {
            LaunchError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
        command.envs(
            options
                .environment
                .iter()
                .map(|(name, value)| (name, value)),
        );

        let mut child = command.spawn().map_err(LaunchError::Spawn)?;
        let process_id = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            LaunchError::Spawn(std::io::Error::other("Claude stdin pipe was not created"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            LaunchError::Spawn(std::io::Error::other("Claude stdout pipe was not created"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            LaunchError::Spawn(std::io::Error::other("Claude stderr pipe was not created"))
        })?;

        let (writer, writer_rx) = mpsc::channel(WRITE_BUFFER);
        // One consumer: an mpsc queue allocates as it fills and moves each
        // event out, where a broadcast ring was built up front and cloned
        // every frame on receive.
        let (queue, events) = mpsc::channel(EVENT_BUFFER);
        let skipped_events = Arc::new(AtomicU64::new(0));
        let event_tx = EventSender {
            queue,
            skipped: Arc::clone(&skipped_events),
        };
        let pending_requests = Arc::new(Mutex::new(HashMap::new()));
        let pending_host_requests = Arc::new(Mutex::new(HashMap::new()));
        let latest_commands = Arc::new(Mutex::new(None));
        let closed = Arc::new(AtomicBool::new(false));
        let (shutdown, shutdown_rx) = oneshot::channel();

        let tasks = vec![
            tokio::spawn(writer_loop(stdin, writer_rx)),
            tokio::spawn(reader_loop(
                stdout,
                event_tx.clone(),
                pending_requests.clone(),
                pending_host_requests.clone(),
                latest_commands.clone(),
                closed.clone(),
            )),
            tokio::spawn(drain_stderr(stderr)),
            tokio::spawn(supervise_process(
                child,
                shutdown_rx,
                event_tx,
                pending_requests.clone(),
                closed.clone(),
            )),
        ];

        Ok(Self {
            session_id: options.target.session_id().to_owned(),
            writer,
            events,
            skipped_events,
            pending_requests,
            pending_host_requests,
            latest_commands,
            closed,
            shutdown: Some(shutdown),
            tasks,
            process_id,
        })
    }

    /// Waits for the next provider or host-interaction event.
    ///
    /// # Errors
    ///
    /// Returns `Closed` after shutdown or `Lagged` when a stalled consumer lost
    /// older events from the bounded queue.
    pub async fn next_event(&mut self) -> Result<ClaudeEvent, ReceiveError> {
        let skipped = self.skipped_events.swap(0, Ordering::AcqRel);
        if skipped > 0 {
            return Err(ReceiveError::Lagged { skipped });
        }
        self.events.recv().await.ok_or(ReceiveError::Closed)
    }

    /// Sends one user turn on the long-lived streaming input channel.
    ///
    /// # Errors
    ///
    /// Returns a delivery error whose certainty states whether a retry is safe.
    pub async fn send_user(&self, text: impl Into<String>) -> Result<(), DeliveryError> {
        self.write_json(&UserMessage::text(&self.session_id, text))
            .await
    }

    /// Stops the active turn and cancels queued user messages where supported.
    ///
    /// # Errors
    ///
    /// Returns an error for delivery failure, provider rejection, shutdown, or
    /// an acknowledgement timeout.
    pub async fn interrupt(&self) -> Result<Value, RequestError> {
        self.request(
            json!({ "subtype": "interrupt", "cancel_queued": true }),
            CONTROL_TIMEOUT,
        )
        .await
    }

    /// Sends a provider control request without enqueueing a model turn.
    ///
    /// # Errors
    ///
    /// Returns an error for delivery failure, provider rejection, shutdown, or
    /// acknowledgement timeout.
    pub async fn control_request(
        &self,
        request: Value,
        deadline: Duration,
    ) -> Result<Value, RequestError> {
        self.request(request, deadline).await
    }

    pub(crate) fn control(&self) -> ClaudeControl {
        ClaudeControl {
            writer: self.writer.clone(),
            pending_requests: self.pending_requests.clone(),
            latest_commands: self.latest_commands.clone(),
            closed: self.closed.clone(),
        }
    }

    /// Changes the live session model; `None` restores the provider default.
    ///
    /// # Errors
    ///
    /// Returns an error when the control request is not acknowledged.
    pub async fn set_model(&self, model: Option<&str>) -> Result<(), RequestError> {
        let mut request = Map::from_iter([("subtype".into(), Value::String("set_model".into()))]);
        if let Some(model) = model {
            request.insert("model".into(), Value::String(model.to_owned()));
        }
        self.request(Value::Object(request), CONTROL_TIMEOUT)
            .await
            .map(drop)
    }

    /// Changes a live session's permission mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider rejects the mode or the request is
    /// not acknowledged. Bypass mode is intentionally launch-only.
    pub async fn set_permission_mode(
        &self,
        mode: crate::PermissionMode,
    ) -> Result<(), RequestError> {
        if mode == crate::PermissionMode::BypassPermissions {
            return Err(RequestError::Delivery(DeliveryError::definitely_unsent(
                "bypassPermissions can only be selected at launch",
            )));
        }
        self.request(
            json!({ "subtype": "set_permission_mode", "mode": mode.as_str() }),
            CONTROL_TIMEOUT,
        )
        .await
        .map(drop)
    }

    /// Merges provider flag settings such as effort and fast mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the control request is not acknowledged.
    pub async fn apply_flag_settings(&self, settings: Value) -> Result<(), RequestError> {
        self.request(
            json!({ "subtype": "apply_flag_settings", "settings": settings }),
            CONTROL_TIMEOUT,
        )
        .await
        .map(drop)
    }

    /// Settles a pending provider-to-host request exactly once.
    ///
    /// # Errors
    ///
    /// Returns a delivery error when the request is stale or the response could
    /// not be written. Outcome-unknown responses are removed to prevent an
    /// unsafe replay.
    pub async fn respond_to_host_request(
        &self,
        request_id: &str,
        response: HostResponse,
    ) -> Result<(), DeliveryError> {
        // Only the tool id leaves the guard; the request itself stays shared.
        let tool_use_id = {
            let mut pending = self.pending_host_requests.lock().await;
            let entry = pending.get_mut(request_id).ok_or_else(|| {
                DeliveryError::definitely_unsent("host request is no longer pending")
            })?;
            if entry.responding {
                return Err(DeliveryError::definitely_unsent(
                    "host request response is already in progress",
                ));
            }
            entry.responding = true;
            entry.request.tool_use_id().map(ToOwned::to_owned)
        };

        let frame = match response {
            HostResponse::Allow { updated_input } => {
                let mut body = Map::from_iter([("behavior".into(), Value::String("allow".into()))]);
                if let Some(input) = updated_input {
                    body.insert("updatedInput".into(), input);
                }
                if let Some(tool_use_id) = &tool_use_id {
                    body.insert("toolUseID".into(), Value::String(tool_use_id.clone()));
                }
                ProviderFrame::control_success(request_id, &Value::Object(body))
            }
            HostResponse::Deny { message } => {
                let mut body = Map::from_iter([
                    ("behavior".into(), Value::String("deny".into())),
                    ("message".into(), Value::String(message)),
                ]);
                if let Some(tool_use_id) = &tool_use_id {
                    body.insert("toolUseID".into(), Value::String(tool_use_id.clone()));
                }
                ProviderFrame::control_success(request_id, &Value::Object(body))
            }
        };

        let delivery = self.write_json(&frame).await;
        match &delivery {
            Ok(())
            | Err(DeliveryError {
                certainty: DeliveryCertainty::OutcomeUnknown,
                ..
            }) => {
                self.pending_host_requests.lock().await.remove(request_id);
            }
            Err(DeliveryError {
                certainty: DeliveryCertainty::DefinitelyUnsent,
                ..
            }) => {
                if let Some(entry) = self.pending_host_requests.lock().await.get_mut(request_id) {
                    entry.responding = false;
                }
            }
        }
        delivery
    }

    #[must_use]
    pub async fn host_request(&self, request_id: &str) -> Option<Arc<HostRequest>> {
        self.pending_host_requests
            .lock()
            .await
            .get(request_id)
            .map(|entry| Arc::clone(&entry.request))
    }

    pub async fn close(&mut self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let (receipt, received) = oneshot::channel();
        if self
            .writer
            .send(WriterCommand::Close { receipt })
            .await
            .is_ok()
        {
            let _ = timeout(GRACEFUL_SHUTDOWN, received).await;
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // One shared deadline for every task, and a task still running at it
        // is aborted rather than detached (draining empties `tasks`, so Drop
        // would find nothing left to abort).
        let deadline = Instant::now() + GRACEFUL_SHUTDOWN + Duration::from_secs(1);
        for mut task in self.tasks.drain(..) {
            if timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
            }
        }
        fail_pending(&self.pending_requests).await;
        self.pending_host_requests.lock().await.clear();
    }

    async fn initialize(&self, initialization: &Initialization) -> Result<Value, RequestError> {
        let mut request = serde_json::to_value(initialization)
            .map_err(|error| DeliveryError::definitely_unsent(error.to_string()))?;
        let object = request.as_object_mut().ok_or_else(|| {
            DeliveryError::definitely_unsent("initialization payload was not an object")
        })?;
        object.insert("subtype".into(), Value::String("initialize".into()));
        let initialized = self.request(request, INITIALIZE_TIMEOUT).await?;
        if let Some(commands) = initialized.get("commands").and_then(Value::as_array) {
            *self.latest_commands.lock().await = Some(Value::Array(commands.clone()));
        }
        Ok(initialized)
    }

    async fn request(&self, request: Value, deadline: Duration) -> Result<Value, RequestError> {
        self.control().request(request, deadline).await
    }

    async fn write_json<T: serde::Serialize + ?Sized>(
        &self,
        value: &T,
    ) -> Result<(), DeliveryError> {
        write_json(&self.writer, &self.closed, value).await
    }
}

#[derive(Clone)]
pub(crate) struct ClaudeControl {
    writer: mpsc::Sender<WriterCommand>,
    pending_requests: PendingRequests,
    latest_commands: LatestCommands,
    closed: Arc<AtomicBool>,
}

impl ClaudeControl {
    pub(crate) async fn get_context_usage(&self) -> Result<Value, RequestError> {
        self.request(json!({ "subtype": "get_context_usage" }), CONTROL_TIMEOUT)
            .await
    }

    pub(crate) async fn supported_commands(&self) -> Option<Value> {
        self.latest_commands.lock().await.clone()
    }

    async fn request(&self, request: Value, deadline: Duration) -> Result<Value, RequestError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RequestError::Delivery(DeliveryError::definitely_unsent(
                "Claude is not running",
            )));
        }
        let request_id = Uuid::new_v4().to_string();
        let (answer, receiver) = oneshot::channel();
        self.pending_requests
            .lock()
            .await
            .insert(request_id.clone(), answer);
        let frame = ProviderFrame::control_request(&request_id, &request);
        if let Err(error) = write_json(&self.writer, &self.closed, &frame).await {
            self.pending_requests.lock().await.remove(&request_id);
            return Err(RequestError::Delivery(error));
        }
        match timeout(deadline, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(RequestError::Closed),
            Err(_) => {
                self.pending_requests.lock().await.remove(&request_id);
                Err(RequestError::TimedOut)
            }
        }
    }
}

async fn write_json<T: serde::Serialize + ?Sized>(
    writer: &mpsc::Sender<WriterCommand>,
    closed: &AtomicBool,
    value: &T,
) -> Result<(), DeliveryError> {
    if closed.load(Ordering::Acquire) {
        return Err(DeliveryError::definitely_unsent("Claude is not running"));
    }
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| DeliveryError::definitely_unsent(error.to_string()))?;
    if bytes.len() >= DEFAULT_MAX_FRAME_BYTES {
        return Err(DeliveryError::definitely_unsent(
            "outbound Claude frame exceeded the protocol limit",
        ));
    }
    bytes.push(b'\n');
    let (receipt, received) = oneshot::channel();
    writer
        .send(WriterCommand::Write { bytes, receipt })
        .await
        .map_err(|_| DeliveryError::definitely_unsent("Claude's input transport is closed"))?;
    received.await.unwrap_or_else(|_| {
        Err(DeliveryError::outcome_unknown(
            "Claude's input transport closed during delivery",
        ))
    })
}

/// Claude Code stamps every process it spawns as a nested session, and a
/// nested Claude runs with transcript saving off, so nothing it does could be
/// resumed. `iface` runs this crate in-process and is routinely started from
/// inside an agent session, so every Claude spawn here drops the stamp.
/// Mirrors `CLAUDE_NESTING_MARKERS` in `electron/src/main/rust-host-client.ts`
/// minus `CLAUDE_CODE_ENTRYPOINT`: that one only names the embedding surface,
/// and an inherited value stays authoritative as it always has.
pub(crate) const CLAUDE_NESTING_MARKERS: [&str; 8] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// Gives a Claude child the host's environment the way every Claude spawn
/// here does: the CLI's toolchain on `PATH`, no `NODE_OPTIONS`, no nesting
/// markers, and the embedding surface named unless inherited.
pub(crate) fn configure_claude_environment(
    command: &mut Command,
    executable: &Path,
) -> Result<(), std::env::JoinPathsError> {
    configure_child_environment(
        command,
        executable,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref().map(Path::new),
        std::env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some(),
        std::env::var_os("CLAUDE_CODE_DISABLE_TERMINAL_TITLE").is_some(),
        std::env::var_os("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").is_some(),
    )
}

fn configure_child_environment(
    command: &mut Command,
    executable: &Path,
    current_path: Option<&OsStr>,
    home: Option<&Path>,
    entrypoint_present: bool,
    terminal_title_present: bool,
    autocompact_present: bool,
) -> Result<(), std::env::JoinPathsError> {
    let search_path = provider_search_path(executable, current_path, home)?;
    command
        // Do not inspect or serialize inherited authentication. The CLI is the
        // credential boundary; these entries only identify the embedding
        // surface, make subprocess lookup reliable, and remove a Node-only
        // injection vector.
        .env("PATH", search_path)
        .env_remove("NODE_OPTIONS");
    for marker in CLAUDE_NESTING_MARKERS {
        command.env_remove(marker);
    }
    if !entrypoint_present {
        command.env("CLAUDE_CODE_ENTRYPOINT", "interface-rust");
    }
    if !terminal_title_present {
        command.env("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1");
    }
    if !autocompact_present {
        command.env("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "75");
    }
    Ok(())
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let (receipt, _received) = oneshot::channel();
        let _ = self.writer.try_send(WriterCommand::Close { receipt });
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn writer_loop(mut stdin: ChildStdin, mut commands: mpsc::Receiver<WriterCommand>) {
    while let Some(command) = commands.recv().await {
        match command {
            WriterCommand::Write { bytes, receipt } => {
                let result = async {
                    stdin.write_all(&bytes).await?;
                    stdin.flush().await
                }
                .await
                .map_err(|error| {
                    DeliveryError::outcome_unknown(format!("Claude input write failed: {error}"))
                });
                let failed = result.is_err();
                let _ = receipt.send(result);
                if failed {
                    break;
                }
            }
            WriterCommand::Close { receipt } => {
                let _ = stdin.shutdown().await;
                let _ = receipt.send(());
                break;
            }
        }
    }
}

async fn reader_loop<R>(
    mut stdout: R,
    events: EventSender,
    pending_requests: PendingRequests,
    pending_host_requests: PendingHostRequests,
    latest_commands: LatestCommands,
    closed: Arc<AtomicBool>,
) where
    R: AsyncRead + Unpin,
{
    let mut decoder = NdjsonDecoder::default();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        match stdout.read(&mut chunk).await {
            Ok(0) => {
                if let Ok(Some(value)) = decoder.finish() {
                    route_frame(
                        value,
                        &events,
                        &pending_requests,
                        &pending_host_requests,
                        &latest_commands,
                    )
                    .await;
                }
                break;
            }
            Ok(read) => {
                let (values, error) = decoder.push_frames(&chunk[..read]);
                for value in values {
                    route_frame(
                        value,
                        &events,
                        &pending_requests,
                        &pending_host_requests,
                        &latest_commands,
                    )
                    .await;
                }
                if let Some(error) = error {
                    events.send(ClaudeEvent::TransportError {
                        message: error.to_string(),
                    });
                    break;
                }
            }
            Err(error) => {
                events.send(ClaudeEvent::TransportError {
                    message: format!("Claude output read failed: {error}"),
                });
                break;
            }
        }
    }
    closed.store(true, Ordering::Release);
    fail_pending(&pending_requests).await;
}

async fn route_frame(
    value: Value,
    events: &EventSender,
    pending_requests: &PendingRequests,
    pending_host_requests: &PendingHostRequests,
    latest_commands: &LatestCommands,
) {
    match ProviderFrame::decode(value) {
        Ok(ProviderFrame::ControlResponse(response)) => {
            let pending = pending_requests.lock().await.remove(&response.request_id);
            if let Some(pending) = pending {
                let answer = if response.is_success() {
                    Ok(response.response.unwrap_or(Value::Null))
                } else {
                    Err(RequestError::Provider(
                        response
                            .error
                            .unwrap_or_else(|| "unknown provider error".into()),
                    ))
                };
                let _ = pending.send(answer);
            }
        }
        Ok(ProviderFrame::ControlRequest(request)) => {
            let request = Arc::new(request);
            pending_host_requests.lock().await.insert(
                request.request_id.clone(),
                PendingHostRequest {
                    request: Arc::clone(&request),
                    responding: false,
                },
            );
            events.send(ClaudeEvent::HostRequest(request));
        }
        Ok(ProviderFrame::ControlCancelled { request_id }) => {
            pending_host_requests.lock().await.remove(&request_id);
            events.send(ClaudeEvent::HostRequestCancelled { request_id });
        }
        Ok(ProviderFrame::KeepAlive) => {}
        Ok(ProviderFrame::Event(value)) => {
            if value.get("type").and_then(Value::as_str) == Some("system")
                && value.get("subtype").and_then(Value::as_str) == Some("commands_changed")
                && let Some(commands) = value.get("commands").and_then(Value::as_array)
            {
                *latest_commands.lock().await = Some(Value::Array(commands.clone()));
            }
            events.send(ClaudeEvent::Provider(value));
        }
        Err(error) => {
            events.send(ClaudeEvent::TransportError {
                message: error.to_string(),
            });
        }
    }
}

async fn supervise_process(
    mut child: tokio::process::Child,
    mut shutdown: oneshot::Receiver<()>,
    events: EventSender,
    pending_requests: PendingRequests,
    closed: Arc<AtomicBool>,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        _ = &mut shutdown => {
            if let Ok(status) = timeout(GRACEFUL_SHUTDOWN, child.wait()).await {
                status
            } else {
                let _ = child.start_kill();
                child.wait().await
            }
        }
    };
    closed.store(true, Ordering::Release);
    fail_pending(&pending_requests).await;
    match status {
        Ok(status) => {
            events.send(ClaudeEvent::ProcessExited {
                success: status.success(),
                code: status.code(),
            });
        }
        Err(error) => {
            events.send(ClaudeEvent::TransportError {
                message: format!("could not wait for Claude: {error}"),
            });
        }
    }
}

async fn drain_stderr<R>(mut stderr: R)
where
    R: AsyncRead + Unpin,
{
    // Stderr can contain provider-generated text. Drain it to prevent child
    // deadlock, but never mirror or persist it where secrets could leak.
    let mut buffer = [0_u8; 8 * 1024];
    let mut bytes = 0_u64;
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes = bytes.saturating_add(read as u64),
        }
    }
    debug!(
        bytes,
        "drained Claude stderr without retaining its contents"
    );
}

async fn fail_pending(pending: &PendingRequests) {
    let requests = {
        let mut pending = pending.lock().await;
        pending
            .drain()
            .map(|(_, sender)| sender)
            .collect::<Vec<_>>()
    };
    for request in requests {
        let _ = request.send(Err(RequestError::Closed));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::*;
    use crate::{PermissionMode, SessionTarget};

    fn command_environment(command: &Command) -> HashMap<String, Option<String>> {
        command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn child_environment_prepends_the_exact_claude_toolchain_and_absent_defaults() {
        let mut command = Command::new("/resolved/bin/claude");
        configure_child_environment(
            &mut command,
            Path::new("/resolved/bin/claude"),
            Some(OsStr::new("/usr/bin:/usr/local/bin")),
            Some(Path::new("/Users/test")),
            false,
            false,
            false,
        )
        .unwrap();

        let environment = command_environment(&command);
        assert_eq!(
            environment.get("PATH").and_then(Option::as_deref),
            Some("/resolved/bin:/Users/test/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin")
        );
        assert_eq!(
            environment
                .get("CLAUDE_CODE_DISABLE_TERMINAL_TITLE")
                .and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            environment
                .get("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
                .and_then(Option::as_deref),
            Some("75")
        );
        assert_eq!(environment.get("NODE_OPTIONS"), Some(&None));
        for marker in CLAUDE_NESTING_MARKERS {
            assert_eq!(environment.get(marker), Some(&None), "{marker} is removed");
        }
        assert_eq!(
            environment
                .get("CLAUDE_CODE_ENTRYPOINT")
                .and_then(Option::as_deref),
            Some("interface-rust")
        );
    }

    #[test]
    fn nesting_markers_match_the_desktop_host_scrub_minus_the_entrypoint() {
        let electron = include_str!("../../../electron/src/main/rust-host-client.ts");
        let list = electron
            .split_once("const CLAUDE_NESTING_MARKERS = [")
            .and_then(|(_, rest)| rest.split_once("] as const;"))
            .map(|(list, _)| list)
            .expect("the desktop host scrub list");
        let mut desktop = list
            .split(',')
            .map(|name| name.trim().trim_matches('"'))
            .filter(|name| !name.is_empty() && *name != "CLAUDE_CODE_ENTRYPOINT")
            .collect::<Vec<_>>();
        let mut rust = CLAUDE_NESTING_MARKERS.to_vec();
        desktop.sort_unstable();
        rust.sort_unstable();
        assert_eq!(rust, desktop);
    }

    #[test]
    fn child_environment_does_not_override_existing_budget_values() {
        let mut command = Command::new("/resolved/bin/claude");
        configure_child_environment(
            &mut command,
            Path::new("/resolved/bin/claude"),
            None,
            Some(Path::new("/Users/test")),
            true,
            true,
            true,
        )
        .unwrap();

        let environment = command_environment(&command);
        assert!(!environment.contains_key("CLAUDE_CODE_DISABLE_TERMINAL_TITLE"));
        assert!(!environment.contains_key("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"));
        assert!(!environment.contains_key("CLAUDE_CODE_ENTRYPOINT"));
    }

    #[tokio::test]
    async fn fake_cli_completes_initialize_user_and_permission_round_trips() {
        let directory = tempdir().expect("temporary directory");
        let executable = directory.path().join("fake-claude");
        fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[],"agents":[],"models":[]}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"control_request","request_id":"approval-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"pwd"}}}'
      ;;
    *'"type":"control_response"'*)
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"fake"}'
      ;;
  esac
done
"#,
        )
        .expect("write fake CLI");
        let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("executable mode");

        let mut options = LaunchOptions::new(
            &executable,
            directory.path(),
            SessionTarget::New {
                session_id: "fake-session".into(),
            },
        );
        options.permission_mode = PermissionMode::Default;
        options.persist_session = false;

        let mut session = ClaudeSession::launch_with_initialization(options)
            .await
            .expect("launch")
            .0;
        session
            .send_user("run pwd")
            .await
            .expect("send user message");
        let request = match session.next_event().await.expect("permission event") {
            ClaudeEvent::HostRequest(request) => request,
            event => panic!("expected host request, received {event:?}"),
        };
        assert_eq!(request.tool_name(), Some("Bash"));
        session
            .respond_to_host_request(
                &request.request_id,
                HostResponse::Allow {
                    updated_input: None,
                },
            )
            .await
            .expect("approval response");
        let result = match session.next_event().await.expect("result event") {
            ClaudeEvent::Provider(value) => value,
            event => panic!("expected provider event, received {event:?}"),
        };
        assert_eq!(result["result"], "ok");
        session.close().await;
    }

    #[tokio::test]
    async fn close_shares_one_deadline_when_a_grandchild_holds_the_pipes() {
        let directory = tempdir().expect("temporary directory");
        let executable = directory.path().join("leaks-pipes");
        // The background sleep inherits stdout and stderr, so neither the
        // reader nor the stderr drain sees EOF after the CLI itself exits.
        fs::write(
            &executable,
            "#!/bin/sh\nsleep 10 &\nwhile IFS= read -r line; do :; done\n",
        )
        .expect("write fake CLI");
        let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("executable mode");

        let options = LaunchOptions::new(
            &executable,
            directory.path(),
            SessionTarget::New {
                session_id: "leaky-session".into(),
            },
        );
        let mut session = ClaudeSession::spawn_uninitialized(&options).expect("spawn");
        let started = std::time::Instant::now();
        session.close().await;
        // Two hung tasks cost 3 s each when joined one after another; one
        // shared deadline bounds the whole close near 3 s.
        assert!(
            started.elapsed() < Duration::from_millis(5_000),
            "close took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_closed_transport_reports_definitely_unsent() {
        let directory = tempdir().expect("temporary directory");
        let executable = directory.path().join("exits-immediately");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write fake CLI");
        let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("executable mode");

        let options = LaunchOptions::new(
            &executable,
            directory.path(),
            SessionTarget::New {
                session_id: "closed-session".into(),
            },
        );
        let mut session = ClaudeSession::spawn_uninitialized(&options).expect("spawn");
        loop {
            if matches!(
                session.next_event().await.expect("exit event"),
                ClaudeEvent::ProcessExited { .. }
            ) {
                break;
            }
        }
        let error = session
            .send_user("never delivered")
            .await
            .expect_err("closed");
        assert_eq!(error.certainty, DeliveryCertainty::DefinitelyUnsent);
    }
}
