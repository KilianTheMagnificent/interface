//! Outside agents end to end (T3 section 11, protocol 65): the real host
//! binary on an isolated data root, a fake Claude Code, and the real
//! `iface mcp` binary speaking MCP over stdio to the host's outside socket.
//! No provider account, network or owner data.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde_json::{Value, json};

/// Generous upper bound for any one answer: the machine may be loaded.
const LIMIT: Duration = Duration::from_secs(60);

/// The hands-free kit's fake Claude Code (`electron/scripts`): scripted
/// stream-json turns, no account or network. It runs on this checkout's
/// Electron as Node (else `node`); the gate installs Electron first.
fn fake_claude() -> std::path::PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../electron/scripts/handsfree-fake-provider.sh");
    assert!(script.is_file(), "{} is missing", script.display());
    script
}

struct Host {
    child: Child,
    input: ChildStdin,
    frames: mpsc::Receiver<Value>,
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Host {
    #[allow(clippy::needless_pass_by_value)]
    fn call(&mut self, id: &str, operation: &str, payload: Value) -> Value {
        writeln!(
            self.input,
            "{}",
            json!({"version": interface_host::PROTOCOL_VERSION, "id": id,
                "operation": operation, "payload": payload})
        )
        .unwrap();
        self.input.flush().unwrap();
        loop {
            let frame = self
                .frames
                .recv_timeout(LIMIT)
                .expect("a host answer in time");
            if frame["kind"] == "response" && frame["id"] == id {
                assert_eq!(frame["ok"], true, "{frame}");
                return frame["result"].clone();
            }
        }
    }

    fn threads(&mut self, id: &str, command: Value) -> Value {
        self.call(id, "iface:threadControl", command)
    }
}

/// `iface mcp` as a client runs it: MCP over its stdio.
struct Mcp {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<Value>,
    next: u64,
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Mcp {
    #[allow(clippy::needless_pass_by_value)]
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        self.input.flush().unwrap();
        let reply = self
            .lines
            .recv_timeout(LIMIT)
            .expect("an iface mcp reply in time");
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    /// One tool call's JSON and whether it is an error.
    #[allow(clippy::needless_pass_by_value)]
    fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().expect("text content");
        (
            serde_json::from_str(text).expect("JSON text"),
            result["isError"].as_bool().expect("isError"),
        )
    }
}

fn private(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn git(project: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(project)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The workspace's `iface` binary, built beside this test (the gate's
/// `cargo test --workspace` builds it).
fn iface_binary() -> std::path::PathBuf {
    let executable = std::env::current_exe().unwrap();
    let binary = executable
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("iface");
    assert!(
        binary.is_file(),
        "{} is missing: build it with `cargo build -p iface`",
        binary.display()
    );
    binary
}

/// A Claude Code outside Interface, through the real `iface mcp`: with
/// outside agents on, it lists the thread the owner has in a project, starts
/// a thread there on (fake) Claude at its asking mode, waits for its answer
/// and reads it; the thread shows as started from outside by its client.
/// Turned off, every call is refused and nothing more starts.
#[test]
#[allow(clippy::too_many_lines)] // One outside session's whole life.
fn iface_mcp_lists_starts_waits_for_and_reads_a_thread_on_a_real_host() {
    let temporary = tempfile::Builder::new()
        .prefix("iout-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temporary.path().join("data");
    let home = temporary.path().join("home");
    let project = home.join("Desktop/Projects/atlas");
    std::fs::create_dir_all(&project).unwrap();
    private(&home);
    std::fs::write(project.join("README.md"), "atlas\n").unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "README.md"]);
    git(
        &project,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    );
    let project = std::fs::canonicalize(&project).unwrap();
    let claude = fake_claude();
    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CLAUDE_CODE_EXECUTABLE", &claude)
        .env("CODEX_EXECUTABLE", temporary.path().join("no-codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GROK_HOME", home.join(".grok"))
        .env("SHELL", super::child_protocol::NO_LOGIN_SHELL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (sender, frames) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && sender.send(value).is_err()
            {
                break;
            }
        }
    });
    let mut host = Host {
        child,
        input,
        frames,
    };
    let ready = host.frames.recv_timeout(LIMIT).unwrap();
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["version"], 72);

    // The owner has one thread in the project (under the default folder,
    // which Interface opens): prepared, launched, bound.
    host.call(
        "default-directory",
        "iface:getDefaultDirectory",
        Value::Null,
    );
    let prepared = host.threads(
        "prepare",
        json!({"kind": "prepareWorkspace", "projectRoot": project, "mode": "local", "hint": "seed"}),
    );
    let token = prepared["prepared"]["token"].as_str().unwrap().to_owned();
    let created = host.call(
        "create",
        "iface:createSession",
        json!({"engine": "claude", "options": {"cwd": project, "prompt": "seed the project",
            "kind": "code", "permissionMode": "acceptEdits"}}),
    );
    let seed = created["key"].as_str().unwrap().to_owned();
    host.threads(
        "bind",
        json!({"kind": "bind", "key": seed, "token": token, "prompt": "seed the project"}),
    );
    let settings = |level: &str| {
        json!({"kind": "setSettings", "settings": {"autoSettleDays": 3, "settleOnMerge": true,
            "generatedTitles": false, "trackPullRequests": true, "outsideAgents": level}})
    };

