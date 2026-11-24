use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::ErrorKind,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use interface_contracts::canonical_timestamp;
use interface_storage::OWNER_FILE_MODE;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, Notify, OwnedMutexGuard, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    agent_client::AGENT_MAILBOX_MAX_PAGE,
    agent_protocol::{
        AGENT_MAX_LINE_BYTES, AGENT_MAX_PROMPT_BYTES, AGENT_RESULT_BUDGET_BYTES, AgentCapabilities,
        AgentErrorCode, AgentFailure, AgentMethod, AgentProtocolError, AgentRequest, AgentResponse,
        AgentResponsePayload, AgentSnapshot, AgentStatus, AgentTaskMetadata, decode_agent_request,
        encode_agent_response, encoded_len, is_agent_name,
    },
    argv::LaunchScope,
    sanitize::for_display,
};

const AGENT_MAILBOX_VERSION: u8 = 1;
const AGENT_MAILBOX_MAX_MESSAGES: usize = 10_000;
/// Settled messages the mailbox keeps: at most this many of the newest, and
/// none settled more than `AGENT_MAILBOX_RETENTION_DAYS` ago.
const AGENT_MAILBOX_KEPT_SETTLED: usize = 2_000;
const AGENT_MAILBOX_RETENTION_DAYS: i64 = 14;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionState {
    pub key: String,
    pub engine: String,
    pub session_id: String,
    pub status: AgentStatus,
    pub cwd: String,
    pub title: String,
    pub model: Option<String>,
    pub task: Option<AgentTaskMetadata>,
    pub capabilities: Option<AgentCapabilities>,
    pub live: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentDeliveryKind {
    DefinitelyUnsent,
    AdapterAccepted,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentDelivery {
    pub kind: AgentDeliveryKind,
    pub message: Option<String>,
    pub queued: bool,
}

impl AgentDelivery {
    #[must_use]
    pub const fn accepted(queued: bool) -> Self {
        Self {
            kind: AgentDeliveryKind::AdapterAccepted,
            message: None,
            queued,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StartAgentInput {
    pub name: String,
    pub prompt: Option<String>,
    pub engine: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub scope: Option<LaunchScope>,
    pub task: Option<AgentTaskMetadata>,
}

#[async_trait]
#[allow(clippy::missing_errors_doc)]
pub trait AgentController: Send + Sync + 'static {
    fn agent_sessions(&self) -> Vec<AgentSessionState>;
    fn agent_session(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError>;
    fn subscribe_agent_state(&self) -> watch::Receiver<u64>;
    fn read_agent(&self, key: &str, lines: usize) -> Result<Vec<String>, AgentProtocolError>;
    async fn send_agent(
        &self,
        key: &str,
        text: String,
    ) -> Result<AgentDelivery, AgentProtocolError>;
    async fn steer_agent(
        &self,
        key: &str,
        text: String,
    ) -> Result<AgentDelivery, AgentProtocolError>;
    async fn stop_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError>;
    async fn resume_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError>;
    async fn start_agent(
        &self,
        input: StartAgentInput,
    ) -> Result<AgentSessionState, AgentProtocolError>;
    async fn focus_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError>;
}

#[derive(Debug)]
pub struct AgentServer {
    socket_path: PathBuf,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl AgentServer {
    /// Stops the server task and removes its socket path.
    ///
    /// # Errors
    ///
    /// Returns an error if the server task cannot be joined or the socket
    /// cannot be removed.
    pub async fn close(mut self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.await.map_err(|error| error.to_string())?;
        }
        match fs::remove_file(&self.socket_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

struct ServerState {
    controller: Arc<dyn AgentController>,
    names: Mutex<AgentNameStore>,
    mailbox: Mutex<AgentMailbox>,
    mailbox_changed: Notify,
    /// Held by a start from its name check to its bind, and by a rename, so
    /// one name cannot be bound twice.
    naming: Mutex<()>,
    sessions: SessionLocks,
}

/// One lock per session key for the verbs that change a session (send,
/// steer, stop, resume, focus). They stay ordered on one session, and none
/// waits for another session's: a stop is never queued behind someone else's
/// slow start or resume.
#[derive(Default)]
struct SessionLocks(std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>);

impl SessionLocks {
    async fn lock(&self, key: &str) -> SessionGuard<'_> {
        let lock = Arc::clone(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key.to_owned())
                .or_default(),
        );
        let guard = Arc::clone(&lock).lock_owned().await;
        SessionGuard {
            locks: self,
            key: key.to_owned(),
            lock,
            guard: Some(guard),
        }
    }
}

struct SessionGuard<'a> {
    locks: &'a SessionLocks,
    key: String,
    lock: Arc<Mutex<()>>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        let mut locks = self.locks.0.lock().unwrap_or_else(PoisonError::into_inner);
        // Every handle is cloned under the map lock, so two (the map's and
        // this one) means nobody else holds or waits for this key.
        if Arc::strong_count(&self.lock) == 2 {
            locks.remove(&self.key);
        }
    }
}

/// Starts the owner-private local agent control server.
///
/// # Errors
///
/// Returns an error if paths are unsafe, durable state cannot be loaded, or
/// the Unix listener cannot be bound securely.
pub async fn start_agent_server(
    data_dir: &Path,
    socket_path: &Path,
    names_path: &Path,
    controller: Arc<dyn AgentController>,
) -> Result<AgentServer, String> {
    validate_socket_paths(data_dir, socket_path, names_path)?;
    let data_root = data_dir.to_path_buf();
    tokio::task::spawn_blocking(move || crate::private_write::ensure_private_directory(&data_root))
        .await
        .unwrap_or_else(|error| Err(error.to_string()))?;
    let owner_uid = fs::symlink_metadata(data_dir)
        .await
        .map_err(|error| error.to_string())?
        .uid();
    let mut names = AgentNameStore::new(names_path.to_owned());
    names.load().await?;
    let mut mailbox = AgentMailbox::new(data_dir.join("agent-messages.json"));
    mailbox.load().await?;
    match fs::remove_file(socket_path).await {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let listener = UnixListener::bind(socket_path).map_err(|error| error.to_string())?;
    fs::set_permissions(
        socket_path,
        std::fs::Permissions::from_mode(OWNER_FILE_MODE),
    )
    .await
    .map_err(|error| error.to_string())?;
    let state = Arc::new(ServerState {
        controller,
        names: Mutex::new(names),
        mailbox: Mutex::new(mailbox),
        mailbox_changed: Notify::new(),
        naming: Mutex::new(()),
        sessions: SessionLocks::default(),
    });
    let (shutdown, receiver) = watch::channel(false);
    let task = tokio::spawn(accept_connections(listener, owner_uid, state, receiver));
    Ok(AgentServer {
        socket_path: socket_path.to_owned(),
        shutdown,
        task: Some(task),
    })
}

async fn accept_connections(
    listener: UnixListener,
    owner_uid: u32,
    state: Arc<ServerState>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // A refused accept is usually recoverable (an aborted peer,
                    // momentary fd pressure). Keep serving, but back off so a
                    // persistent failure cannot spin this loop.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                };
                if stream.peer_cred().ok().map(|credential| credential.uid()) != Some(owner_uid) {
                    continue;
                }
                let connection_state = Arc::clone(&state);
                let connection_shutdown = shutdown.clone();
                tokio::spawn(async move {
                    handle_connection(stream, connection_state, connection_shutdown).await;
                });
            }
        }
    }
}

/// How long a connected peer has to deliver its one request line. The CLI
/// writes the whole line as soon as it connects.
const AGENT_REQUEST_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn handle_connection(
    mut socket: UnixStream,
    state: Arc<ServerState>,
    shutdown: watch::Receiver<bool>,
) {
    let mut id = "invalid-request".to_owned();
    let request = tokio::time::timeout(AGENT_REQUEST_READ_TIMEOUT, read_request(&mut socket))
        .await
        .unwrap_or_else(|_| {
            Err(AgentProtocolError::invalid(
                "Agent protocol request could not be read.",
            ))
        });
    let payload = match request {
        Ok(request) => {
            id.clone_from(&request.id);
            match dispatch(&state, request, shutdown).await {
                Ok(result) => AgentResponsePayload::Result(result),
                Err(error) => AgentResponsePayload::Error(failure(error)),
            }
        }
        Err(error) => AgentResponsePayload::Error(failure(error)),
    };
    let response = AgentResponse {
        id: id.clone(),
        payload,
    };
    // A response that cannot be framed (one over 1 MiB) is answered with the
    // encoder's error under the same id. Silence would read as a stopped TUI,
    // which scripts retry.
    let line = encode_agent_response(response).or_else(|error| {
        encode_agent_response(AgentResponse {
            id,
            payload: AgentResponsePayload::Error(failure(error)),
        })
    });
    if let Ok(line) = line {
        let _ = socket.write_all(&line).await;
        let _ = socket.shutdown().await;
    }
}

async fn read_request(socket: &mut UnixStream) -> Result<AgentRequest, AgentProtocolError> {
    let mut input = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = socket.read(&mut chunk).await.map_err(|_| {
            AgentProtocolError::invalid("Agent protocol request could not be read.")
        })?;
        if read == 0 {
            break;
        }
        input.extend_from_slice(&chunk[..read]);
        if input.len() > AGENT_MAX_LINE_BYTES + 1 {
            return Err(AgentProtocolError::invalid(
                "Agent protocol line exceeds the 1 MiB limit.",
            ));
        }
        if let Some(newline) = input.iter().position(|byte| *byte == b'\n') {
            if newline + 1 != input.len() {
                return Err(AgentProtocolError::invalid(
                    "Agent protocol accepts one request per connection.",
                ));
            }
            break;
        }
    }
    decode_agent_request(&input)
}

async fn dispatch(
    state: &Arc<ServerState>,
    request: AgentRequest,
    shutdown: watch::Receiver<bool>,
) -> Result<Value, AgentProtocolError> {
    match request.method {
        AgentMethod::Commands => {
            assert_params(&request.params, &[])?;
            serde_json::to_value(interface_contracts::command_catalog::command_catalog())
                .map_err(serialization_error)
        }
        AgentMethod::List => {
            assert_params(&request.params, &["live"])?;
            let live_only = optional_bool(&request.params, "live")?;
            serde_json::to_value(list_snapshots(state, live_only).await)
                .map_err(serialization_error)
        }
        AgentMethod::Get => {
            let key = target_key(state, &request.params, &["target"]).await?;
            snapshot_value(state, &key).await
        }
        AgentMethod::Read => {
            let key = target_key(state, &request.params, &["target", "lines"]).await?;
            let lines = required_positive_integer(&request.params, "lines")?;
            let value = json!({
                "snapshot": snapshot(state, &key).await?,
                "lines": state.controller.read_agent(&key, lines)?,
            });
            Ok(value)
        }
        AgentMethod::Send | AgentMethod::Steer => {
            let key = target_key(
                state,
                &request.params,
                &[
                    "target",
                    "text",
                    "sender",
                    "messageId",
                    "replyTo",
                    "expectReply",
                ],
            )
            .await?;
            let _session = state.sessions.lock(&key).await;
            let steer = request.method == AgentMethod::Steer;
            deliver_message(state, key, &request.params, steer).await
        }
        AgentMethod::Stop => {
            let key = target_key(state, &request.params, &["target"]).await?;
            let _session = state.sessions.lock(&key).await;
            state.controller.stop_agent(&key).await?;
            snapshot_value(state, &key).await
        }
        AgentMethod::Inbox | AgentMethod::Monitor => {
            let monitor = request.method == AgentMethod::Monitor;
            let (key, after, limit, timeout) = inbox_query(state, &request.params, monitor).await?;
            if monitor {
                monitor_inbox(state, &key, after, limit, timeout, shutdown).await
            } else {
                serde_json::to_value(state.mailbox.lock().await.page(&key, after, limit))
                    .map_err(serialization_error)
            }
        }
        AgentMethod::Wait => {
            let key =
                target_key(state, &request.params, &["target", "status", "timeoutMs"]).await?;
            let desired = AgentStatus::parse_waitable(required_string(&request.params, "status")?)
                .ok_or_else(|| {
                    AgentProtocolError::invalid("status must be idle, working, or blocked.")
                })?;
            let timeout = optional_non_negative_integer(&request.params, "timeoutMs")?;
            serde_json::to_value(wait_for_status(state, &key, desired, timeout, shutdown).await?)
                .map_err(serialization_error)
        }
        AgentMethod::Resume => {
            let key = target_key(state, &request.params, &["target"]).await?;
            let _session = state.sessions.lock(&key).await;
            state.controller.resume_agent(&key).await?;
            snapshot_value(state, &key).await
        }
        AgentMethod::Start => {
            let _naming = state.naming.lock().await;
            start_agent(state, &request.params).await
        }
        AgentMethod::Rename => {
            let _naming = state.naming.lock().await;
            rename_agent(state, &request.params).await
        }
        AgentMethod::Focus => {
            let key = target_key(state, &request.params, &["target"]).await?;
            let _session = state.sessions.lock(&key).await;
            state.controller.focus_agent(&key).await?;
            snapshot_value(state, &key).await
        }
    }
}

/// Refuses parameters outside `allowed`, then resolves the request's `target`
/// to a session key.
async fn target_key(
    state: &ServerState,
    params: &Map<String, Value>,
    allowed: &[&str],
) -> Result<String, AgentProtocolError> {
    assert_params(params, allowed)?;
    resolve_target(state, required_string(params, "target")?).await
}

async fn snapshot_value(state: &ServerState, key: &str) -> Result<Value, AgentProtocolError> {
    serde_json::to_value(snapshot(state, key).await?).map_err(serialization_error)
}

/// The sessions `agent.list` answers with: live and named ones first, then
/// stored (not live, unnamed) ones, each group by name or key. The rows stop
/// where the reply would outgrow the result budget, so a long registry leaves
/// out stored sessions instead of making the reply unsendable. `live_only`
/// leaves out every session that is not live.
async fn list_snapshots(state: &ServerState, live_only: bool) -> Vec<AgentSnapshot> {
    let names = state.names.lock().await;
    let name_of = names
        .state
        .names
        .iter()
        .map(|(name, row)| (row.key.as_str(), name.as_str()))
        .collect::<HashMap<_, _>>();
    // Only sessions the controller holds are listed: a durable name can
    // outlive the session it was bound to.
    let mut rows = state
        .controller
        .agent_sessions()
        .into_iter()
        .filter(|session| session.live || !live_only)
        .map(|session| {
            let name = name_of
                .get(session.key.as_str())
                .map(|name| (*name).to_owned());
            snapshot_from(session, name)
        })
        .collect::<Vec<_>>();
    drop(name_of);
    drop(names);
    let stored = |row: &AgentSnapshot| !row.live && row.name.is_none();
    rows.sort_by(|left, right| {
        stored(left).cmp(&stored(right)).then_with(|| {
            left.name
                .as_deref()
                .unwrap_or(&left.key)
                .cmp(right.name.as_deref().unwrap_or(&right.key))
        })
    });
    // The array's brackets, then each row and its separator.
    let mut bytes = 2_usize;
    let fits = rows
        .iter()
        .take_while(|row| {
            bytes = bytes.saturating_add(encoded_len(row).saturating_add(1));
            bytes <= AGENT_RESULT_BUDGET_BYTES
        })
        .count();
    rows.truncate(fits);
    rows
}

async fn snapshot(state: &ServerState, key: &str) -> Result<AgentSnapshot, AgentProtocolError> {
    let session = state.controller.agent_session(key)?;
    let name = state.names.lock().await.name_for_key(key);
    Ok(snapshot_from(session, name))
}

fn snapshot_from(state: AgentSessionState, name: Option<String>) -> AgentSnapshot {
    let default_capabilities = AgentCapabilities::defaults(state.live, state.status);
    AgentSnapshot {
        name,
        key: state.key,
        engine: state.engine,
        session_id: state.session_id,
        status: if state.live {
            state.status
        } else {
            AgentStatus::NotLive
        },
        cwd: state.cwd,
        title: for_display(&state.title),
        model: state.model.map(|value| for_display(&value)),
        task: state.task,
        capabilities: state.capabilities.unwrap_or(default_capabilities),
        live: state.live,
    }
}

async fn resolve_target(state: &ServerState, target: &str) -> Result<String, AgentProtocolError> {
    let names = state.names.lock().await;
    if let Some(row) = names.state.names.get(target) {
        return Ok(row.key.clone());
    }
    let keys = state
        .controller
        .agent_sessions()
        .into_iter()
        .map(|row| row.key)
        .chain(names.state.names.values().map(|row| row.key.clone()))
        .collect::<BTreeSet<_>>();
    let candidates = keys
        .iter()
        .filter(|key| {
            let session_id = key.split_once('#').map_or(key.as_str(), |(_, id)| id);
            session_id.starts_with(target) || key.starts_with(target)
        })
        .cloned()
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => Err(AgentProtocolError::new(
            AgentErrorCode::NotFound,
            format!("No agent session matches {target}."),
        )),
        [key] => Ok(key.clone()),
        _ => Err(AgentProtocolError::new(
            AgentErrorCode::Ambiguous,
            format!("Target {target} matches more than one session."),
        )
        .with_data(json!({ "candidates": candidates }))),
    }
}

