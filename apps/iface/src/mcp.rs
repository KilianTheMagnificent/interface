//! `iface mcp` (T3 section 11, protocol 65): a stdio MCP server a Claude Code
//! or Codex outside Interface runs (`claude mcp add interface -- iface mcp`),
//! proxying each tool call to the running Interface over its owner-only
//! outside socket. It owns no rule: what a call may do is the host's to
//! decide from the owner's setting, and who calls is the connection the host
//! accepted. This process answers `initialize` and `tools/list` itself, so a
//! client always starts; with no Interface to ask, every call answers a
//! typed refusal saying so.
//!
//! One connection serves the whole stdio session (one outside session, as
//! the host counts it). If Interface goes away, the next call connects
//! again as a new session: threads the old one started can no longer be
//! sent to or stopped from here.

use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use interface_common::sync::lock;
use interface_settings::thread_team;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{UnixStream, unix::OwnedWriteHalf},
    sync::{Mutex as AsyncMutex, oneshot},
};

use crate::errors::{CLI_EXIT, CommandResult};

pub const MCP_USAGE: &str = "Usage: iface mcp [--data-root <dir>]

Lets a Claude Code or Codex running outside Interface list, read and (when the
owner allows it) start and message Interface threads. It speaks MCP over stdio;
add it once:

  claude mcp add interface -- iface mcp
  codex mcp add interface -- iface mcp

Interface must be running, with Settings > Threads > Outside agents turned on.

Options:
  --data-root <dir>   The Interface data folder to reach (default: the installed
                      Interface's; the Dev app's is ~/Library/Application Support/Iface Dev)";

/// The longest stdio request line read: a task is at most 64 KiB.
pub const MAX_STDIO_LINE_BYTES: usize = 4 << 20;

/// What `iface mcp` was asked.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpOptions {
    pub data_root: Option<PathBuf>,
    pub help: bool,
}

/// Parses `iface mcp`'s own arguments.
///
/// # Errors
/// An unknown, repeated or incomplete option.
pub fn parse(args: &[String]) -> Result<McpOptions, String> {
    let mut options = McpOptions::default();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--help" | "-h" | "help" => options.help = true,
            "--data-root" => {
                if options.data_root.is_some() {
                    return Err("--data-root may only be specified once".into());
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or("--data-root requires a folder")?;
                let path = std::path::absolute(value)
                    .map_err(|error| format!("--data-root could not be resolved: {error}"))?;
                options.data_root = Some(path);
                index += 1;
            }
            other => return Err(format!("Unexpected argument: {other}")),
        }
        index += 1;
    }
    Ok(options)
}

/// Runs `iface mcp` on this process's stdio until its client goes away.
pub async fn run(args: &[String], default_data_root: &Path) -> CommandResult {
    let options = match parse(args) {
        Ok(options) => options,
        Err(error) => {
            return CommandResult::error(format!("{error}\n{MCP_USAGE}"), CLI_EXIT.validation);
        }
    };
    if options.help {
        return CommandResult::success(format!("{MCP_USAGE}\n"));
    }
    let Ok(uid) = interface_common::unix::effective_uid() else {
        return CommandResult::error("This user could not be identified.", CLI_EXIT.generic);
    };
    let data_root = options
        .data_root
        .unwrap_or_else(|| default_data_root.to_path_buf());
    let socket = thread_team::outside_socket_path(&data_root, uid);
    match serve(
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        socket,
        uid,
        crate::app::product_version(),
    )
    .await
    {
        Ok(()) => CommandResult::success(String::new()),
        Err(error) => CommandResult::error(error, CLI_EXIT.generic),
    }
}

/// The tools a client sees: the host's catalogue, whatever the level (a
/// call the level does not allow answers why).
fn tools() -> Vec<Value> {
    interface_host::outside_agents::catalogue()
}

const INSTRUCTIONS: &str = "Interface outside agents v1. List the projects and threads Interface works on on this Mac, read them, wait for them, and (when the owner allows it) start a thread in one of Interface's projects and message the threads you started. A thread you start is an ordinary Interface thread the owner sees and can take over; it asks the owner before acting. Every result is JSON; a refusal is {code, message}. Give start_thread and send_to_thread a stable requestId; never retry an unknown outcome with a new one.";