    let socket = root.join("team").join("outside.sock");
    assert!(socket.exists(), "the host binds its outside socket");
    let mut command = Command::new(iface_binary());
    command
        .arg("mcp")
        .arg("--data-root")
        .arg(&root)
        .env("HOME", &home)
        .current_dir(&home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = command.spawn().unwrap();
    let mcp_input = process.stdin.take().unwrap();
    let mcp_output = process.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(mcp_output).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && sender.send(value).is_err()
            {
                break;
            }
        }
    });
    let mut mcp = Mcp {
        child: process,
        input: mcp_input,
        lines,
        next: 0,
    };
    let initialized = mcp.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "e2e-client", "version": "1.0.0"}}),
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], "interface");

    // Off by default: refused, and nothing is listed or started.
    let (refused, is_error) = mcp.tool("list_threads", json!({}));
    assert!(is_error);
    assert_eq!(refused["code"], "outside-agents-off", "{refused}");

    host.threads("full", settings("full"));
    let (projects, _) = mcp.tool("list_projects", json!({}));
    assert_eq!(
        projects["projects"],
        json!([{"name": "atlas", "project": project, "threads": 1}])
    );
    let (listed, is_error) = mcp.tool("list_threads", json!({}));
    assert!(!is_error, "{listed}");
    let rows = listed["threads"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["thread"], seed.as_str());
    assert_eq!(rows[0]["project"], json!(project));

    // A folder outside Interface's projects is refused before anything runs.
    let (refused, _) = mcp.tool(
        "start_thread",
        json!({"project": temporary.path(), "task": "anything"}),
    );
    assert_eq!(refused["code"], "unknown-project", "{refused}");

    let task = "write outside.txt hello from outside";
    let (started, is_error) = mcp.tool(
        "start_thread",
        json!({"project": "atlas", "task": task, "requestId": "e2e-1"}),
    );
    assert!(!is_error, "{started}");
    let thread = started["thread"].as_str().unwrap().to_owned();
    assert!(thread.starts_with("claude#"), "{thread}");
    let (waited, _) = mcp.tool("wait_thread", json!({"thread": thread, "timeoutSec": 60}));
    assert_eq!(waited["status"], "done", "{waited}");
    assert_eq!(waited["answer"], "Wrote `outside.txt`.", "{waited}");
    // It worked in the project's folder.
    assert_eq!(
        std::fs::read_to_string(project.join("outside.txt")).unwrap(),
        "hello from outside\n"
    );
    let (read, _) = mcp.tool("read_thread", json!({"thread": thread}));
    let items = read["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "prompt" && item["text"].as_str().unwrap().contains(task)),
        "{read}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "answer" && item["text"] == "Wrote `outside.txt`."),
        "{read}"
    );
    // Its mode is Claude's asking one, not the seed thread's acceptEdits:
    // the access store keeps only modes other than the default.
    let access: Value =
        serde_json::from_slice(&std::fs::read(root.join("session-access.v1.json")).unwrap())
            .unwrap();
    assert_eq!(access["sessions"][&seed]["mode"], "acceptEdits", "{access}");
    assert!(access["sessions"].get(&thread).is_none(), "{access}");

    // The owner sees it as an ordinary thread, started from outside.
    let view = host.threads("read", json!({"kind": "read"}));
    let shown = view["view"]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == thread.as_str())
        .unwrap()
        .clone();
    assert_eq!(shown["startedFromOutside"], "e2e-client");
    assert_eq!(view["view"]["settings"]["outsideAgents"], "full");

    // Read only, then off: refused at once, for this same session.
    host.threads("read-only", settings("read"));
    let (refused, _) = mcp.tool(
        "send_to_thread",
        json!({"thread": thread, "message": "more"}),
    );
    assert_eq!(refused["code"], "read-only", "{refused}");
    host.threads("off", settings("off"));
    let (refused, _) = mcp.tool("read_thread", json!({"thread": thread}));
    assert_eq!(refused["code"], "outside-agents-off", "{refused}");

    // The host gone, iface mcp still answers, with a refusal saying so.
    drop(host);
    let (refused, is_error) = mcp.tool("list_threads", json!({}));
    assert!(is_error);
    assert_eq!(refused["code"], "interface-not-running", "{refused}");
}