async fn start_agent(
    state: &ServerState,
    params: &Map<String, Value>,
) -> Result<Value, AgentProtocolError> {
    assert_params(
        params,
        &[
            "name", "prompt", "brief", "engine", "cwd", "model", "effort", "scope", "task",
        ],
    )?;
    let name = required_string(params, "name")?;
    if !is_agent_name(name) {
        return Err(AgentProtocolError::invalid(format!(
            "Invalid agent name: {name}."
        )));
    }
    if state.names.lock().await.key_for_name(name).is_some() {
        return Err(AgentProtocolError::new(
            AgentErrorCode::NameTaken,
            format!("Agent name {name} is already in use."),
        ));
    }
    let prompt = optional_string(params, "prompt")?;
    let brief = optional_string(params, "brief")?;
    if prompt.is_some() && brief.is_some() {
        return Err(AgentProtocolError::invalid(
            "agent.start cannot combine prompt with brief.",
        ));
    }
    let prompt = prompt.or(brief).map(str::to_owned);
    if prompt
        .as_ref()
        .is_some_and(|value| value.len() > AGENT_MAX_PROMPT_BYTES)
    {
        return Err(AgentProtocolError::invalid(
            "agent.start prompt is limited to 64 KiB of UTF-8.",
        ));
    }
    let engine = optional_string(params, "engine")?;
    if engine.is_some_and(|value| !valid_engine(value)) {
        return Err(AgentProtocolError::invalid(
            "engine must be claude, codex, or acp:<slug>.",
        ));
    }
    let scope = optional_string(params, "scope")?
        .map(|value| {
            LaunchScope::parse(value).ok_or_else(|| {
                AgentProtocolError::invalid("scope must be all, project, or isolated.")
            })
        })
        .transpose()?;
    let mut task = optional_task_metadata(params)?;
    if let Some(parent) = task
        .as_ref()
        .and_then(|metadata| metadata.parent_session_key.clone())
    {
        let resolved = resolve_target(state, &parent).await?;
        task.as_mut().expect("task exists").parent_session_key = Some(resolved);
    }
    let session = state
        .controller
        .start_agent(StartAgentInput {
            name: name.to_owned(),
            prompt,
            engine: engine.map(str::to_owned),
            cwd: optional_string(params, "cwd")?.map(str::to_owned),
            model: optional_string(params, "model")?.map(str::to_owned),
            effort: optional_string(params, "effort")?.map(str::to_owned),
            scope,
            task,
        })
        .await?;
    match state
        .names
        .lock()
        .await
        .bind(name.to_owned(), session.key.clone())
        .await?
    {
        NameBind::NameTaken => {
            return Err(AgentProtocolError::new(
                AgentErrorCode::NameTaken,
                format!("Agent name {name} is already in use."),
            ));
        }
        NameBind::Changed | NameBind::Unchanged => {}
    }
    let _session = state.sessions.lock(&session.key).await;
    state.controller.focus_agent(&session.key).await?;
    snapshot_value(state, &session.key).await
}

