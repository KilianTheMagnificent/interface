//! `iface mcp` against a fake host socket: MCP framing on stdio, the hello
//! that names the client once per session, refusals with no Interface, and
//! fail-closed answers when the host goes away mid-call.

use std::{os::unix::fs::PermissionsExt, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf},
    net::UnixListener,
    task::JoinHandle,
};

use super::*;

/// Generous upper bound for any one reply (the machine may be loaded).
const LIMIT: Duration = Duration::from_secs(30);

struct Client {
    writer: WriteHalf<DuplexStream>,
    lines: tokio::io::Lines<BufReader<ReadHalf<DuplexStream>>>,
    served: JoinHandle<Result<(), String>>,
}

impl Client {
    async fn send(&mut self, value: Value) {
        self.writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn next(&mut self) -> Value {
        let line = tokio::time::timeout(LIMIT, self.lines.next_line())
            .await
            .expect("a reply in time")
            .unwrap()
            .expect("a reply line");
        serde_json::from_str(&line).unwrap()
    }

    async fn call(&mut self, id: u64, name: &str, arguments: Value) -> (Value, bool) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}}))
            .await;
        let reply = self.next().await;
        assert_eq!(reply["id"], id, "{reply}");
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            serde_json::from_str(text).unwrap(),
            result["isError"].as_bool().unwrap(),
        )
    }

    async fn close(mut self) {
        // The client closes its end: iface mcp reads end of input.
        self.writer.shutdown().await.unwrap();
        tokio::time::timeout(LIMIT, self.served)
            .await
            .expect("iface mcp ends with its client")
            .unwrap()
            .unwrap();
    }
}

fn start(socket: PathBuf) -> Client {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let (read, write) = tokio::io::split(theirs);
    let uid = interface_common::unix::effective_uid().unwrap();
    let served = tokio::spawn(serve(
        BufReader::new(read),
        write,
        socket,
        uid,
        "9.9.9-test",
    ));
    let (reader, writer) = tokio::io::split(ours);
    Client {
        writer,
        lines: BufReader::new(reader).lines(),
        served,
    }
}

/// An owner-private directory with a socket path in it.
fn private_socket() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("team");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    (temporary, directory.join("outside.sock"))
}

/// What the fake host saw: each connection's hello and calls.
#[derive(Default)]
struct Seen {
    hellos: Vec<Value>,
    calls: Vec<Value>,
}

/// A fake host: answers hello, then each call with `answer(tool, input)`,
/// or closes the connection when that answers `None`.
fn fake_host(
    socket: &Path,
    answer: impl Fn(&str, &Value) -> Option<Value> + Send + Sync + 'static,
) -> Arc<Mutex<Seen>> {
    let listener = UnixListener::bind(socket).unwrap();
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let recorded = Arc::clone(&seen);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (seen, answer) = (Arc::clone(&recorded), Arc::clone(&answer));
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let id = request["id"].clone();
                    let reply = match request["method"].as_str() {
                        Some("hello") => {
                            lock(&seen).hellos.push(request["params"].clone());
                            json!({"jsonrpc": "2.0", "id": id, "result": {"protocol": 1, "level": "full"}})
                        }
                        Some("call") => {
                            lock(&seen).calls.push(request["params"].clone());
                            let params = &request["params"];
                            match answer(params["tool"].as_str().unwrap(), &params["input"]) {
                                Some(result) => {
                                    json!({"jsonrpc": "2.0", "id": id, "result": result})
                                }
                                None => return,
                            }
                        }
                        _ => {
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no"}})
                        }
                    };
                    writer
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                }
            });
        }
    });
    seen
}

