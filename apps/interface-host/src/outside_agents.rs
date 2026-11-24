//! The outside socket (T3 section 11, protocol 65): how `iface mcp`, run by
//! a Claude Code or Codex outside Interface, reaches this host. One
//! owner-only socket at `<data root>/team/outside.sock` (or the short
//! fallback [`thread_team::outside_socket_path`] names), in a 0700
//! directory, and every connection's peer uid is checked against this
//! process's. Each accepted connection is one outside session: the host
//! names it, and the client only declares a display name. What each tool
//! does is `crate::threads`' (`threads/outside.rs`).
//!
//! The protocol is newline-delimited JSON-RPC 2.0 and only this:
//! `hello {client}` first, answering `{protocol, level}`; then any number
//! of `call {tool, input}`, answering `{text, isError}`. Nothing else is
//! served; the orchestration and pane sockets stay as they are.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::StreamExt;
use interface_settings::thread_team;
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{Mutex as AsyncMutex, OnceCell, Semaphore},
};
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    sync::CancellationToken,
};

use crate::{ThreadHost, mcp_transport};

/// The outside protocol's version, which `hello` answers.
pub const OUTSIDE_PROTOCOL: u64 = 1;
/// One request line: a task is at most 64 KiB, escaped several times over.
pub const MAX_LINE_BYTES: usize = 1 << 20;
/// Calls one connection may have running at once (a wait may last 600 s).
const MAX_IN_FLIGHT: usize = 16;
/// Outside sessions connected at once.
const MAX_CONNECTIONS: usize = 16;

/// The tools outside agents have, for `iface mcp`'s `tools/list`, in
/// [`thread_team::OUTSIDE_READ_TOOLS`] then
/// [`thread_team::OUTSIDE_FULL_TOOLS`] order. The level is the owner's
/// setting, never the input's.
#[must_use]
pub fn catalogue() -> Vec<Value> {
    let string = json!({"type": "string"});
    let thread =
        json!({"type": "string", "description": "A thread key from list_threads or start_thread."});
    let request = json!({"type": "string", "description": "A stable id for this call; a retry with the same id answers the first call's result."});
    let tool = |name: &str, description: &str, properties: Value, required: &[&str], read: bool| {
        mcp_transport::tool(name, description, &properties, required, read)
    };
    vec![
        tool(
            "list_projects",
            "The projects Interface works in on this Mac (each a folder at least one Interface thread works in), with how many threads each has.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "list_threads",
            "Up to 50 Interface threads, newest activity first, with their project, provider, status and whether this session started them. `project` (a root or name from list_projects) keeps one project's.",
            json!({"project": string}),
            &[],
            true,
        ),
        tool(
            "read_thread",
            "A thread's record as compact items (prompts, answers, tool calls and results, diffs), about 30,000 characters a call; pass `next` back as `after` for more.",
            json!({"thread": thread, "after": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 2000}}),
            &["thread"],
            true,
        ),
        tool(
            "thread_status",
            "A thread's status now (running, needsYou, done, failed or stopped), with its last answer when it ended.",
            json!({"thread": thread}),
            &["thread"],
            true,
        ),
        tool(
            "wait_thread",
            "Wait (up to timeoutSec, default 120, at most 600) for a thread to finish or need the owner. Answers its status, its last answer when it ended, and whether the wait timed out; a wait that times out stops nothing.",
            json!({"thread": thread, "timeoutSec": {"type": "integer", "minimum": 0, "maximum": 600}}),
            &["thread"],
            true,
        ),
        tool(
            "start_thread",
            "Needs the owner's \"Can start and message threads\" setting. Start an ordinary Interface thread in one of Interface's projects (`project`: a root or name from list_projects; no other folder) and send it `task`. It runs in its provider's asking mode (it asks the owner before acting) without these tools, on the provider's default account. `provider` is claude (default), codex or acp:<id>; `workspace` is local (the project folder, default) or worktree (a new worktree).",
            json!({
                "project": string,
                "task": string,
                "title": string,
                "provider": string,
                "model": string,
                "workspace": {"type": "string", "enum": ["local", "worktree"]},
                "requestId": request,
            }),
            &["project", "task"],
            false,
        ),
        tool(
            "send_to_thread",
            "Needs the owner's \"Can start and message threads\" setting. Send a follow-up to a thread this session started, only while it is idle (a running thread refuses; nothing is queued).",
            json!({"thread": thread, "message": string, "requestId": request}),
            &["thread", "message"],
            false,
        ),
        tool(
            "stop_thread",
            "Needs the owner's \"Can start and message threads\" setting. Stop the running turn of a thread this session started.",
            json!({"thread": thread}),
            &["thread"],
            false,
        ),
    ]
}

/// Whether a connection whose peer runs as `peer` may be served by a
/// process running as `owner`: only the owner's own processes.
#[must_use]
pub const fn peer_allowed(peer: Option<u32>, owner: u32) -> bool {
    matches!(peer, Some(peer) if peer == owner)
}

/// The bound outside socket; dropping it stops serving and removes it.
pub struct OutsideAgentsServer {
    path: PathBuf,
    directory: PathBuf,
    identity: (u64, u64),
    stop: CancellationToken,
}

impl OutsideAgentsServer {
    /// Binds the outside socket for the Interface whose data root is
    /// `data_root` and serves it on the current runtime. Outside agents
    /// are off until the owner turns them on; the socket refuses every call
    /// until then. `None` (logged) when it cannot be bound: Interface runs
    /// without outside agents.
    #[must_use]
    pub fn start(host: ThreadHost, data_root: &Path) -> Option<Self> {
        let uid = interface_common::unix::effective_uid().ok()?;
        let path = thread_team::outside_socket_path(data_root, uid);
        Self::start_at(host, &path, uid)
    }