async fn rename_agent(
    state: &ServerState,
    params: &Map<String, Value>,
) -> Result<Value, AgentProtocolError> {
    let key = target_key(state, params, &["target", "name", "clear"]).await?;
    let clear = params
        .get("clear")
        .and_then(Value::as_bool)
        .ok_or_else(|| AgentProtocolError::invalid("clear must be a boolean."))?;
    if clear {
        if params.contains_key("name") {
            return Err(AgentProtocolError::invalid(
                "rename cannot combine name with clear.",
            ));
        }
        state.names.lock().await.clear_key(&key).await?;
    } else {
        let name = required_string(params, "name")?;
        if !is_agent_name(name) {
            return Err(AgentProtocolError::invalid(format!(
                "Invalid agent name: {name}."
            )));
        }
        if state
            .names
            .lock()
            .await
            .bind(name.to_owned(), key.clone())
            .await?
            == NameBind::NameTaken
        {
            return Err(AgentProtocolError::new(
                AgentErrorCode::NameTaken,
                format!("Agent name {name} is already in use."),
            ));
        }
    }
    snapshot_value(state, &key).await
}

/// Delivers `params` to `key`, already resolved from its `target` with the
/// parameters checked.
#[allow(clippy::too_many_lines)]
async fn deliver_message(
    state: &ServerState,
    key: String,
    params: &Map<String, Value>,
    steer: bool,
) -> Result<Value, AgentProtocolError> {
    let text = required_string(params, "text")?.to_owned();
    let sender = match optional_string(params, "sender")? {
        Some(target) => Some(resolve_target(state, target).await?),
        None => None,
    };
    let message_id = optional_string(params, "messageId")?
        .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned);
    let parent = optional_string(params, "replyTo")?.map(str::to_owned);
    let expect_reply = optional_bool(params, "expectReply")?;
    if (expect_reply || parent.is_some()) && sender.is_none() {
        return Err(AgentProtocolError::invalid(
            "sender is required for expectReply and replyTo deliveries.",
        ));
    }
    let begun = state
        .mailbox
        .lock()
        .await
        .begin(BeginMessage {
            message_id: message_id.clone(),
            sender_session_key: sender,
            recipient_session_key: key.clone(),
            body: text,
            expect_reply,
            parent_message_id: parent,
        })
        .await?;
    state.mailbox_changed.notify_waiters();
    if !begun.created {
        return message_result(state, &key, begun.message).await;
    }
    let prompt = agent_message_prompt(&begun.message);
    let delivery = if steer {
        state.controller.steer_agent(&key, prompt).await
    } else {
        state.controller.send_agent(&key, prompt).await
    };
    let delivery = match delivery {
        Ok(delivery) => delivery,
        Err(error) => {
            let settled = state
                .mailbox
                .lock()
                .await
                .settle(
                    &message_id,
                    AgentMessageState::Failed,
                    Some(error.code),
                    Some(for_display(&error.message)),
                )
                .await;
            return match settled {
                Ok(_) => {
                    state.mailbox_changed.notify_waiters();
                    Err(error)
                }
                Err(_) => {
                    unknown_message_result(
                        state,
                        &key,
                        begun.message,
                        "The provider was not called, but the durable failure receipt could not be recorded.",
                    )
                    .await
                }
            };
        }
    };
    let receipt_state = match delivery.kind {
        AgentDeliveryKind::AdapterAccepted if delivery.queued => AgentMessageState::Queued,
        AgentDeliveryKind::AdapterAccepted => AgentMessageState::ProviderAccepted,
        AgentDeliveryKind::DefinitelyUnsent => AgentMessageState::DefinitelyUnsent,
        AgentDeliveryKind::OutcomeUnknown => AgentMessageState::OutcomeUnknown,
    };
    let settled = state
        .mailbox
        .lock()
        .await
        .settle(
            &message_id,
            receipt_state,
            None,
            delivery.message.map(|message| for_display(&message)),
        )
        .await;
    match settled {
        Ok(settled) => {
            state.mailbox_changed.notify_waiters();
            message_result(state, &key, settled).await
        }
        Err(_) => {
            unknown_message_result(
                state,
                &key,
                begun.message,
                "The provider attempt settled, but its durable receipt could not be recorded.",
            )
            .await
        }
    }
}

async fn message_result(
    state: &ServerState,
    key: &str,
    message: AgentMailboxMessage,
) -> Result<Value, AgentProtocolError> {
    let receipt = message.receipts.last().expect("validated receipts");
    if receipt.state == AgentMessageState::Failed {
        return Err(AgentProtocolError {
            code: receipt.error_code.unwrap_or(AgentErrorCode::Failed),
            message: receipt
                .detail
                .clone()
                .unwrap_or_else(|| "Agent message delivery failed.".into()),
            data: None,
        });
    }
    let kind = match receipt.state {
        AgentMessageState::Queued | AgentMessageState::ProviderAccepted => {
            AgentDeliveryKind::AdapterAccepted
        }
        AgentMessageState::DefinitelyUnsent => AgentDeliveryKind::DefinitelyUnsent,
        AgentMessageState::OutcomeUnknown | AgentMessageState::Requested => {
            AgentDeliveryKind::OutcomeUnknown
        }
        AgentMessageState::Failed => unreachable!(),
    };
    Ok(json!({
        "snapshot": snapshot(state, key).await?,
        "message": message,
        "delivery": {
            "kind": kind,
            "message": receipt.detail,
        },
        "queued": receipt.state == AgentMessageState::Queued,
    }))
}

async fn unknown_message_result(
    state: &ServerState,
    key: &str,
    mut message: AgentMailboxMessage,
    detail: &str,
) -> Result<Value, AgentProtocolError> {
    let at = now_iso();
    message.receipts.push(AgentMessageReceipt {
        state: AgentMessageState::OutcomeUnknown,
        at: at.clone(),
        error_code: None,
        detail: Some(detail.to_owned()),
    });
    message.updated_at = at;
    message_result(state, key, message).await
}

async fn inbox_query(
    state: &ServerState,
    params: &Map<String, Value>,
    monitor: bool,
) -> Result<(String, u64, usize, Option<u64>), AgentProtocolError> {
    let key = target_key(
        state,
        params,
        if monitor {
            &["target", "after", "limit", "timeoutMs"]
        } else {
            &["target", "after", "limit"]
        },
    )
    .await?;
    let after = optional_non_negative_integer(params, "after")?.unwrap_or(0);
    let limit = optional_non_negative_integer(params, "limit")?.unwrap_or(50);
    if limit == 0 || limit > AGENT_MAILBOX_MAX_PAGE {
        return Err(AgentProtocolError::invalid(format!(
            "limit must be an integer from 1 to {AGENT_MAILBOX_MAX_PAGE}."
        )));
    }
    let timeout = optional_non_negative_integer(params, "timeoutMs")?;
    let limit = usize::try_from(limit)
        .map_err(|_| AgentProtocolError::invalid("limit is not supported on this platform."))?;
    Ok((key, after, limit, timeout))
}