/// Serves MCP over `input`/`output`, reaching the host at `socket`.
///
/// # Errors
/// Reading stdin or writing stdout failed.
pub async fn serve<R, W>(
    mut input: R,
    output: W,
    socket: PathBuf,
    uid: u32,
    version: &'static str,
) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let output = Arc::new(AsyncMutex::new(output));
    let proxy = Arc::new(Proxy {
        socket,
        uid,
        client: Mutex::new(None),
        link: AsyncMutex::new(None),
    });
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let mut line = Vec::new();
        let read = (&mut input)
            .take(u64::try_from(MAX_STDIO_LINE_BYTES + 1).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut line)
            .await
            .map_err(|error| format!("stdin could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_STDIO_LINE_BYTES {
            return Err("A request was longer than iface mcp reads.".into());
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            emit(&output, error_reply(Value::Null, -32_700, "Parse error")).await?;
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
            emit(&output, error_reply(Value::Null, -32_700, "Parse error")).await?;
            continue;
        };
        // A notification gets no reply.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => {
                let declared = params
                    .pointer("/clientInfo/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                *lock(&proxy.client) = declared;
                emit(
                    &output,
                    result_reply(
                        id,
                        json!({
                            "protocolVersion": interface_collaboration::mcp::MCP_PROTOCOL_VERSION,
                            "capabilities": {"tools": {"listChanged": false}},
                            "serverInfo": {"name": "interface", "title": "Interface", "version": version},
                            "instructions": INSTRUCTIONS,
                        }),
                    ),
                )
                .await?;
            }
            "ping" => emit(&output, result_reply(id, json!({}))).await?,
            "tools/list" => emit(&output, result_reply(id, json!({"tools": tools()}))).await?,
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let Some(name) =
                    name.filter(|name| tools().iter().any(|tool| tool["name"] == name.as_str()))
                else {
                    emit(&output, error_reply(id, -32_602, "Unknown tool")).await?;
                    continue;
                };
                let (proxy, output) = (Arc::clone(&proxy), Arc::clone(&output));
                tasks.spawn(async move {
                    let (text, is_error) = proxy.call(&name, arguments).await;
                    let _ = emit(
                        &output,
                        result_reply(
                            id,
                            json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
                        ),
                    )
                    .await;
                });
            }
            _ => emit(&output, error_reply(id, -32_601, "Method not found")).await?,
        }
        while tasks.try_join_next().is_some() {}
    }
    // The client closed stdin: let calls already made answer first.
    while tasks.join_next().await.is_some() {}
    Ok(())
}

fn result_reply(id: Value, result: Value) -> Value {
    let mut reply = serde_json::Map::new();
    reply.insert("jsonrpc".into(), "2.0".into());
    reply.insert("id".into(), id);
    reply.insert("result".into(), result);
    Value::Object(reply)
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    let mut reply = serde_json::Map::new();
    reply.insert("jsonrpc".into(), "2.0".into());
    reply.insert("id".into(), id);
    reply.insert("error".into(), json!({"code": code, "message": message}));
    Value::Object(reply)
}

async fn emit<W: AsyncWrite + Unpin>(output: &AsyncMutex<W>, value: Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    line.push(b'\n');
    let mut output = output.lock().await;
    output
        .write_all(&line)
        .await
        .map_err(|error| format!("stdout could not be written: {error}"))?;
    output
        .flush()
        .await
        .map_err(|error| format!("stdout could not be written: {error}"))
}

fn refusal(code: &str, message: &str) -> (String, bool) {
    (json!({"code": code, "message": message}).to_string(), true)
}

/// Room for a host request's JSON-RPC envelope around its `params`.
const ENVELOPE_BYTES: usize = 128;

const NOT_RUNNING_MESSAGE: &str = "Interface is not running on this Mac, or is a version without outside agents, so nothing was done. Open Interface; the owner can turn outside agents on in Settings > Threads > Outside agents.";

const NOT_ACCEPTED_MESSAGE: &str = "Interface did not accept this connection, so nothing was done. It may already be serving as many outside agents as it allows (16 at once), or it was closing. Try again after another outside agent has disconnected, or once Interface is open again.";

struct Proxy {
    socket: PathBuf,
    uid: u32,
    client: Mutex<Option<String>>,
    /// The host connection, made at the first call and again after it ends.
    link: AsyncMutex<Option<Arc<Link>>>,
}

impl Proxy {
    /// One tool call: its text and whether it is an error.
    async fn call(&self, name: &str, input: Value) -> (String, bool) {
        // The host ends a session on a line longer than it reads: refuse
        // such a call here, unsent, rather than lose the session and answer
        // an unknown outcome for a call that could not have happened.
        let request = json!({"tool": name, "input": input});
        if serde_json::to_vec(&request).map_or(true, |line| {
            line.len() + ENVELOPE_BYTES > interface_host::outside_agents::MAX_LINE_BYTES
        }) {
            return refusal(
                thread_team::INVALID_INPUT,
                "This call is longer than Interface reads, so nothing was sent. Shorten the task or message.",
            );
        }
        let link = match self.link().await {
            Ok(link) => link,
            // Nothing was sent: a plain refusal, safe to try again.
            Err(NoLink::NotRunning) => {
                return refusal(thread_team::NOT_RUNNING, NOT_RUNNING_MESSAGE);
            }
            Err(NoLink::Refused) => {
                return refusal(thread_team::NOT_RUNNING, NOT_ACCEPTED_MESSAGE);
            }
        };
        match link.request("call", request).await {
            Ok(result) => {
                let text = result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let is_error = result.get("isError").and_then(Value::as_bool) != Some(false);
                (text, is_error)
            }
            Err(Lost::Refused(message)) => refusal(thread_team::UNAVAILABLE, &message),
            Err(Lost::Unsent) => refusal(thread_team::NOT_RUNNING, NOT_RUNNING_MESSAGE),
            Err(Lost::AfterSend) if thread_team::OUTSIDE_FULL_TOOLS.contains(&name) => refusal(
                thread_team::OUTCOME_UNKNOWN,
                "Interface stopped answering after the call reached it, so whether it happened is unknown. Check with list_threads or thread_status; do not try again with a new requestId.",
            ),
            Err(Lost::AfterSend) => refusal(
                thread_team::NOT_RUNNING,
                "Interface stopped answering before it replied; nothing was changed. Try again once Interface is running.",
            ),
        }
    }

