//! The `iface_voice` MCP server: one owner-only Unix socket per voice
//! session, reached by the Claude CLI through `/usr/bin/nc -U <socket>` the
//! way the collaboration broker is. Transport and framing live in
//! `crate::mcp_transport`; this file is the voice tool table and the
//! per-session socket lifecycle.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use engine_claude::{McpServer, VOICE_READ_ONLY_TOOLS, VoiceMcpProvider};
use interface_common::sync::lock;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::mcp_transport::{
    BoundSocket, MAX_UNIX_SOCKET_PATH_BYTES, McpServerSpec, ToolBackend, bind, descriptor,
    remove_bound_socket, serve, tool,
};

pub(crate) use crate::mcp_transport::ToolOutput;

const MCP_SERVER_WIRE_NAME: &str = "iface-voice";
const SOCKET_DIRECTORY: &str = "voice/sockets";
const SOCKET_ID_CHARS: usize = 12;
const INSTRUCTIONS: &str = "Tools that observe and operate the Interface desktop app on behalf of the voice assistant. Call app_state before acting on panes or sessions.";

#[cfg(test)]
const TOOL_NAMES: [&str; 16] = [
    "app_state",
    "list_panes",
    "start_pane",
    "send_to_pane",
    "read_pane",
    "close_pane",
    "list_sessions",
    "read_session",
    "send_to_session",
    "notify",
    "ui_snapshot",
    "ui_act",
    "ui_screenshot",
    "open_project",
    "focus_pane",
    "list_profiles",
];

/// The provider slot bootstrap installs on the Claude adapter before the
/// voice service, which needs the runtime and dispatchers, can exist.
#[derive(Default)]
pub struct DeferredVoiceMcp {
    inner: Mutex<Option<Arc<dyn VoiceMcpProvider>>>,
}

impl DeferredVoiceMcp {
    pub fn attach(&self, provider: Arc<dyn VoiceMcpProvider>) {
        *lock(&self.inner) = Some(provider);
    }
}

impl VoiceMcpProvider for DeferredVoiceMcp {
    fn descriptor_for(&self, session_key: &str, cwd: &Path) -> Option<McpServer> {
        let provider = lock(&self.inner).clone()?;
        provider.descriptor_for(session_key, cwd)
    }
}

struct BoundServer {
    path: PathBuf,
    identity: (u64, u64),
    stop: CancellationToken,
}

pub(crate) struct VoiceMcpServers {
    root: PathBuf,
    spec: Arc<McpServerSpec>,
    backend: Arc<dyn ToolBackend>,
    servers: Mutex<HashMap<String, BoundServer>>,
}

impl VoiceMcpServers {
    /// Must be called inside a Tokio runtime; accept loops run on it.
    pub(crate) fn new(data_root: &Path, backend: Arc<dyn ToolBackend>) -> Self {
        Self {
            root: data_root.join(SOCKET_DIRECTORY),
            spec: Arc::new(McpServerSpec {
                wire_name: MCP_SERVER_WIRE_NAME,
                title: "Interface Voice Assistant",
                instructions: INSTRUCTIONS,
                tools: tool_catalogue(),
                compatibility_tools: &[],
                max_request_line_bytes: None,
            }),
            backend,
            servers: Mutex::new(HashMap::new()),
        }
    }

    /// Binds (or reuses) the socket for one session and returns the stdio
    /// bridge descriptor. `None` when the socket cannot be created; the
    /// session then launches without tools.
    pub(crate) fn descriptor_for(&self, session_key: &str) -> Option<McpServer> {
        let session_id = session_key.rsplit('#').next().unwrap_or(session_key);
        let mut servers = lock(&self.servers);
        if let Some(server) = servers.get(session_id) {
            if server.path.exists() {
                return Some(descriptor(&server.path));
            }
            server.stop.cancel();
            servers.remove(session_id);
        }
        let path = self.root.join(format!("{}.sock", socket_id(session_id)));
        if path.as_os_str().len() > MAX_UNIX_SOCKET_PATH_BYTES {
            tracing::error!(path = %path.display(), "voice MCP socket path exceeds the Unix socket limit");
            return None;
        }
        let BoundSocket { listener, identity } = match bind(&self.root, &path) {
            Ok(bound) => bound,
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "voice MCP socket could not be bound");
                return None;
            }
        };
        let stop = CancellationToken::new();
        serve(
            listener,
            &path,
            Arc::clone(&self.spec),
            Arc::clone(&self.backend),
            stop.clone(),
        );
        servers.insert(
            session_id.to_owned(),
            BoundServer {
                path: path.clone(),
                identity,
                stop,
            },
        );
        Some(descriptor(&path))
    }

    pub(crate) fn stop_all(&self) {
        for (_, server) in lock(&self.servers).drain() {
            server.stop.cancel();
            remove_bound_socket(&server.path, server.identity);
        }
    }
}