async fn monitor_inbox(
    state: &ServerState,
    key: &str,
    after: u64,
    limit: usize,
    timeout_ms: Option<u64>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Value, AgentProtocolError> {
    let future = async {
        loop {
            if *shutdown.borrow() {
                return Err(AgentProtocolError::new(
                    AgentErrorCode::NotRunning,
                    "The iface TUI stopped while monitoring.",
                ));
            }
            // Arm the waiter before reading, so a delivery that lands between
            // the read and the wait cannot be dropped.
            let notified = state.mailbox_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let page = state.mailbox.lock().await.page(key, after, limit);
            if !page.messages.is_empty() {
                return serde_json::to_value(page).map_err(serialization_error);
            }
            tokio::select! {
                () = notified => {}
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        return Err(AgentProtocolError::new(
                            AgentErrorCode::NotRunning,
                            "The iface TUI stopped while monitoring.",
                        ));
                    }
                }
            }
        }
    };
    if let Some(timeout) = timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(timeout), future)
            .await
            .map_err(|_| {
                AgentProtocolError::new(
                    AgentErrorCode::Timeout,
                    "Timed out monitoring the agent inbox.",
                )
            })?
    } else {
        future.await
    }
}

async fn wait_for_status(
    state: &ServerState,
    key: &str,
    desired: AgentStatus,
    timeout_ms: Option<u64>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<AgentSnapshot, AgentProtocolError> {
    let mut changes = state.controller.subscribe_agent_state();
    let future = async {
        loop {
            if *shutdown.borrow() {
                return Err(AgentProtocolError::new(
                    AgentErrorCode::NotRunning,
                    "The iface TUI stopped while waiting.",
                ));
            }
            let current = snapshot(state, key).await?;
            if current.status == desired {
                return Ok(current);
            }
            match current.status {
                AgentStatus::Failed => {
                    return Err(AgentProtocolError::new(
                        AgentErrorCode::Failed,
                        format!("Agent session {key} has failed."),
                    ));
                }
                AgentStatus::NotLive => {
                    return Err(AgentProtocolError::new(
                        AgentErrorCode::NotLive,
                        format!("Agent session {key} is not live."),
                    ));
                }
                _ => {}
            }
            tokio::select! {
                result = changes.changed() => {
                    if result.is_err() {
                        return Err(AgentProtocolError::new(
                            AgentErrorCode::NotRunning,
                            "The iface TUI stopped while waiting.",
                        ));
                    }
                }
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        return Err(AgentProtocolError::new(
                            AgentErrorCode::NotRunning,
                            "The iface TUI stopped while waiting.",
                        ));
                    }
                }
            }
        }
    };
    if let Some(timeout) = timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(timeout), future)
            .await
            .map_err(|_| {
                AgentProtocolError::new(
                    AgentErrorCode::Timeout,
                    format!("Timed out waiting for {}.", desired.as_str()),
                )
            })?
    } else {
        future.await
    }
}

fn required_string<'a>(
    params: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, AgentProtocolError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentProtocolError::invalid(format!("{name} must be a non-empty string.")))
}

fn optional_string<'a>(
    params: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, AgentProtocolError> {
    match params.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|value| !value.is_empty())
            .map(Some)
            .ok_or_else(|| {
                AgentProtocolError::invalid(format!("{name} must be a non-empty string."))
            }),
    }
}

fn optional_bool(params: &Map<String, Value>, name: &str) -> Result<bool, AgentProtocolError> {
    params.get(name).map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| AgentProtocolError::invalid(format!("{name} must be a boolean.")))
    })
}

fn required_positive_integer(
    params: &Map<String, Value>,
    name: &str,
) -> Result<usize, AgentProtocolError> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| AgentProtocolError::invalid(format!("{name} must be a positive integer.")))
}

fn optional_non_negative_integer(
    params: &Map<String, Value>,
    name: &str,
) -> Result<Option<u64>, AgentProtocolError> {
    match params.get(name) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            AgentProtocolError::invalid(format!("{name} must be a non-negative integer."))
        }),
    }
}

fn assert_params(params: &Map<String, Value>, allowed: &[&str]) -> Result<(), AgentProtocolError> {
    if let Some(unexpected) = params.keys().find(|name| !allowed.contains(&name.as_str())) {
        return Err(AgentProtocolError::invalid(format!(
            "Unexpected agent request parameter: {unexpected}."
        )));
    }
    Ok(())
}

fn optional_task_metadata(
    params: &Map<String, Value>,
) -> Result<Option<AgentTaskMetadata>, AgentProtocolError> {
    let Some(value) = params.get("task") else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| AgentProtocolError::invalid("task must be an object."))?;
    assert_params(
        object,
        &[
            "planId",
            "taskId",
            "parentSessionKey",
            "role",
            "scope",
            "briefHash",
        ],
    )?;
    let metadata = AgentTaskMetadata {
        plan_id: bounded_metadata(object.get("planId"), "task.planId", 128)?,
        task_id: bounded_metadata(object.get("taskId"), "task.taskId", 128)?,
        parent_session_key: bounded_metadata(
            object.get("parentSessionKey"),
            "task.parentSessionKey",
            256,
        )?,
        role: bounded_metadata(object.get("role"), "task.role", 128)?,
        scope: bounded_metadata(object.get("scope"), "task.scope", 512)?,
        brief_hash: bounded_metadata(object.get("briefHash"), "task.briefHash", 256)?,
    };
    if metadata == AgentTaskMetadata::default() {
        return Err(AgentProtocolError::invalid(
            "task must contain at least one metadata field.",
        ));
    }
    Ok(Some(metadata))
}

fn bounded_metadata(
    value: Option<&Value>,
    name: &str,
    maximum: usize,
) -> Result<Option<String>, AgentProtocolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let valid = value.as_str().filter(|value| {
        !value.is_empty()
            && value.chars().count() <= maximum
            && !value.chars().any(char::is_control)
    });
    valid.map(|value| Some(value.to_owned())).ok_or_else(|| {
        AgentProtocolError::invalid(format!(
            "{name} must be a non-empty control-free string of at most {maximum} characters."
        ))
    })
}

fn failure(error: AgentProtocolError) -> AgentFailure {
    AgentFailure {
        code: error.code,
        message: for_display(&error.message),
        data: error.data,
    }
}

fn serialization_error(_error: serde_json::Error) -> AgentProtocolError {
    AgentProtocolError::new(
        AgentErrorCode::Failed,
        "Agent response could not be encoded.",
    )
}

fn validate_socket_paths(data: &Path, socket: &Path, names: &Path) -> Result<(), String> {
    if !data.is_absolute()
        || !socket.is_absolute()
        || !names.is_absolute()
        || socket.parent() != Some(data)
    {
        return Err(
            "Agent socket must be an absolute path directly inside the CLI data directory.".into(),
        );
    }
    Ok(())
}