    /// Binds and serves `path` for processes running as `owner` (tests).
    #[must_use]
    pub fn start_at(host: ThreadHost, path: &Path, owner: u32) -> Option<Self> {
        let directory = path.parent()?.to_path_buf();
        let bound = match mcp_transport::bind(&directory, path) {
            Ok(bound) => bound,
            Err(error) => {
                tracing::warn!(%error, "the outside agents socket could not be bound; outside agents are unavailable");
                return None;
            }
        };
        let listener = match UnixListener::from_std(bound.listener) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, "the outside agents socket could not join the runtime");
                mcp_transport::remove_bound_socket(path, bound.identity);
                return None;
            }
        };
        let stop = CancellationToken::new();
        tokio::spawn(accept_loop(listener, host, owner, stop.clone()));
        Some(Self {
            path: path.to_path_buf(),
            directory,
            identity: bound.identity,
            stop,
        })
    }

    /// Where it listens.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OutsideAgentsServer {
    fn drop(&mut self) {
        self.stop.cancel();
        mcp_transport::remove_bound_socket(&self.path, self.identity);
        // Only the short fallback directory is this socket's alone.
        if self.directory.starts_with("/private/tmp") {
            let _ = std::fs::remove_dir(&self.directory);
        }
    }
}

async fn accept_loop(
    listener: UnixListener,
    host: ThreadHost,
    owner: u32,
    stop: CancellationToken,
) {
    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "an outside agents connection could not be accepted");
                tokio::select! {
                    () = stop.cancelled() => break,
                    () = tokio::time::sleep(std::time::Duration::from_millis(50)) => continue,
                }
            }
        };
        let peer = stream.peer_cred().ok().map(|credentials| credentials.uid());
        if !peer_allowed(peer, owner) {
            tracing::warn!("dropped an outside agents connection that is not the owner's");
            continue;
        }
        let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
            tracing::warn!("dropped an outside agents connection: too many are open");
            continue;
        };
        let (host, stop) = (host.clone(), stop.child_token());
        tokio::spawn(async move {
            let _permit = permit;
            serve_connection(stream, host, stop).await;
        });
    }
}

/// One outside session: `hello`, then its calls, until it disconnects.
async fn serve_connection(stream: UnixStream, host: ThreadHost, stop: CancellationToken) {
    let session = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(AsyncMutex::new(writer));
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
    let client: Arc<OnceCell<String>> = Arc::new(OnceCell::new());
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
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
                // An oversized or unreadable line ends the session.
                Some(Err(_)) | None => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(&line) else {
            write(&writer, json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32_700, "message": "Parse error"}})).await;
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "hello" => {
                let declared = params.get("client").and_then(Value::as_str);
                let body = if client.set(thread_team::client_name(declared)).is_ok() {
                    json!({"result": {
                        "protocol": OUTSIDE_PROTOCOL,
                        "level": host.outside_level().as_str(),
                    }})
                } else {
                    json!({"error": {"code": -32_600, "message": "hello was already said"}})
                };
                write(&writer, reply(id, body)).await;
            }
            "call" => {
                call(
                    &host,
                    &session,
                    client.get().cloned(),
                    id,
                    &params,
                    &writer,
                    permit,
                )
                .await;
            }
            _ => {
                write(
                    &writer,
                    reply(
                        id,
                        json!({"error": {"code": -32_601, "message": "Method not found"}}),
                    ),
                )
                .await;
            }
        }
    }
}

/// One `call`, after `hello`: answered on its own task, so a long wait
/// never holds up the session's other calls.
async fn call(
    host: &ThreadHost,
    session: &str,
    client: Option<String>,
    id: Value,
    params: &Value,
    writer: &Arc<AsyncMutex<OwnedWriteHalf>>,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let Some(client) = client else {
        let refused = json!({"error": {"code": -32_002, "message": "Say hello first."}});
        write(writer, reply(id, refused)).await;
        return;
    };
    let tool = params
        .get("tool")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let input = params.get("input").cloned().unwrap_or_else(|| json!({}));
    let Some(tool) = tool.filter(|tool| !tool.is_empty() && tool.len() <= 64) else {
        let refused = json!({"error": {"code": -32_602, "message": "Invalid params"}});
        write(writer, reply(id, refused)).await;
        return;
    };
    let (host, writer, session) = (host.clone(), Arc::clone(writer), session.to_owned());
    tokio::spawn(async move {
        let _permit = permit;
        let body = match host.outside_call(&session, &client, &tool, input).await {
            Ok(text) => json!({"result": {"text": text, "isError": false}}),
            Err(text) => json!({"result": {"text": text, "isError": true}}),
        };
        write(&writer, reply(id, body)).await;
    });
}

fn reply(id: Value, body: Value) -> Value {
    let mut response = serde_json::Map::new();
    response.insert("jsonrpc".into(), Value::String("2.0".into()));
    response.insert("id".into(), id);
    if let Value::Object(body) = body {
        response.extend(body);
    }
    Value::Object(response)
}

async fn write(writer: &AsyncMutex<OwnedWriteHalf>, value: Value) {
    let Ok(mut line) = serde_json::to_vec(&value) else {
        return;
    };
    line.push(b'\n');
    let mut writer = writer.lock().await;
    let _ = writer.write_all(&line).await;
    let _ = writer.flush().await;
}