    /// The live host connection, connecting (and saying hello) when there
    /// is none: no Interface at the socket, or one that took the connection
    /// and did not answer its hello.
    async fn link(&self) -> Result<Arc<Link>, NoLink> {
        let mut slot = self.link.lock().await;
        if let Some(link) = slot.as_ref().filter(|link| link.alive()) {
            return Ok(Arc::clone(link));
        }
        *slot = None;
        let stream = connect(&self.socket, self.uid)
            .await
            .ok_or(NoLink::NotRunning)?;
        let link = Link::start(stream);
        let client = lock(&self.client).clone();
        link.request("hello", json!({"client": client, "protocol": 1}))
            .await
            .map_err(|_| NoLink::Refused)?;
        *slot = Some(Arc::clone(&link));
        Ok(link)
    }
}

/// Why there is no host connection to send a call on.
enum NoLink {
    /// Nothing that is Interface's listens at the socket.
    NotRunning,
    /// An Interface took the connection and closed or refused it before its
    /// hello was answered: past its outside sessions, or closing.
    Refused,
}

/// Connects to the host's outside socket, only when it is this user's
/// socket in a directory only this user can enter.
async fn connect(socket: &Path, uid: u32) -> Option<UnixStream> {
    if !socket_is_owners(socket, uid) {
        return None;
    }
    UnixStream::connect(socket).await.ok()
}

/// The socket and its directory belong to `uid` and the directory admits
/// nobody else: a socket anyone else made is never talked to.
#[must_use]
#[allow(clippy::verbose_bit_mask)] // "No group or other bits", as the other owner-only checks say it.
pub fn socket_is_owners(socket: &Path, uid: u32) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    let Some(directory) = socket.parent() else {
        return false;
    };
    let (Ok(directory), Ok(file)) = (
        std::fs::symlink_metadata(directory),
        std::fs::symlink_metadata(socket),
    ) else {
        return false;
    };
    directory.is_dir()
        && directory.uid() == uid
        && directory.mode() & 0o077 == 0
        && file.file_type().is_socket()
        && file.uid() == uid
}

/// Why a host request has no answer.
enum Lost {
    /// The request never left this process.
    Unsent,
    /// The connection ended after the request was written.
    AfterSend,
    /// The host answered with a protocol error.
    Refused(String),
}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, Lost>>>;

/// One host connection: requests by id, answers routed by a reader task.
struct Link {
    writer: AsyncMutex<OwnedWriteHalf>,
    pending: Arc<Mutex<Option<Pending>>>,
    next: AtomicU64,
}

impl Link {
    fn start(stream: UnixStream) -> Arc<Self> {
        let (reader, writer) = stream.into_split();
        let pending: Arc<Mutex<Option<Pending>>> = Arc::new(Mutex::new(Some(HashMap::new())));
        let routed = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            loop {
                let mut line = Vec::new();
                let read = (&mut reader)
                    .take(u64::try_from(MAX_STDIO_LINE_BYTES + 1).unwrap_or(u64::MAX))
                    .read_until(b'\n', &mut line)
                    .await;
                if !matches!(read, Ok(1..)) || line.len() > MAX_STDIO_LINE_BYTES {
                    break;
                }
                let Ok(reply) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                let Some(id) = reply.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let waiter = lock(&routed)
                    .as_mut()
                    .and_then(|pending| pending.remove(&id));
                if let Some(waiter) = waiter {
                    let answer = match reply.get("result") {
                        Some(result) => Ok(result.clone()),
                        None => Err(Lost::Refused(
                            reply
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("Interface refused the request.")
                                .to_owned(),
                        )),
                    };
                    let _ = waiter.send(answer);
                }
            }
            // The connection ended: every call still waiting was sent.
            if let Some(pending) = lock(&routed).take() {
                for (_, waiter) in pending {
                    let _ = waiter.send(Err(Lost::AfterSend));
                }
            }
        });
        Arc::new(Self {
            writer: AsyncMutex::new(writer),
            pending,
            next: AtomicU64::new(1),
        })
    }

    fn alive(&self) -> bool {
        lock(&self.pending).is_some()
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, Lost> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        match lock(&self.pending).as_mut() {
            Some(pending) => pending.insert(id, sender),
            None => return Err(Lost::Unsent),
        };
        let mut line = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))
        .map_err(|_| Lost::Unsent)?;
        line.push(b'\n');
        {
            let mut writer = self.writer.lock().await;
            if writer.write_all(&line).await.is_err() || writer.flush().await.is_err() {
                // A partial write may have reached the host.
                if let Some(pending) = lock(&self.pending).as_mut() {
                    pending.remove(&id);
                }
                return Err(Lost::AfterSend);
            }
        }
        receiver.await.unwrap_or(Err(Lost::AfterSend))
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