/// The socket name keeps a prefix of the session id: a full UUID would push
/// `~/Library/Application Support/Interface/voice/sockets/<id>.sock` past
/// the 103-byte Unix socket path limit.
fn socket_id(session_id: &str) -> String {
    let id: String = session_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(SOCKET_ID_CHARS)
        .collect();
    if id.is_empty() {
        "voice".to_owned()
    } else {
        id
    }
}

/// The two pane tools the pane server offers as well, defined once. Each
/// server words them for its own reach and names where its pane ids come from.
pub(crate) fn send_to_pane_tool(description: &str, pane_id: &str) -> Value {
    voice_tool(
        "send_to_pane",
        description,
        &json!({
            "pane_id": pane_id_schema(pane_id),
            "text": {"type": "string"},
            "enter": {"type": "boolean", "default": true}
        }),
        &["pane_id", "text"],
    )
}

pub(crate) fn read_pane_tool(description: &str, pane_id: &str) -> Value {
    voice_tool(
        "read_pane",
        description,
        &json!({
            "pane_id": pane_id_schema(pane_id),
            "lines": {"type": "integer", "minimum": 1, "maximum": 400, "default": 60}
        }),
        &["pane_id"],
    )
}

/// Where the voice tools' pane ids come from.
const VOICE_PANE_ID: &str = "pane_id from app_state or list_panes";

fn pane_id_schema(source: &str) -> Value {
    json!({"type": "string", "description": source})
}

#[allow(clippy::too_many_lines)]
pub(crate) fn tool_catalogue() -> Vec<Value> {
    let pane_id = pane_id_schema(VOICE_PANE_ID);
    let session_key =
        json!({"type": "string", "description": "session_key from app_state or list_sessions"});
    vec![
        voice_tool(
            "app_state",
            "Projects, sessions and terminal panes currently open in Interface. Lists the 50 most recently active sessions; sessions_total counts them all. Call this before acting on panes or sessions.",
            &json!({}),
            &[],
        ),
        voice_tool("list_panes", "The open terminal panes.", &json!({}), &[]),
        voice_tool(
            "start_pane",
            "Open a new terminal pane running the given profile in a project directory.",
            &json!({
                "profile": {"type": "string", "enum": ["shell", "claude", "codex", "grok", "qwen", "antigravity", "omp", "opencode", "muse", "kimi"]},
                "cwd": {"type": "string", "description": "absolute project directory"},
                "title": {"type": "string"}
            }),
            &["profile", "cwd"],
        ),
        send_to_pane_tool(
            "Type text into a pane; presses Enter afterwards unless enter is false.",
            VOICE_PANE_ID,
        ),
        read_pane_tool(
            "The last lines of a pane's output, with terminal colours stripped.",
            VOICE_PANE_ID,
        ),
        voice_tool(
            "close_pane",
            "Close a terminal pane. This terminates the program running in it, including an agent mid-task.",
            &json!({"pane_id": pane_id}),
            &["pane_id"],
        ),
        voice_tool(
            "list_sessions",
            "The chat sessions Interface knows about, the most recently active first, at most 50. total counts every match; pass query to find older sessions by title, folder or engine.",
            &json!({
                "query": {"type": "string", "description": "words from the session's title, folder or engine"}
            }),
            &[],
        ),
        voice_tool(
            "read_session",
            "The last turns of a session as user:/assistant: lines.",
            &json!({
                "session_key": session_key,
                "last_n": {"type": "integer", "minimum": 1, "maximum": 50, "default": 10}
            }),
            &["session_key"],
        ),
        voice_tool(
            "send_to_session",
            "Send a message to a chat session as the user.",
            &json!({"session_key": session_key, "text": {"type": "string"}}),
            &["session_key", "text"],
        ),
        voice_tool(
            "notify",
            "Show a short notification in Interface.",
            &json!({"title": {"type": "string"}, "body": {"type": "string"}}),
            &["title", "body"],
        ),
        voice_tool(
            "ui_snapshot",
            "A text tree of the visible Interface window: one element per line with an id, role, label and state.",
            &json!({}),
            &[],
        ),
        voice_tool(
            "ui_act",
            "Act on an element from the latest ui_snapshot.",
            &json!({
                "id": {"type": "string", "description": "element id from ui_snapshot"},
                "action": {"type": "string", "enum": ["click", "type", "key", "focus"]},
                "text": {"type": "string", "description": "text to type or key name to press"}
            }),
            &["id", "action"],
        ),
        voice_tool(
            "ui_screenshot",
            "A PNG screenshot of the Interface window.",
            &json!({}),
            &[],
        ),
        voice_tool(
            "open_project",
            "Select a project in the sidebar by its root directory.",
            &json!({"root": {"type": "string"}}),
            &["root"],
        ),
        voice_tool(
            "focus_pane",
            "Bring a terminal pane to the front.",
            &json!({"pane_id": pane_id}),
            &["pane_id"],
        ),
        voice_tool(
            "list_profiles",
            "List available terminal agent profiles from the host roster.",
            &json!({}),
            &[],
        ),
    ]
}