/// With no Interface running, `initialize` and `tools/list` still answer
/// (the client starts), a notification gets no reply, and every tool call
/// is a typed refusal saying how to turn outside agents on.
#[tokio::test]
async fn iface_mcp_starts_and_refuses_every_call_when_no_interface_runs() {
    let (_temporary, socket) = private_socket();
    let mut client = start(socket);
    client
        .send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "claude-code", "version": "2.1.0"}}}),
        )
        .await;
    let initialized = client.next().await;
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["protocolVersion"],
        interface_collaboration::mcp::MCP_PROTOCOL_VERSION
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], "interface");
    assert_eq!(initialized["result"]["serverInfo"]["version"], "9.9.9-test");
    // A notification: no reply. The next reply is the list's.
    client
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    client
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .await;
    let listed = client.next().await;
    assert_eq!(listed["id"], 2);
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        thread_team::OUTSIDE_READ_TOOLS
            .iter()
            .chain(thread_team::OUTSIDE_FULL_TOOLS.iter())
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    );
    for tool in listed["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        // No tool names its caller or its level.
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        for forbidden in [
            "caller", "client", "level", "session", "mode", "cwd", "path",
        ] {
            assert!(!properties.contains_key(forbidden), "{forbidden}");
        }
    }
    for (id, tool) in [(3, "list_threads"), (4, "start_thread")] {
        let (refused, is_error) = client.call(id, tool, json!({})).await;
        assert!(is_error);
        assert_eq!(refused["code"], thread_team::NOT_RUNNING, "{refused}");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("Settings > Threads > Outside agents")
        );
    }
    // An unknown tool and an unknown method are protocol errors.
    client
        .send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "team_info"}}))
        .await;
    assert_eq!(client.next().await["error"]["code"], -32_602);
    client
        .send(json!({"jsonrpc": "2.0", "id": 6, "method": "resources/list"}))
        .await;
    assert_eq!(client.next().await["error"]["code"], -32_601);
    client.close().await;
}

/// Calls go to the host over one connection that said hello once with the
/// client's declared name; the host's text and error flag come back as the
/// tool result; the caller's input is passed as given.
#[tokio::test]
async fn iface_mcp_proxies_calls_over_one_session_named_by_its_client() {
    let (_temporary, socket) = private_socket();
    let seen = fake_host(&socket, |tool, _| {
        Some(match tool {
            "start_thread" => {
                json!({"text": json!({"code": "outside-agents-off", "message": "off"}).to_string(), "isError": true})
            }
            _ => json!({"text": json!({"threads": []}).to_string(), "isError": false}),
        })
    });
    let mut client = start(socket);
    client
        .send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "codex-mcp-client"}}}),
        )
        .await;
    client.next().await;
    let (listed, is_error) = client
        .call(2, "list_threads", json!({"project": "atlas"}))
        .await;
    assert!(!is_error);
    assert_eq!(listed, json!({"threads": []}));
    let (refused, is_error) = client
        .call(3, "start_thread", json!({"project": "atlas", "task": "t"}))
        .await;
    assert!(is_error);
    assert_eq!(refused["code"], "outside-agents-off");
    client.close().await;
    let seen = lock(&seen);
    assert_eq!(seen.hellos.len(), 1, "one session");
    assert_eq!(seen.hellos[0]["client"], "codex-mcp-client");
    assert_eq!(
        seen.calls,
        [
            json!({"tool": "list_threads", "input": {"project": "atlas"}}),
            json!({"tool": "start_thread", "input": {"project": "atlas", "task": "t"}}),
        ]
    );
}

/// A call the host received but never answered (it went away) is an
/// unknown outcome for a changing tool, never a refusal to retry; a read
/// is safe to retry; the next call connects again as a new session.
#[tokio::test]
async fn a_call_lost_after_it_reached_interface_is_an_unknown_outcome() {
    let (_temporary, socket) = private_socket();
    let seen = fake_host(&socket, |tool, _| {
        (tool == "thread_status").then(|| json!({"text": "{}", "isError": false}))
    });
    let mut client = start(socket);
    let (unknown, is_error) = client
        .call(
            1,
            "start_thread",
            json!({"project": "atlas", "task": "t", "requestId": "r1"}),
        )
        .await;
    assert!(is_error);
    assert_eq!(unknown["code"], thread_team::OUTCOME_UNKNOWN, "{unknown}");
    let (retry, _) = client.call(2, "list_threads", json!({})).await;
    assert_eq!(retry["code"], thread_team::NOT_RUNNING, "{retry}");
    let (status, is_error) = client
        .call(3, "thread_status", json!({"thread": "claude#a"}))
        .await;
    assert!(!is_error);
    assert_eq!(status, json!({}));
    client.close().await;
    assert_eq!(lock(&seen).hellos.len(), 3, "a new session after each loss");
}

