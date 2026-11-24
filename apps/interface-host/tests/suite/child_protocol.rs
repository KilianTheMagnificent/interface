use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// The spawned host's `SHELL`. Its provider lookup asks the login shell for
/// `PATH`, which would run the developer's own startup files (up to the 5 s
/// lookup bound) before the ready frame, to find providers these tests fake
/// by absolute path anyway. `true` prints no `PATH`, so nothing is found.
pub(crate) const NO_LOGIN_SHELL: &str = "/usr/bin/true";

#[test]
#[allow(clippy::too_many_lines)]
fn real_child_routes_desktop_and_runtime_smoke_on_injected_state() {
    let temporary = tempfile::tempdir().expect("temporary child root");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory fixture");
    let mut permissions = std::fs::metadata(&home)
        .expect("home metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&home, permissions).expect("private test home");
    let codex = temporary.path().join("fake-codex");
    std::fs::write(
        &codex,
        r#"#!/usr/bin/env python3
import json, sys
if "--version" in sys.argv:
    print("codex-cli 0.0.0-test")
    raise SystemExit(0)
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        with open(sys.argv[0] + ".client-version", "w") as recorded:
            recorded.write(message["params"]["clientInfo"]["version"])
        result = {"serverInfo":{"version":"test"}}
    elif method == "thread/list":
        result = {"data":[]}
    elif method == "model/list":
        result = {"data":[]}
    elif method == "account/read":
        result = {"account":None}
    else:
        result = {}
    if message.get("id") is not None:
        print(json.dumps({"id":message["id"],"result":result}), flush=True)
"#,
    )
    .expect("fake Codex executable");
    let mut codex_permissions = std::fs::metadata(&codex)
        .expect("fake Codex metadata")
        .permissions();
    codex_permissions.set_mode(0o700);
    std::fs::set_permissions(&codex, codex_permissions).expect("fake Codex mode");

    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CODEX_EXECUTABLE", &codex)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GROK_HOME", home.join(".grok"))
        .env("SHELL", NO_LOGIN_SHELL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn real interface-host child");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    let ready = read_frame(&mut stdout);
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["version"], 72);
    assert_eq!(ready["hostVersion"], interface_common::product_version!());
    assert_eq!(ready["operations"], 127);
    assert_eq!(ready["pushes"], 12);

    let project = request(
        &mut stdin,
        &mut stdout,
        "create-project",
        "iface:createProject",
        &json!({"name":"Created project"}),
    );
    let created = home.join("Developer/Projects/Created project");
    assert_eq!(
        project,
        json!(std::fs::canonicalize(&created).expect("created project"))
    );
    assert_eq!(
        std::fs::read_dir(&created).expect("empty project").count(),
        0
    );
    let duplicate = request_frame(
        &mut stdin,
        &mut stdout,
        "duplicate-project",
        "iface:createProject",
        &json!({"name":"Created project"}),
    );
    assert_eq!(duplicate["ok"], false);
    assert_eq!(duplicate["error"]["code"], "request-refused");

    // A saved pane whose folder moved and left a link behind resolves to the
    // new home over the real wire; the move itself rides the roster save.
    let moved_home = home.join("Developer/Created project");
    std::fs::create_dir_all(moved_home.parent().expect("moved parent")).expect("moved parent");
    std::fs::rename(&created, &moved_home).expect("move the project");
    std::os::unix::fs::symlink(&moved_home, &created).expect("link left behind");
    let moved_target = std::fs::canonicalize(&moved_home).expect("canonical moved home");
    let roster = |cwd: &Path| {
        json!({"workspaces":[{
            "id":"moved","name":"Moved","createdAt":"2026-09-23T00:00:00Z",
            "updatedAt":"2026-09-23T00:00:00Z",
            "terminals":[{"id":"6b1f8c1e-4a8e-4f3b-9d7a-2c5e8f0a1b3c","title":"Shell","cwd":cwd}]
        }]})
    };
    request(
        &mut stdin,
        &mut stdout,
        "save-linked-roster",
        "iface:setTerminalWorkspaces",
        &roster(&created),
    );
    assert_eq!(
        request(
            &mut stdin,
            &mut stdout,
            "resolve-moved-folder",
            "iface:resolveMovedTerminalFolder",
            &json!({"path":created}),
        ),
        json!(moved_target)
    );
    let mut moved_roster = roster(&moved_target);
    moved_roster["movedFolders"] = json!([{"from":created,"to":moved_target}]);
    let saved = request(
        &mut stdin,
        &mut stdout,
        "save-moved-roster",
        "iface:setTerminalWorkspaces",
        &moved_roster,
    );
    assert_eq!(saved[0]["terminals"][0]["cwd"], json!(moved_target));

    let agent_state = request(
        &mut stdin,
        &mut stdout,
        "agent-read",
        "iface:agentControl",
        &json!({"kind":"read"}),
    );
    assert_eq!(agent_state["revision"], 0);
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../electron/src/fixtures/agent-workspace-v14.json"
    ))
    .expect("shared agent fixture");
    let saved = request(
        &mut stdin,
        &mut stdout,
        "agent-save",
        "iface:agentControl",
        &fixture["commands"][2],
    );
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["agents"][0]["shape"], "star");
    let routine = request(
        &mut stdin,
        &mut stdout,
        "routine-save",
        "iface:agentControl",
        &fixture["commands"][3],
    );
    assert_eq!(routine["routines"][0]["enabled"], false);
    let reloaded = request(
        &mut stdin,
        &mut stdout,
        "agent-reload",
        "iface:agentControl",
        &json!({"kind":"read"}),
    );
    assert_eq!(reloaded, routine);
    assert_eq!(reloaded["runs"], json!([]));

    let appearance = request(
        &mut stdin,
        &mut stdout,
        "appearance",
        "iface:getAppearance",
        &json!({"systemDark": false}),
    );
    assert_eq!(appearance["preference"], "system");
    assert_eq!(appearance["resolved"], "light");

    let default_directory = request(
        &mut stdin,
        &mut stdout,
        "directory",
        "iface:getDefaultDirectory",
        &json!({}),
    );
    assert_eq!(
        default_directory,
        Value::String(
            std::fs::canonicalize(home.join("Desktop/Projects"))
                .expect("canonical default directory")
                .to_string_lossy()
                .into_owned()
        )
    );

    let sessions = request(
        &mut stdin,
        &mut stdout,
        "sessions",
        "iface:listSessions",
        &json!({}),
    );
    assert_eq!(sessions, json!([]));

    let initial_orchestration = request(
        &mut stdin,
        &mut stdout,
        "orchestration-snapshot",
        "iface:getOrchestrationSnapshot",
        &json!({}),
    );
    assert_eq!(initial_orchestration["version"], 1);
    assert_eq!(initial_orchestration["agents"], json!([]));
    assert!(initial_orchestration.get("effects").is_none());
    // The real-child smoke deliberately stops at the read: creating an agent
    // would cross a provider boundary. Provider effects are covered by
    // deterministic fake-runtime tests in the host library.

    let worktrees = request(
        &mut stdin,
        &mut stdout,
        "worktrees",
        "iface:listManagedWorktrees",
        &json!({}),
    );
    assert_eq!(worktrees, json!([]));

    let updater_configuration = request(
        &mut stdin,
        &mut stdout,
        "updater-configure",
        "iface:nativeUpdaterConfigure",
        &json!({
            "isPackaged": false,
            "currentVersion": "0.2.0",
            "appBundlePath": null
        }),
    );
    assert_eq!(updater_configuration["enabled"], false);
    assert_eq!(updater_configuration["feedUrl"], Value::Null);
    let updater_state = request(
        &mut stdin,
        &mut stdout,
        "updater-state",
        "iface:getUpdateState",
        &json!({}),
    );
    assert_eq!(updater_state["phase"], "disabled");
    assert_eq!(updater_state["currentVersion"], "0.2.0");

    let canonical_project_root = std::fs::canonicalize(home.join("Desktop/Projects"))
        .expect("canonical prompt-context root");
    let prompt_context = request(
        &mut stdin,
        &mut stdout,
        "prompt-context",
        "iface:validatePromptContext",
        &json!({
            "target": {"root": canonical_project_root},
            "items": []
        }),
    );
    assert_eq!(prompt_context, json!({"ok":true,"items":[],"invalid":[]}));

    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../electron/src/fixtures/session-kind-contract.json"
    ))
    .expect("session-kind contract fixture");
    let chat = request_frame(
        &mut stdin,
        &mut stdout,
        "codex-chat",
        "iface:createSession",
        &json!({
            "engine": "codex",
            "options": {
                "cwd": default_directory,
                "prompt": "hi",
                "kind": "chat"
            }
        }),
    );
    assert_eq!(chat["ok"], false);
    assert_eq!(chat["error"]["message"], fixture["codexChatRefusal"]);
    assert_eq!(
        request(
            &mut stdin,
            &mut stdout,
            "sessions-after-chat",
            "iface:listSessions",
            &json!({}),
        ),
        json!([])
    );

    let status = close_and_wait(&mut child, stdin, stdout, Duration::from_secs(3))
        .expect("interface-host child did not exit after stdin closed");
    assert!(status.success(), "child exited with {status}");
    // The host hands its product version to the Codex handshake at runtime.
    assert_eq!(
        std::fs::read_to_string(temporary.path().join("fake-codex.client-version"))
            .expect("the fake Codex app-server was initialized"),
        interface_common::product_version!()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn voice_toggle_drives_the_helper_and_pushes_state() {
    let temporary = tempfile::tempdir().expect("temporary child root");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory fixture");
    private(&home);
    let claude = temporary.path().join("fake-claude");
    std::fs::write(
        &claude,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":2}}'
      ;;
    *'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .expect("fake Claude executable");
    private(&claude);
    let helper = temporary.path().join("fake-interface-voice");
    std::fs::write(
        &helper,
        r#"#!/usr/bin/env python3
import json, sys, threading, time
def emit(message):
    print(json.dumps(message), flush=True)
emit({"type": "ready", "protocol": 1, "voices": [], "defaultVoice": "fake"})
emit({"type": "state", "listening": True, "speaking": False})
def later():
    time.sleep(0.2)
    emit({"type": "final", "text": "hello"})
threading.Thread(target=later, daemon=True).start()
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("type") == "shutdown":
        break
"#,
    )
    .expect("fake voice helper");
    private(&helper);

    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CLAUDE_CODE_EXECUTABLE", &claude)
        .env("INTERFACE_VOICE_HELPER", &helper)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GROK_HOME", home.join(".grok"))
        .env("SHELL", NO_LOGIN_SHELL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn real interface-host child");
    let log = HostLog::drain(&mut child);
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));
    let ready = read_frame(&mut stdout);
    assert_eq!(ready["kind"], "ready");

    let cwd = std::fs::canonicalize(home.join("Desktop/Projects"))
        .expect("canonical project root")
        .to_string_lossy()
        .into_owned();
    let mut voice_pushes = Vec::new();
    let enabled = request_collecting(
        &mut stdin,
        &mut stdout,
        "voice-on",
        "iface:voiceSetEnabled",
        &json!({"enabled": true, "cwd": cwd}),
        &mut voice_pushes,
    );
    assert_eq!(enabled["ok"], true, "enable failed: {enabled}");
    let session_key = enabled["result"]["session_key"]
        .as_str()
        .unwrap_or_else(|| panic!("voice session key missing: {enabled}"))
        .to_owned();
    assert!(session_key.starts_with("claude#"), "{session_key}");
    while !voice_pushes.iter().any(|push| push["state"] == "listening") {
        assert!(
            voice_pushes.len() < 64,
            "no listening push: {voice_pushes:?}"
        );
        collect_voice_push(&mut stdout, &mut voice_pushes);
    }
    assert_eq!(voice_pushes[0]["state"], "starting");
    assert_eq!(voice_pushes[0]["cwd"], cwd);
    let listening = voice_pushes
        .iter()
        .find(|push| push["state"] == "listening")
        .expect("listening push");
    assert_eq!(listening["session_key"], session_key);

    let disabled = request_collecting(
        &mut stdin,
        &mut stdout,
        "voice-off",
        "iface:voiceSetEnabled",
        &json!({"enabled": false, "cwd": cwd}),
        &mut voice_pushes,
    );
    assert_eq!(disabled["ok"], true, "disable failed: {disabled}");
    assert_eq!(disabled["result"]["state"], "off");
    assert_eq!(
        voice_pushes.last().map(|push| &push["state"]),
        Some(&json!("off")),
        "{voice_pushes:?}"
    );
    assert!(
        !voice_pushes.iter().any(|push| push["state"] == "error"),
        "{voice_pushes:?}"
    );

    let Some(status) = close_and_wait(&mut child, stdin, stdout, Duration::from_secs(5)) else {
        panic!(
            "interface-host child did not exit after stdin closed:\n{}",
            log.collected()
        );
    };
    assert!(
        status.success(),
        "child exited with {status}:\n{}",
        log.collected()
    );
}

/// The child's stderr (its tracing log), read as it is written so a full pipe
/// never stalls the host, for a failure to show which shutdown stage failed.
struct HostLog(std::thread::JoinHandle<String>);

impl HostLog {
    fn drain(child: &mut std::process::Child) -> Self {
        let mut stderr = child.stderr.take().expect("child stderr");
        Self(std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            text
        }))
    }

    /// Everything logged, once the pipe closes; a provider child still
    /// holding it open gets a second, then the log is reported missing.
    fn collected(self) -> String {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.0.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.0.is_finished() {
            self.0.join().unwrap_or_default()
        } else {
            "(the host log was still open)".into()
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn voice_recovers_from_an_error_when_enabled_again() {
    let temporary = tempfile::tempdir().expect("temporary child root");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory fixture");
    private(&home);
    let claude = temporary.path().join("fake-claude");
    std::fs::write(
        &claude,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*|*'"type":"control_request"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .expect("fake Claude executable");
    private(&claude);
    // Denies the microphone on its first run and grants it on the next, so
    // the second enable can only reach `listening` through a fresh helper.
    let helper = temporary.path().join("fake-interface-voice");
    std::fs::write(
        &helper,
        r#"#!/usr/bin/env python3
import json, os, sys
marker = sys.argv[0] + ".denied"
def emit(message):
    print(json.dumps(message), flush=True)
emit({"type": "ready", "protocol": 1, "voices": [], "defaultVoice": "fake"})
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("type") == "start":
        if os.path.exists(marker):
            emit({"type": "permission", "microphone": "granted"})
            emit({"type": "state", "listening": True, "speaking": False})
        else:
            open(marker, "w").close()
            emit({"type": "permission", "microphone": "denied"})
    elif message.get("type") == "shutdown":
        break
"#,
    )
    .expect("fake voice helper");
    private(&helper);

    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CLAUDE_CODE_EXECUTABLE", &claude)
        .env("INTERFACE_VOICE_HELPER", &helper)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GROK_HOME", home.join(".grok"))
        .env("SHELL", NO_LOGIN_SHELL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn real interface-host child");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));
    assert_eq!(read_frame(&mut stdout)["kind"], "ready");

    let cwd = std::fs::canonicalize(home.join("Desktop/Projects"))
        .expect("canonical project root")
        .to_string_lossy()
        .into_owned();
    let mut voice_pushes = Vec::new();
    let enabled = request_collecting(
        &mut stdin,
        &mut stdout,
        "voice-on",
        "iface:voiceSetEnabled",
        &json!({"enabled": true, "cwd": cwd}),
        &mut voice_pushes,
    );
    assert_eq!(enabled["ok"], true, "enable failed: {enabled}");
    while !voice_pushes.iter().any(|push| push["state"] == "error") {
        assert!(voice_pushes.len() < 64, "no error push: {voice_pushes:?}");
        collect_voice_push(&mut stdout, &mut voice_pushes);
    }
    let denied = voice_pushes
        .iter()
        .find(|push| push["state"] == "error")
        .expect("error push");
    assert_eq!(denied["microphone"], "denied", "{denied}");
    let failed = voice_pushes.len();

    let again = request_collecting(
        &mut stdin,
        &mut stdout,
        "voice-on-again",
        "iface:voiceSetEnabled",
        &json!({"enabled": true, "cwd": cwd}),
        &mut voice_pushes,
    );
    assert_eq!(again["ok"], true, "re-enable failed: {again}");
    assert_ne!(again["result"]["state"], "error", "{again}");
    while !voice_pushes[failed..]
        .iter()
        .any(|push| push["state"] == "listening")
    {
        assert!(
            voice_pushes.len() < 64,
            "no listening push after the error: {voice_pushes:?}"
        );
        collect_voice_push(&mut stdout, &mut voice_pushes);
    }
    let recovered: Vec<_> = voice_pushes[failed..]
        .iter()
        .map(|push| push["state"].clone())
        .collect();
    assert_eq!(recovered.first(), Some(&json!("off")), "{recovered:?}");
    assert_eq!(recovered.get(1), Some(&json!("starting")), "{recovered:?}");
    assert!(!recovered.contains(&json!("error")), "{recovered:?}");

    let disabled = request_collecting(
        &mut stdin,
        &mut stdout,
        "voice-off",
        "iface:voiceSetEnabled",
        &json!({"enabled": false, "cwd": cwd}),
        &mut voice_pushes,
    );
    assert_eq!(disabled["result"]["state"], "off", "{disabled}");

    let status = close_and_wait(&mut child, stdin, stdout, Duration::from_secs(5))
        .expect("interface-host child did not exit after stdin closed");
    assert!(status.success(), "child exited with {status}");
}

/// Ends the host the way the app does: closes its stdin and keeps reading its
/// stdout until it exits. Closing stdout too would fail any frame the host
/// still writes while it settles with a broken pipe, and the host exits 1.
/// `None` means it was still running after `limit` and has been killed.
fn close_and_wait(
    child: &mut std::process::Child,
    stdin: ChildStdin,
    mut stdout: BufReader<ChildStdout>,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    drop(stdin);
    std::thread::spawn(move || std::io::copy(&mut stdout, &mut std::io::sink()));
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("child status") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled child");
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn private(path: &std::path::Path) {
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).expect("private mode");
}

/// Like `request_frame`, but keeps every `iface:voice` push seen on the way.
fn request_collecting(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: &str,
    operation: &str,
    payload: &Value,
    voice_pushes: &mut Vec<Value>,
) -> Value {
    writeln!(
        stdin,
        "{}",
        json!({
            "version": interface_host::PROTOCOL_VERSION,
            "id": id,
            "operation": operation,
            "payload": payload,
        })
    )
    .expect("write child request");
    stdin.flush().expect("flush child request");
    loop {
        let frame = read_frame(stdout);
        if frame["kind"] == "push" && frame["operation"] == "iface:voice" {
            voice_pushes.push(frame["payload"].clone());
        }
        if frame["kind"] == "response" && frame["id"] == id {
            return frame;
        }
    }
}

fn collect_voice_push(stdout: &mut BufReader<ChildStdout>, voice_pushes: &mut Vec<Value>) {
    loop {
        let frame = read_frame(stdout);
        if frame["kind"] == "push" && frame["operation"] == "iface:voice" {
            voice_pushes.push(frame["payload"].clone());
            return;
        }
    }
}

fn request(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: &str,
    operation: &str,
    payload: &Value,
) -> Value {
    let frame = request_frame(stdin, stdout, id, operation, payload);
    assert_eq!(frame["ok"], true, "failed response: {frame}");
    frame["result"].clone()
}

fn request_frame(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: &str,
    operation: &str,
    payload: &Value,
) -> Value {
    writeln!(
        stdin,
        "{}",
        json!({
            "version": interface_host::PROTOCOL_VERSION,
            "id": id,
            "operation": operation,
            "payload": payload,
        })
    )
    .expect("write child request");
    stdin.flush().expect("flush child request");
    loop {
        let frame = read_frame(stdout);
        if frame["kind"] != "response" || frame["id"] != id {
            continue;
        }
        return frame;
    }
}

fn read_frame(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    let bytes = stdout.read_line(&mut line).expect("read child frame");
    assert_ne!(bytes, 0, "interface-host stdout ended before a frame");
    serde_json::from_str(&line).expect("valid child NDJSON frame")
}

#[test]
fn retired_control_channel_answers_unavailable_and_starts_nothing() {
    // The Control assistant was retired on 2026-10-14. `iface:voiceControl`
    // stays wire protocol: a well-formed request is refused as definitely
    // unsent, a malformed one is still an invalid payload, and the host never
    // opens the Control store or a coordinator session.
    let temporary = tempfile::Builder::new()
        .prefix("ivc-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).unwrap();
    private(&home);
    let cwd = std::fs::canonicalize(home.join("Desktop/Projects"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let claude = temporary.path().join("fake-control-claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\n[ \"$1\" = --version ] && echo '2.1.0 fixture' && exit 0\nexit 1\n",
    )
    .unwrap();
    private(&claude);
    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CLAUDE_CODE_EXECUTABLE", &claude)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GROK_HOME", home.join(".grok"))
        .env("SHELL", NO_LOGIN_SHELL)
        .env(
            "INTERFACE_VOICE_HELPER",
            "/nonexistent/audio-must-not-start",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    assert_eq!(read_frame(&mut stdout)["kind"], "ready");
    let control = |command: Value| json!({"version":1,"command":command});
    for (id, command) in [
        (
            "begin",
            json!({"kind":"begin","runId":"run-fixture","cwd":cwd,"durationSeconds":60,"maxActions":16}),
        ),
        ("inspect", json!({"kind":"inspect"})),
        ("stop", json!({"kind":"stop","runId":"run-fixture"})),
    ] {
        let refused = request_frame(
            &mut stdin,
            &mut stdout,
            id,
            "iface:voiceControl",
            &control(command),
        );
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused["error"]["code"], "voice-control-unavailable");
        assert_eq!(refused["error"]["replaySafe"], true);
        assert_eq!(refused["error"]["retryable"], false);
    }
    let malformed = request_frame(
        &mut stdin,
        &mut stdout,
        "malformed",
        "iface:voiceControl",
        &control(json!({"kind":"inspect","bypassPermissions":true})),
    );
    assert_eq!(malformed["error"]["code"], "invalid-payload", "{malformed}");
    let status = close_and_wait(&mut child, stdin, stdout, Duration::from_secs(10))
        .expect("host failed to shut down");
    assert!(status.success(), "child exited with {status}");
    assert!(!root.join("voice/control").exists());
}