fn valid_engine(value: &str) -> bool {
    matches!(value, "claude" | "codex")
        || value
            .strip_prefix("acp:")
            .is_some_and(|slug| !slug.is_empty())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentNameRow {
    key: String,
    named_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentNameState {
    version: u8,
    names: BTreeMap<String, AgentNameRow>,
}

struct AgentNameStore {
    file: PathBuf,
    state: AgentNameState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameBind {
    Changed,
    Unchanged,
    NameTaken,
}

impl AgentNameStore {
    fn new(file: PathBuf) -> Self {
        Self {
            file,
            state: AgentNameState {
                version: 1,
                names: BTreeMap::new(),
            },
        }
    }

    async fn load(&mut self) -> Result<(), String> {
        match secure_read(&self.file).await {
            Ok(Some(bytes)) => match serde_json::from_slice::<AgentNameState>(&bytes) {
                Ok(state) if validate_names(&state) => self.state = state,
                _ => {
                    let corrupt = self.file.with_extension("json.corrupt");
                    let _ = fs::rename(&self.file, &corrupt).await;
                    let _ = fs::set_permissions(
                        &corrupt,
                        std::fs::Permissions::from_mode(OWNER_FILE_MODE),
                    )
                    .await;
                }
            },
            Ok(None) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn key_for_name(&self, name: &str) -> Option<String> {
        self.state.names.get(name).map(|row| row.key.clone())
    }

    fn name_for_key(&self, key: &str) -> Option<String> {
        self.state
            .names
            .iter()
            .find_map(|(name, row)| (row.key == key).then(|| name.clone()))
    }

    async fn bind(&mut self, name: String, key: String) -> Result<NameBind, AgentProtocolError> {
        if let Some(row) = self.state.names.get(&name) {
            if row.key != key {
                return Ok(NameBind::NameTaken);
            }
        }
        let current = self.name_for_key(&key);
        if current.as_deref() == Some(name.as_str()) {
            return Ok(NameBind::Unchanged);
        }
        let mut next = self.state.clone();
        if let Some(current) = current {
            next.names.remove(&current);
        }
        next.names.insert(
            name,
            AgentNameRow {
                key,
                named_at: now_iso(),
            },
        );
        atomic_write(&self.file, &next)
            .await
            .map_err(|error| AgentProtocolError::new(AgentErrorCode::Failed, error))?;
        self.state = next;
        Ok(NameBind::Changed)
    }

    async fn clear_key(&mut self, key: &str) -> Result<(), AgentProtocolError> {
        let Some(name) = self.name_for_key(key) else {
            return Ok(());
        };
        let mut next = self.state.clone();
        next.names.remove(&name);
        atomic_write(&self.file, &next)
            .await
            .map_err(|error| AgentProtocolError::new(AgentErrorCode::Failed, error))?;
        self.state = next;
        Ok(())
    }
}

fn validate_names(state: &AgentNameState) -> bool {
    if state.version != 1 {
        return false;
    }
    let mut keys = BTreeSet::new();
    state.names.iter().all(|(name, row)| {
        is_agent_name(name)
            && valid_session_key(&row.key)
            && !row.named_at.is_empty()
            && keys.insert(row.key.clone())
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum AgentMessageState {
    Requested,
    Queued,
    ProviderAccepted,
    DefinitelyUnsent,
    OutcomeUnknown,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentMessageReceipt {
    state: AgentMessageState,
    at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<AgentErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentMailboxMessage {
    version: u8,
    cursor: u64,
    message_id: String,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sender_session_key: Option<String>,
    recipient_session_key: String,
    body: String,
    body_bytes: usize,
    expect_reply: bool,
    receipts: Vec<AgentMessageReceipt>,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentMailboxState {
    version: u8,
    next_cursor: u64,
    messages: Vec<AgentMailboxMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentInboxPage {
    messages: Vec<AgentMailboxMessage>,
    next_cursor: u64,
}

struct BeginMessage {
    message_id: String,
    sender_session_key: Option<String>,
    recipient_session_key: String,
    body: String,
    expect_reply: bool,
    parent_message_id: Option<String>,
}

struct BegunMessage {
    message: AgentMailboxMessage,
    created: bool,
}

struct AgentMailbox {
    file: PathBuf,
    state: AgentMailboxState,
}

impl AgentMailbox {
    fn new(file: PathBuf) -> Self {
        Self {
            file,
            state: AgentMailboxState {
                version: AGENT_MAILBOX_VERSION,
                next_cursor: 1,
                messages: Vec::new(),
            },
        }
    }

    async fn load(&mut self) -> Result<(), String> {
        let Some(bytes) = secure_read(&self.file).await? else {
            return Ok(());
        };
        let state = serde_json::from_slice::<AgentMailboxState>(&bytes).map_err(|_| {
            "Agent mailbox is corrupt; refusing to discard delivery history".to_owned()
        })?;
        if !validate_mailbox(&state) {
            return Err("Agent mailbox is corrupt; refusing to discard delivery history".into());
        }
        self.state = state;
        let now = now_iso();
        let mut changed = false;
        for message in &mut self.state.messages {
            if message
                .receipts
                .last()
                .is_some_and(|receipt| receipt.state == AgentMessageState::Requested)
            {
                message.receipts.push(AgentMessageReceipt {
                    state: AgentMessageState::OutcomeUnknown,
                    at: now.clone(),
                    error_code: None,
                    detail: Some(
                        "iface restarted before the provider delivery outcome was recorded.".into(),
                    ),
                });
                message.updated_at.clone_from(&now);
                changed = true;
            }
        }
        // A prune alone is written with the next delivery.
        self.prune(Utc::now());
        if changed {
            atomic_write(&self.file, &self.state).await?;
        }
        Ok(())
    }

    /// Drops settled messages past the retention window or beyond the newest
    /// `AGENT_MAILBOX_KEPT_SETTLED`. A message still `Requested`, or whose
    /// outcome is unknown, stays, so a retry of its id is answered from its
    /// receipt instead of sending twice; so does every parent a kept reply
    /// names. `next_cursor` never moves back, so no cursor is handed out twice.
    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - TimeDelta::days(AGENT_MAILBOX_RETENTION_DAYS);
        let mut settled = 0_usize;
        let mut parents = HashSet::new();
        let mut keep = vec![true; self.state.messages.len()];
        for (index, message) in self.state.messages.iter().enumerate().rev() {
            if is_settled(message) {
                settled += 1;
                keep[index] = parents.contains(message.message_id.as_str())
                    || (settled <= AGENT_MAILBOX_KEPT_SETTLED && !settled_before(message, cutoff));
            }
            if keep[index]
                && let Some(parent) = &message.parent_message_id
            {
                parents.insert(parent.as_str());
            }
        }
        let mut keep = keep.into_iter();
        self.state.messages.retain(|_| keep.next().unwrap_or(true));
    }

    async fn begin(&mut self, input: BeginMessage) -> Result<BegunMessage, AgentProtocolError> {
        if !bounded_id(&input.message_id) {
            return Err(AgentProtocolError::invalid(
                "messageId must be a control-free string of at most 128 characters.",
            ));
        }
        if input.body.len() > AGENT_MAX_PROMPT_BYTES {
            return Err(AgentProtocolError::invalid(
                "agent message text is limited to 64 KiB of UTF-8.",
            ));
        }
        // Memory may run ahead of disk here; the write below carries it.
        self.prune(Utc::now());
        if let Some(existing) = self
            .state
            .messages
            .iter()
            .find(|message| message.message_id == input.message_id)
        {
            if !same_intent(existing, &input) {
                return Err(AgentProtocolError::invalid(format!(
                    "messageId {} is already bound to another delivery intent.",
                    input.message_id
                )));
            }
            return Ok(BegunMessage {
                message: existing.clone(),
                created: false,
            });
        }
        if self.state.messages.len() >= AGENT_MAILBOX_MAX_MESSAGES {
            return Err(mailbox_full());
        }
        let parent = match input.parent_message_id.as_ref() {
            Some(parent) => Some(
                self.state
                    .messages
                    .iter()
                    .find(|message| &message.message_id == parent)
                    .ok_or_else(|| {
                        AgentProtocolError::new(
                            AgentErrorCode::NotFound,
                            format!("No mailbox message matches {parent}."),
                        )
                    })?,
            ),
            None => None,
        };
        if parent.is_some_and(|parent| {
            !parent.expect_reply
                || input.sender_session_key != Some(parent.recipient_session_key.clone())
                || Some(input.recipient_session_key.clone()) != parent.sender_session_key
        }) {
            return Err(AgentProtocolError::invalid(
                "The reply does not match the original sender, recipient, and expect-reply contract.",
            ));
        }
        let now = now_iso();
        let message = AgentMailboxMessage {
            version: 1,
            cursor: self.state.next_cursor,
            message_id: input.message_id.clone(),
            thread_id: parent.map_or_else(
                || input.message_id.clone(),
                |parent| parent.thread_id.clone(),
            ),
            parent_message_id: input.parent_message_id,
            sender_session_key: input.sender_session_key,
            recipient_session_key: input.recipient_session_key,
            body_bytes: input.body.len(),
            body: input.body,
            expect_reply: input.expect_reply,
            receipts: vec![AgentMessageReceipt {
                state: AgentMessageState::Requested,
                at: now.clone(),
                error_code: None,
                detail: None,
            }],
            created_at: now.clone(),
            updated_at: now,
        };
        // In-place mutation is equivalent on every synchronous error path.
        // A dropped future (runtime shutdown today) can leave memory ahead of
        // disk; do not wrap begin/settle in timeout or select!.
        let previous_cursor = self.state.next_cursor;
        self.state.next_cursor = previous_cursor.saturating_add(1);
        self.state.messages.push(message.clone());
        if let Err(error) = atomic_write(&self.file, &self.state).await {
            let _ = self.state.messages.pop();
            self.state.next_cursor = previous_cursor;
            return Err(AgentProtocolError::new(AgentErrorCode::Failed, error));
        }
        Ok(BegunMessage {
            message,
            created: true,
        })
    }

    async fn settle(
        &mut self,
        message_id: &str,
        state: AgentMessageState,
        error_code: Option<AgentErrorCode>,
        detail: Option<String>,
    ) -> Result<AgentMailboxMessage, AgentProtocolError> {
        let index = self
            .state
            .messages
            .iter()
            .position(|message| message.message_id == message_id)
            .ok_or_else(|| {
                AgentProtocolError::new(
                    AgentErrorCode::Failed,
                    format!("No durable agent message {message_id}"),
                )
            })?;
        let previous_updated_at = self.state.messages[index].updated_at.clone();
        let now = now_iso();
        self.state.messages[index]
            .receipts
            .push(AgentMessageReceipt {
                state,
                at: now.clone(),
                error_code,
                detail,
            });
        self.state.messages[index].updated_at = now;
        let result = self.state.messages[index].clone();
        if let Err(error) = atomic_write(&self.file, &self.state).await {
            let message = &mut self.state.messages[index];
            let _ = message.receipts.pop();
            message.updated_at = previous_updated_at;
            return Err(AgentProtocolError::new(AgentErrorCode::Failed, error));
        }
        Ok(result)
    }

    /// Up to `limit` messages after `after`, stopping early once the page
    /// would outgrow the response budget. `next_cursor` then marks where the
    /// short page ended; it always holds at least one message when any wait.
    fn page(&self, key: &str, after: u64, limit: usize) -> AgentInboxPage {
        let mut messages = Vec::new();
        let mut bytes = 0_usize;
        for message in self
            .state
            .messages
            .iter()
            .filter(|message| message.recipient_session_key == key && message.cursor > after)
            .take(limit)
        {
            bytes = bytes.saturating_add(encoded_len(message).saturating_add(1));
            if bytes > AGENT_RESULT_BUDGET_BYTES && !messages.is_empty() {
                break;
            }
            messages.push(message.clone());
        }
        AgentInboxPage {
            next_cursor: messages.last().map_or(after, |message| message.cursor),
            messages,
        }
    }
}

fn validate_mailbox(state: &AgentMailboxState) -> bool {
    if state.version != 1
        || state.next_cursor == 0
        || state.messages.len() > AGENT_MAILBOX_MAX_MESSAGES
    {
        return false;
    }
    let mut ids = BTreeSet::new();
    let mut cursors = BTreeSet::new();
    for message in &state.messages {
        if message.version != 1
            || message.cursor == 0
            || message.cursor >= state.next_cursor
            || !bounded_id(&message.message_id)
            || !bounded_id(&message.thread_id)
            || !message
                .parent_message_id
                .as_ref()
                .is_none_or(|parent| ids.contains(parent))
            || !message
                .sender_session_key
                .as_ref()
                .is_none_or(|key| valid_session_key(key))
            || !valid_session_key(&message.recipient_session_key)
            || message.body_bytes != message.body.len()
            || message.body_bytes > AGENT_MAX_PROMPT_BYTES
            || message.receipts.is_empty()
            || !ids.insert(message.message_id.clone())
            || !cursors.insert(message.cursor)
        {
            return false;
        }
    }
    true
}

fn mailbox_full() -> AgentProtocolError {
    AgentProtocolError::new(
        AgentErrorCode::Blocked,
        format!(
            "The durable agent mailbox is full: settled messages are pruned after \
             {AGENT_MAILBOX_RETENTION_DAYS} days, but deliveries with an unknown outcome are \
             kept so a retry cannot send twice. Move agent-messages.json out of the iface data \
             directory to start a new mailbox."
        ),
    )
}

/// Whether the message's delivery has a known outcome: anything but an
/// in-flight `Requested` or an unknown outcome.
fn is_settled(message: &AgentMailboxMessage) -> bool {
    message.receipts.last().is_some_and(|receipt| {
        matches!(
            receipt.state,
            AgentMessageState::Queued
                | AgentMessageState::ProviderAccepted
                | AgentMessageState::DefinitelyUnsent
                | AgentMessageState::Failed
        )
    })
}

/// An unreadable time never counts as old; the count bound still applies.
fn settled_before(message: &AgentMailboxMessage, cutoff: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&message.updated_at).is_ok_and(|at| at < cutoff)
}

fn same_intent(message: &AgentMailboxMessage, input: &BeginMessage) -> bool {
    message.sender_session_key == input.sender_session_key
        && message.recipient_session_key == input.recipient_session_key
        && message.body == input.body
        && message.expect_reply == input.expect_reply
        && message.parent_message_id == input.parent_message_id
}

fn agent_message_prompt(message: &AgentMailboxMessage) -> String {
    if message.sender_session_key.is_none()
        && message.parent_message_id.is_none()
        && !message.expect_reply
    {
        return message.body.clone();
    }
    let mut metadata = vec![
        format!("message-id: {}", message.message_id),
        format!("thread-id: {}", message.thread_id),
    ];
    if let Some(parent) = &message.parent_message_id {
        metadata.push(format!("reply-to: {parent}"));
    }
    if let Some(sender) = &message.sender_session_key {
        metadata.push(format!("sender: {sender}"));
    }
    metadata.push(format!(
        "expect-reply: {}",
        if message.expect_reply { "yes" } else { "no" }
    ));
    format!(
        "--- IFACE AGENT MESSAGE ---\n{}\n--- BODY ---\n{}",
        metadata.join("\n"),
        message.body
    )
}

async fn secure_read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || secure_read_blocking(&path))
        .await
        .unwrap_or_else(|error| Err(error.to_string()))
}

/// Reads an owner-private store file through one descriptor. The name is
/// checked first for the familiar refusals; the mode is then re-asserted and
/// the bytes read on the opened file itself, which `O_NOFOLLOW` keeps from
/// being a link swapped in after the check.
fn secure_read_blocking(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let symlink = || format!("{} must not be a symbolic link", path.display());
    let irregular = || format!("{} must be a regular file", path.display());
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(symlink()),
        Ok(metadata) if !metadata.is_file() => return Err(irregular()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(symlink()),
        Err(error) => return Err(error.to_string()),
    };
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(irregular());
    }
    file.set_permissions(std::fs::Permissions::from_mode(OWNER_FILE_MODE))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(Some(bytes))
}

async fn atomic_write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || crate::private_write::write_private_bytes(&path, &bytes))
        .await
        .unwrap_or_else(|error| Err(error.to_string()))
}

fn valid_session_key(value: &str) -> bool {
    value
        .split_once('#')
        .is_some_and(|(engine, session)| !engine.is_empty() && !session.is_empty())
}

fn bounded_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn now_iso() -> String {
    canonical_timestamp(Utc::now())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex as StdMutex};

    use tempfile::TempDir;

    use super::*;
    use crate::agent_client::agent_rpc;

    struct FakeController {
        sessions: StdMutex<HashMap<String, AgentSessionState>>,
        transcript: StdMutex<HashMap<String, Vec<String>>>,
        revision: watch::Sender<u64>,
        /// The one method that waits for `Hold::release` once it is called.
        hold: Option<(&'static str, Arc<Hold>)>,
        /// `method key` per finished provider call, in order.
        calls: StdMutex<Vec<String>>,
    }

    #[derive(Default)]
    struct Hold {
        entered: Notify,
        release: Notify,
    }

    impl FakeController {
        fn new(rows: Vec<AgentSessionState>) -> Self {
            let sessions = rows.into_iter().map(|row| (row.key.clone(), row)).collect();
            let (revision, _) = watch::channel(0);
            Self {
                sessions: StdMutex::new(sessions),
                transcript: StdMutex::new(HashMap::new()),
                revision,
                hold: None,
                calls: StdMutex::new(Vec::new()),
            }
        }

        fn holding(mut self, method: &'static str) -> (Self, Arc<Hold>) {
            let hold = Arc::new(Hold::default());
            self.hold = Some((method, Arc::clone(&hold)));
            (self, hold)
        }

        async fn call(&self, method: &str, key: &str) {
            if let Some((held, hold)) = &self.hold
                && *held == method
            {
                hold.entered.notify_one();
                hold.release.notified().await;
            }
            self.calls
                .lock()
                .expect("calls")
                .push(format!("{method} {key}"));
        }
    }

    #[async_trait]
    impl AgentController for FakeController {
        fn agent_sessions(&self) -> Vec<AgentSessionState> {
            self.sessions
                .lock()
                .expect("sessions")
                .values()
                .cloned()
                .collect()
        }

        fn agent_session(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
            self.sessions
                .lock()
                .expect("sessions")
                .get(key)
                .cloned()
                .ok_or_else(|| AgentProtocolError::new(AgentErrorCode::NotFound, "missing"))
        }

        fn subscribe_agent_state(&self) -> watch::Receiver<u64> {
            self.revision.subscribe()
        }

        fn read_agent(&self, key: &str, lines: usize) -> Result<Vec<String>, AgentProtocolError> {
            let rows = self
                .transcript
                .lock()
                .expect("transcript")
                .get(key)
                .cloned()
                .unwrap_or_default();
            Ok(rows[rows.len().saturating_sub(lines)..].to_vec())
        }

        async fn send_agent(
            &self,
            key: &str,
            _text: String,
        ) -> Result<AgentDelivery, AgentProtocolError> {
            self.call("send", key).await;
            let row = self.agent_session(key)?;
            match row.status {
                AgentStatus::Blocked => {
                    Err(AgentProtocolError::new(AgentErrorCode::Blocked, "blocked"))
                }
                AgentStatus::Failed => {
                    Err(AgentProtocolError::new(AgentErrorCode::Failed, "failed"))
                }
                _ if !row.live => Err(AgentProtocolError::new(AgentErrorCode::NotLive, "not live")),
                AgentStatus::Working => Ok(AgentDelivery::accepted(true)),
                _ => Ok(AgentDelivery::accepted(false)),
            }
        }

        async fn steer_agent(
            &self,
            key: &str,
            _text: String,
        ) -> Result<AgentDelivery, AgentProtocolError> {
            let row = self.agent_session(key)?;
            if row.status != AgentStatus::Working {
                return Err(AgentProtocolError::new(
                    AgentErrorCode::Unsupported,
                    "unsupported",
                ));
            }
            Ok(AgentDelivery::accepted(false))
        }

        async fn stop_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
            self.call("stop", key).await;
            let mut sessions = self.sessions.lock().expect("sessions");
            let row = sessions
                .get_mut(key)
                .ok_or_else(|| AgentProtocolError::new(AgentErrorCode::NotFound, "missing"))?;
            row.status = AgentStatus::Idle;
            self.revision.send_modify(|value| *value += 1);
            Ok(row.clone())
        }

        async fn resume_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
            self.call("resume", key).await;
            let mut sessions = self.sessions.lock().expect("sessions");
            let row = sessions
                .get_mut(key)
                .ok_or_else(|| AgentProtocolError::new(AgentErrorCode::NotFound, "missing"))?;
            row.live = true;
            self.revision.send_modify(|value| *value += 1);
            Ok(row.clone())
        }

        async fn start_agent(
            &self,
            input: StartAgentInput,
        ) -> Result<AgentSessionState, AgentProtocolError> {
            self.call("start", &input.name).await;
            let engine = input.engine.unwrap_or_else(|| "claude".into());
            let session_id = format!("{}-id", input.name);
            let row = session(&format!("{engine}#{session_id}"), AgentStatus::Idle, true);
            self.sessions
                .lock()
                .expect("sessions")
                .insert(row.key.clone(), row.clone());
            self.revision.send_modify(|value| *value += 1);
            Ok(row)
        }

        async fn focus_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
            let row = self.agent_session(key)?;
            if !row.live {
                return Err(AgentProtocolError::new(AgentErrorCode::NotLive, "not live"));
            }
            Ok(row)
        }
    }

    fn session(key: &str, status: AgentStatus, live: bool) -> AgentSessionState {
        let (engine, session_id) = key.split_once('#').expect("key");
        AgentSessionState {
            key: key.into(),
            engine: engine.into(),
            session_id: session_id.into(),
            status,
            cwd: "/repo".into(),
            title: session_id.into(),
            model: None,
            task: None,
            capabilities: None,
            live,
        }
    }

    async fn harness(rows: Vec<AgentSessionState>) -> (TempDir, AgentServer, PathBuf) {
        harness_with(FakeController::new(rows)).await
    }

    async fn harness_with(controller: FakeController) -> (TempDir, AgentServer, PathBuf) {
        harness_shared(Arc::new(controller)).await
    }

    async fn harness_shared(controller: Arc<FakeController>) -> (TempDir, AgentServer, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let data = temp.path().join("data");
        let socket = data.join("iface.sock");
        let names = data.join("agents.json");
        let server = start_agent_server(&data, &socket, &names, controller)
            .await
            .expect("server");
        (temp, server, socket)
    }

    async fn rpc(
        socket: &Path,
        method: AgentMethod,
        params: Value,
    ) -> Result<Value, AgentProtocolError> {
        agent_rpc(
            socket,
            &AgentRequest {
                id: Uuid::new_v4().to_string(),
                method,
                params: params.as_object().cloned().expect("params"),
            },
        )
        .await
    }

    #[tokio::test]
    async fn lists_resolves_and_maps_live_status() {
        let (_temp, server, socket) = harness(vec![
            session("claude#abc123", AgentStatus::Idle, true),
            session("codex#dead", AgentStatus::Failed, false),
        ])
        .await;
        let listed = rpc(&socket, AgentMethod::List, json!({}))
            .await
            .expect("list");
        assert_eq!(listed.as_array().expect("array").len(), 2);
        let dormant = rpc(&socket, AgentMethod::Get, json!({"target":"dead"}))
            .await
            .expect("get");
        assert_eq!(dormant["status"], "not-live");
        server.close().await.expect("close");
    }

    #[tokio::test]
    async fn persists_deduplicated_correlated_messages_with_owner_only_modes() {
        let (temp, server, socket) = harness(vec![
            session("claude#parent", AgentStatus::Idle, true),
            session("codex#child", AgentStatus::Working, true),
        ])
        .await;
        let params = json!({
            "target":"child", "sender":"parent", "text":"Review this.",
            "messageId":"request-1", "expectReply":true
        });
        let first = rpc(&socket, AgentMethod::Send, params.clone())
            .await
            .expect("send");
        assert_eq!(first["message"]["threadId"], "request-1");
        assert_eq!(first["queued"], true);
        let second = rpc(&socket, AgentMethod::Send, params)
            .await
            .expect("dedupe");
        assert_eq!(second["message"]["messageId"], "request-1");
        let socket_mode = std::fs::metadata(&socket)
            .expect("socket")
            .permissions()
            .mode()
            & 0o777;
        let mailbox_mode = std::fs::metadata(temp.path().join("data/agent-messages.json"))
            .expect("mailbox")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(socket_mode, 0o600);
        assert_eq!(mailbox_mode, 0o600);
        server.close().await.expect("close");
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn enforces_one_capped_request_per_connection() {
        let (_temp, server, socket) = harness(Vec::new()).await;
        let mut stream = UnixStream::connect(&socket).await.expect("connect");
        stream
            .write_all(b"{\"id\":\"one\",\"method\":\"agent.list\",\"params\":{}}\n{}\n")
            .await
            .expect("write");
        stream.shutdown().await.expect("shutdown");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("read");
        let response = crate::agent_protocol::decode_agent_response(&response).expect("response");
        assert!(matches!(response.payload, AgentResponsePayload::Error(_)));
        server.close().await.expect("close");
    }

    #[tokio::test]
    async fn refuses_a_symlinked_data_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir(&real).await.expect("real");
        std::os::unix::fs::symlink(&real, &linked).expect("symlink");
        let error = start_agent_server(
            &linked,
            &linked.join("iface.sock"),
            &linked.join("agents.json"),
            Arc::new(FakeController::new(Vec::new())),
        )
        .await
        .expect_err("symlink refused");
        assert!(error.contains("symbolic link"));
    }

    #[tokio::test]
    async fn store_files_are_made_private_and_links_or_other_types_are_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let data = temp.path().join("data");
        std::fs::create_dir(&data).expect("data");
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).expect("private");
        let mailbox = data.join("agent-messages.json");
        std::fs::write(&mailbox, br#"{"version":1,"nextCursor":1,"messages":[]}"#)
            .expect("mailbox");
        std::fs::set_permissions(&mailbox, std::fs::Permissions::from_mode(0o644))
            .expect("readable");
        let names = data.join("agents.json");
        let socket = data.join("iface.sock");
        let start = || {
            start_agent_server(
                &data,
                &socket,
                &names,
                Arc::new(FakeController::new(Vec::new())),
            )
        };
        start().await.expect("server").close().await.expect("close");
        let mode = std::fs::metadata(&mailbox)
            .expect("mailbox")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        std::os::unix::fs::symlink(&mailbox, &names).expect("link");
        let error = start().await.expect_err("link refused");
        assert_eq!(
            error,
            format!("{} must not be a symbolic link", names.display())
        );
        std::fs::remove_file(&names).expect("unlink");
        std::fs::create_dir(&names).expect("directory");
        let error = start().await.expect_err("directory refused");
        assert_eq!(error, format!("{} must be a regular file", names.display()));
    }

    #[tokio::test]
    async fn an_unsendable_response_is_an_error_frame_not_silence() {
        let controller = FakeController::new(vec![session("claude#big", AgentStatus::Idle, true)]);
        // Twenty 64 KiB lines: more than one response line can carry.
        controller
            .transcript
            .lock()
            .expect("transcript")
            .insert("claude#big".into(), vec!["x".repeat(64 * 1024); 20]);
        let (_temp, server, socket) = harness_with(controller).await;
        let error = rpc(
            &socket,
            AgentMethod::Read,
            json!({"target":"big","lines":20}),
        )
        .await
        .expect_err("oversized");
        // The frame carried the request id; a mismatch would read differently.
        assert_eq!(error.code, AgentErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "Agent protocol line exceeds the 1 MiB limit."
        );
        assert!(
            rpc(
                &socket,
                AgentMethod::Read,
                json!({"target":"big","lines":2})
            )
            .await
            .is_ok()
        );
        server.close().await.expect("close");
    }

    #[tokio::test]
    async fn a_peer_that_never_sends_its_request_is_answered_and_released() {
        let (_temp, server, socket) = harness(Vec::new()).await;
        let mut stream = UnixStream::connect(&socket).await.expect("connect");
        let mut response = Vec::new();
        tokio::time::timeout(
            AGENT_REQUEST_READ_TIMEOUT * 6,
            stream.read_to_end(&mut response),
        )
        .await
        .expect("released after the read deadline")
        .expect("read");
        let response = crate::agent_protocol::decode_agent_response(&response).expect("response");
        assert_eq!(response.id, "invalid-request");
        assert_eq!(
            response.payload,
            AgentResponsePayload::Error(AgentFailure {
                code: AgentErrorCode::InvalidRequest,
                message: "Agent protocol request could not be read.".into(),
                data: None,
            })
        );
        server.close().await.expect("close");
    }

    fn mailbox_message(cursor: u64, body: String) -> AgentMailboxMessage {
        AgentMailboxMessage {
            version: 1,
            cursor,
            message_id: format!("message-{cursor}"),
            thread_id: format!("message-{cursor}"),
            parent_message_id: None,
            sender_session_key: None,
            recipient_session_key: "claude#one".into(),
            body_bytes: body.len(),
            body,
            expect_reply: false,
            receipts: vec![AgentMessageReceipt {
                state: AgentMessageState::Queued,
                at: "2026-09-27T00:00:00.000Z".into(),
                error_code: None,
                detail: None,
            }],
            created_at: "2026-09-27T00:00:00.000Z".into(),
            updated_at: "2026-09-27T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn inbox_pages_stop_at_the_response_budget_and_page_on_from_the_cursor() {
        let mut mailbox = AgentMailbox::new(PathBuf::from("/unused/agent-messages.json"));
        // Quotes encode as two bytes each, so every body is about 128 KiB of JSON.
        mailbox.state.messages = (1..=40)
            .map(|cursor| mailbox_message(cursor, "\"".repeat(AGENT_MAX_PROMPT_BYTES)))
            .collect();
        mailbox.state.next_cursor = 41;
        let first = mailbox.page("claude#one", 0, 100);
        assert!(!first.messages.is_empty() && first.messages.len() < 40);
        assert!(encoded_len(&first) <= AGENT_RESULT_BUDGET_BYTES);
        assert_eq!(
            first.next_cursor,
            first.messages.last().expect("last").cursor
        );
        let response = AgentResponse {
            id: Uuid::new_v4().to_string(),
            payload: AgentResponsePayload::Result(serde_json::to_value(&first).expect("page")),
        };
        assert!(encode_agent_response(response).is_ok());

        let mut cursors = first
            .messages
            .iter()
            .map(|message| message.cursor)
            .collect::<Vec<_>>();
        let mut after = first.next_cursor;
        loop {
            let page = mailbox.page("claude#one", after, 100);
            if page.messages.is_empty() {
                assert_eq!(page.next_cursor, after);
                break;
            }
            cursors.extend(page.messages.iter().map(|message| message.cursor));
            after = page.next_cursor;
        }
        assert_eq!(cursors, (1..=40).collect::<Vec<_>>());

        // One message larger than the budget still makes a page of one.
        mailbox.state.messages = vec![mailbox_message(
            1,
            "\u{1}".repeat(AGENT_MAX_PROMPT_BYTES * 3),
        )];
        assert_eq!(mailbox.page("claude#one", 0, 100).messages.len(), 1);
    }
    #[tokio::test]
    async fn a_long_list_keeps_live_and_named_sessions_and_fits_one_reply() {
        // Three thousand stored sessions with long working directories are
        // well past what one reply line can carry.
        let wide = format!("/{}", "d".repeat(400));
        let mut rows = (0..3_000)
            .map(|index| {
                let mut row = session(
                    &format!("claude#stored-{index:04}"),
                    AgentStatus::Idle,
                    false,
                );
                row.cwd.clone_from(&wide);
                row
            })
            .collect::<Vec<_>>();
        rows.push(session("codex#zz-live", AgentStatus::Working, true));
        rows.push(session("claude#zz-named", AgentStatus::Idle, false));
        let (_temp, server, socket) = harness(rows).await;
        rpc(
            &socket,
            AgentMethod::Rename,
            json!({"target":"zz-named","name":"keeper","clear":false}),
        )
        .await
        .expect("rename");

        let listed = rpc(&socket, AgentMethod::List, json!({}))
            .await
            .expect("list");
        let listed = listed.as_array().expect("array");
        assert!(listed.len() > 2 && listed.len() < 3_002, "{}", listed.len());
        assert!(encoded_len(listed) <= AGENT_RESULT_BUDGET_BYTES);
        assert_eq!(listed[0]["key"], "codex#zz-live");
        assert_eq!(listed[1]["name"], "keeper");
        assert_eq!(listed[2]["key"], "claude#stored-0000");
        assert!(listed[2..].iter().all(|row| row["live"] == false));

        let live = rpc(&socket, AgentMethod::List, json!({"live":true}))
            .await
            .expect("live list");
        let live = live.as_array().expect("array");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0]["key"], "codex#zz-live");
        let error = rpc(&socket, AgentMethod::List, json!({"live":"yes"}))
            .await
            .expect_err("not a boolean");
        assert_eq!(error.message, "live must be a boolean.");
        server.close().await.expect("close");
    }

    #[tokio::test]
    async fn a_stop_does_not_wait_for_another_sessions_start_or_resume() {
        for (method, request, params) in [
            ("start", AgentMethod::Start, json!({"name":"slow"})),
            ("resume", AgentMethod::Resume, json!({"target":"dormant"})),
        ] {
            let (controller, hold) = FakeController::new(vec![
                session("claude#busy", AgentStatus::Working, true),
                session("claude#dormant", AgentStatus::Idle, false),
            ])
            .holding(method);
            let (_temp, server, socket) = harness_with(controller).await;
            let slow = tokio::spawn({
                let socket = socket.clone();
                async move { rpc(&socket, request, params).await }
            });
            hold.entered.notified().await;
            let stopped = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                rpc(&socket, AgentMethod::Stop, json!({"target":"busy"})),
            )
            .await
            .unwrap_or_else(|_| panic!("the stop waited for the {method}"))
            .expect("stop");
            assert_eq!(stopped["status"], "idle");
            hold.release.notify_one();
            slow.await.expect("join").expect(method);
            server.close().await.expect("close");
        }
    }

    #[tokio::test]
    async fn verbs_on_one_session_keep_their_order() {
        let (controller, hold) =
            FakeController::new(vec![session("claude#one", AgentStatus::Idle, true)])
                .holding("send");
        let controller = Arc::new(controller);
        let (_temp, server, socket) = harness_shared(Arc::clone(&controller)).await;
        let send = tokio::spawn({
            let socket = socket.clone();
            async move {
                rpc(
                    &socket,
                    AgentMethod::Send,
                    json!({"target":"one","text":"first"}),
                )
                .await
            }
        });
        hold.entered.notified().await;
        let stop = tokio::spawn({
            let socket = socket.clone();
            async move { rpc(&socket, AgentMethod::Stop, json!({"target":"one"})).await }
        });
        // Time for the stop to reach the server, where it waits for the send.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!stop.is_finished());
        hold.release.notify_one();
        send.await.expect("join").expect("send");
        stop.await.expect("join").expect("stop");
        assert_eq!(
            *controller.calls.lock().expect("calls"),
            ["send claude#one", "stop claude#one"]
        );
        server.close().await.expect("close");
    }

    fn settled_at(cursor: u64, state: AgentMessageState, at: &str) -> AgentMailboxMessage {
        let mut message = mailbox_message(cursor, format!("body {cursor}"));
        message.receipts[0].state = state;
        message.created_at = at.into();
        message.updated_at = at.into();
        message
    }

    #[tokio::test]
    async fn the_mailbox_prunes_settled_messages_but_keeps_unknown_ones_and_their_parents() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut mailbox = AgentMailbox::new(temp.path().join("data/agent-messages.json"));
        let old = "2026-01-01T00:00:00.000Z";
        let recent = canonical_timestamp(Utc::now());
        let mut messages = vec![
            settled_at(1, AgentMessageState::ProviderAccepted, old),
            settled_at(2, AgentMessageState::OutcomeUnknown, old),
            settled_at(3, AgentMessageState::Requested, old),
            settled_at(4, AgentMessageState::Failed, old),
        ];
        // An unknown reply to an old settled request keeps the request.
        let mut reply = settled_at(5, AgentMessageState::OutcomeUnknown, old);
        reply.parent_message_id = Some("message-4".into());
        reply.thread_id = "message-4".into();
        messages.push(reply);
        // Ten thousand recent settled messages: the mailbox's old limit.
        messages.extend(
            (6..=AGENT_MAILBOX_MAX_MESSAGES as u64)
                .map(|cursor| settled_at(cursor, AgentMessageState::Queued, &recent)),
        );
        assert_eq!(messages.len(), AGENT_MAILBOX_MAX_MESSAGES);
        mailbox.state.messages = messages;
        mailbox.state.next_cursor = AGENT_MAILBOX_MAX_MESSAGES as u64 + 1;

        let begun = mailbox
            .begin(BeginMessage {
                message_id: "after-the-limit".into(),
                sender_session_key: None,
                recipient_session_key: "claude#one".into(),
                body: "still delivered".into(),
                expect_reply: false,
                parent_message_id: None,
            })
            .await
            .expect("a send past ten thousand messages");
        assert!(begun.created);
        assert_eq!(begun.message.cursor, AGENT_MAILBOX_MAX_MESSAGES as u64 + 1);
        assert_eq!(
            mailbox.state.next_cursor,
            AGENT_MAILBOX_MAX_MESSAGES as u64 + 2
        );
        assert!(validate_mailbox(&mailbox.state));
        let kept = |cursor: u64| {
            mailbox
                .state
                .messages
                .iter()
                .any(|message| message.cursor == cursor)
        };
        assert!(!kept(1), "an old settled message goes");
        assert!(kept(2) && kept(3), "unknown and in-flight messages stay");
        assert!(kept(4) && kept(5), "a kept reply keeps its parent");
        // The newest settled messages up to the count stay, older ones go.
        let recent_kept = (6..=AGENT_MAILBOX_MAX_MESSAGES as u64)
            .filter(|cursor| kept(*cursor))
            .count();
        assert_eq!(recent_kept, AGENT_MAILBOX_KEPT_SETTLED);
        assert!(kept(AGENT_MAILBOX_MAX_MESSAGES as u64));

        // What was written loads back as the same mailbox.
        let mut reloaded = AgentMailbox::new(temp.path().join("data/agent-messages.json"));
        reloaded.load().await.expect("load");
        assert_eq!(reloaded.state.next_cursor, mailbox.state.next_cursor);
        assert_eq!(reloaded.state.messages.len(), mailbox.state.messages.len());
    }
}