fn voice_tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    tool(
        name,
        description,
        properties,
        required,
        VOICE_READ_ONLY_TOOLS.contains(&name),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use async_trait::async_trait;
    use tokio::{io::AsyncBufReadExt, io::BufReader, net::UnixStream};

    use super::*;
    use crate::mcp_transport::{MCP_PROTOCOL_VERSION, NC, Peer, test_support::exchange};

    struct EchoBackend;

    #[async_trait]
    impl ToolBackend for EchoBackend {
        async fn call(&self, name: &str, input: Value, _peer: Peer) -> Result<ToolOutput, String> {
            match name {
                "app_state" => Ok(ToolOutput::Text(format!("state {input}"))),
                "ui_screenshot" => Ok(ToolOutput::Image {
                    png_base64: "AAAA".into(),
                    text: "captured 1x1".into(),
                }),
                _ => Err(format!("{name} refused")),
            }
        }
    }

    #[test]
    fn catalogue_marks_exactly_the_read_only_tools_and_closes_every_schema() {
        let catalogue = tool_catalogue();
        let names = catalogue
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, TOOL_NAMES);
        let read_only = catalogue
            .iter()
            .filter(|tool| tool["annotations"]["readOnlyHint"] == true)
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(read_only, VOICE_READ_ONLY_TOOLS.into_iter().collect());
        for tool in &catalogue {
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{}",
                tool["name"]
            );
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        assert_eq!(
            socket_id("0a1b2c3d-4e5f-6789-abcd-ef0123456789"),
            "0a1b2c3d4e5f"
        );
        assert_eq!(socket_id("#"), "voice");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn serves_initialize_tools_list_and_tools_call_over_the_session_socket() {
        let temporary = tempfile::tempdir().unwrap();
        let servers = VoiceMcpServers::new(temporary.path(), Arc::new(EchoBackend));
        let descriptor = servers
            .descriptor_for("claude#0a1b2c3d-4e5f-6789-abcd-ef0123456789")
            .expect("socket bound");
        assert_eq!(descriptor.command, PathBuf::from(NC));
        assert_eq!(descriptor.args[0], "-U");
        assert_eq!(
            servers
                .descriptor_for("claude#0a1b2c3d-4e5f-6789-abcd-ef0123456789")
                .unwrap()
                .args,
            descriptor.args
        );
        let socket = PathBuf::from(&descriptor.args[1]);
        assert!(socket.ends_with("voice/sockets/0a1b2c3d4e5f.sock"));

        let stream = UnixStream::connect(&socket).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialized = exchange(&mut writer, &mut lines, json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}
        }))
        .await;
        assert_eq!(initialized["id"], 1);
        assert_eq!(
            initialized["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "iface-voice");
        let listed = exchange(
            &mut writer,
            &mut lines,
            json!({"jsonrpc": "2.0", "id": "two", "method": "tools/list"}),
        )
        .await;
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 16);
        let called = exchange(
            &mut writer,
            &mut lines,
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "app_state", "arguments": {}}
            }),
        )
        .await;
        assert_eq!(called["result"]["content"][0]["text"], "state {}");
        let image = exchange(
            &mut writer,
            &mut lines,
            json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {"name": "ui_screenshot"}
            }),
        )
        .await;
        assert_eq!(image["result"]["content"][0]["type"], "image");
        assert_eq!(image["result"]["content"][1]["text"], "captured 1x1");
        let refused = exchange(
            &mut writer,
            &mut lines,
            json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": {"name": "close_pane", "arguments": {"pane_id": "p"}}
            }),
        )
        .await;
        assert_eq!(refused["result"]["isError"], true);
        assert_eq!(
            refused["result"]["content"][0]["text"],
            "close_pane refused"
        );
        let unknown = exchange(
            &mut writer,
            &mut lines,
            json!({
                "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": {"name": "rm_rf"}
            }),
        )
        .await;
        assert_eq!(unknown["error"]["code"], -32_602);
        let missing = exchange(
            &mut writer,
            &mut lines,
            json!({"jsonrpc": "2.0", "id": 7, "method": "resources/list"}),
        )
        .await;
        assert_eq!(missing["error"]["code"], -32_601);

        servers.stop_all();
        assert!(!socket.exists());
    }
}