/// `iface mcp` talks only to this user's socket in a directory nobody else
/// can enter: anything else is treated as no Interface at all.
#[tokio::test]
async fn iface_mcp_never_talks_to_a_socket_others_could_reach() {
    let (_temporary, socket) = private_socket();
    let seen = fake_host(&socket, |_, _| {
        Some(json!({"text": "{}", "isError": false}))
    });
    let uid = interface_common::unix::effective_uid().unwrap();
    assert!(socket_is_owners(&socket, uid));
    assert!(!socket_is_owners(&socket, uid + 1));
    let directory = socket.parent().unwrap();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!socket_is_owners(&socket, uid));
    let mut client = start(socket.clone());
    let (refused, _) = client.call(1, "list_projects", json!({})).await;
    assert_eq!(refused["code"], thread_team::NOT_RUNNING);
    client.close().await;
    assert!(lock(&seen).hellos.is_empty(), "never connected");
    // A plain file is not a socket either.
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = directory.join("plain");
    std::fs::write(&file, b"x").unwrap();
    assert!(!socket_is_owners(&file, uid));
}

/// A call longer than the host reads in one line would end the session
/// there (the host drops a connection on an over-long line): the call would
/// read as an unknown outcome although nothing could have happened, and the
/// session's started threads would leave its control. `iface mcp` refuses
/// it before sending, and the session goes on.
#[tokio::test]
async fn a_call_longer_than_interface_reads_is_refused_unsent_and_the_session_goes_on() {
    let (_temporary, socket) = private_socket();
    let seen = fake_host(&socket, |_, _| {
        Some(json!({"text": json!({"status": "running"}).to_string(), "isError": false}))
    });
    let mut client = start(socket);
    let (sent, is_error) = client
        .call(
            1,
            "send_to_thread",
            json!({"thread": "claude#a", "message": "hi"}),
        )
        .await;
    assert!(!is_error, "{sent}");
    let long = "x".repeat(interface_host::outside_agents::MAX_LINE_BYTES + 1);
    let (refused, is_error) = client
        .call(
            2,
            "send_to_thread",
            json!({"thread": "claude#a", "message": long}),
        )
        .await;
    assert!(is_error);
    assert_eq!(refused["code"], thread_team::INVALID_INPUT, "{refused}");
    let (again, is_error) = client
        .call(
            3,
            "send_to_thread",
            json!({"thread": "claude#a", "message": "hi"}),
        )
        .await;
    assert!(!is_error, "{again}");
    client.close().await;
    let seen = lock(&seen);
    assert_eq!(seen.calls.len(), 2, "the long call was never sent");
    assert_eq!(seen.hellos.len(), 1, "one session throughout");
}

/// An Interface that is running but closes a new outside connection before
/// answering (it already serves the most outside sessions it allows) is not
/// "not running": the refusal says what may be the case, and nothing was
/// sent.
#[tokio::test]
async fn a_connection_interface_closes_unanswered_is_not_called_interface_not_running() {
    let (_temporary, socket) = private_socket();
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let accepted = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&accepted);
    tokio::spawn(async move {
        // As the host past its 16 sessions: accept, then drop at once.
        while let Ok((stream, _)) = listener.accept().await {
            *lock(&counted) += 1;
            drop(stream);
        }
    });
    let mut client = start(socket);
    let (refused, is_error) = client
        .call(1, "start_thread", json!({"project": "atlas", "task": "t"}))
        .await;
    assert!(is_error);
    assert_eq!(refused["code"], thread_team::NOT_RUNNING, "{refused}");
    let message = refused["message"].as_str().unwrap();
    assert!(!message.contains("is not running"), "{message}");
    assert!(message.contains("16"), "{message}");
    assert!(message.contains("nothing was done"), "{message}");
    client.close().await;
    assert_eq!(*lock(&accepted), 1);
}
