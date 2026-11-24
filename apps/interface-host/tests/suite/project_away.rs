//! Real host + native Codex protocol fixture. No provider account or source data.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Host {
    child: Child,
    input: std::process::ChildStdin,
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
    fn request(&mut self, id: &str, command: Value) -> Value {
        let payload =
            json!({"version":if command["kind"] == "status" {2} else {1},"command":command});
        self.call(id, "iface:projectAway", payload)
    }
    #[allow(clippy::needless_pass_by_value)]
    fn call(&mut self, id: &str, operation: &str, payload: Value) -> Value {
        writeln!(self.input, "{}", json!({"version":interface_host::PROTOCOL_VERSION,"id":id,"operation":operation,"payload":payload})).unwrap();
        self.input.flush().unwrap();
        loop {
            let frame = self
                .frames
                .recv_timeout(Duration::from_secs(15))
                .expect("bounded host response");
            if frame["id"] == id {
                assert_eq!(frame["ok"], true, "{frame}");
                return frame["result"].clone();
            }
        }
    }
}
#[test]
#[allow(clippy::too_many_lines)]
fn project_away_real_host_native_turn_review_and_retained_results() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = home.join("Desktop/Projects");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(project.join("fixture.txt"), "before").unwrap();
    // A loop works in its own git worktree of the project (review B3).
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "fixture.txt"],
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
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&project)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    let codex = temp.path().join("fixture-codex");
    std::fs::write(&codex, r"#!/usr/bin/env python3
import json, os, sys
if '--version' in sys.argv:
    print('codex-cli 0.0.0-test'); raise SystemExit(0)
thread = 'away-fixture-' + str(os.getpid())
cwd = None
def out(value): print(json.dumps(value), flush=True)
for line in sys.stdin:
    msg = json.loads(line); method = msg.get('method'); params = msg.get('params', {})
    with open(os.path.join(os.path.dirname(sys.argv[0]), 'protocol.log'), 'a') as log: log.write(json.dumps(msg) + '\n')
    result = {}
    if method in ['thread/list', 'model/list', 'skills/list']: result = {'data': []}
    elif method == 'account/read': result = {'account': None}
    elif method == 'thread/start':
        cwd = params['cwd']
        sandbox = params.get('sandbox', 'workspace-write')
        result = {'thread': {'id': thread}, 'sandbox': {'type': 'readOnly' if sandbox == 'read-only' else 'workspaceWrite'}}
    elif method == 'turn/start': result = {'turn': {'id': 'turn-1'}}
    if 'id' in msg: out({'id': msg['id'], 'result': result})
    if method == 'turn/start':
        prompt = '\n'.join(part.get('text','') for part in params['input'])
        review = 'You are a fresh independent reviewer.' in prompt
        assert 'Owner objective:' in prompt
        record = json.load(open(os.path.join(os.path.dirname(sys.argv[0]), 'data', 'away', 'project-away.v3.json')))
        assert record['runs'][0]['turns'][-1]['sessionKey'] == 'codex#' + thread
        with open(os.path.join(cwd, 'native-turns.log'), 'a') as f: f.write(('review' if review else 'work') + '\n')
        if not review:
            with open(os.path.join(cwd, 'fixture.txt'), 'w') as f: f.write('after')
        assert open(os.path.join(cwd, 'fixture.txt')).read() == 'after'
        report = 'AWAY_RESULT ' + json.dumps({'outcome': 'complete' if review else 'continue', 'summary': 'Verified the fixture edit', 'checks': ['fixture.txt equals after: passed']})
        out({'method': 'item/agentMessage/delta', 'params': {'threadId': thread, 'delta': report}})
        out({'method': 'turn/completed', 'params': {'threadId': thread, 'turn': {'id': 'turn-1', 'status': 'completed'}}})
        out({'method': 'thread/status/changed', 'params': {'threadId': thread, 'status': {'type': 'idle'}}})
").unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", temp.path().join("data"))
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CODEX_EXECUTABLE", &codex)
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
            if let Ok(value) = serde_json::from_str(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut host = Host {
        child,
        input,
        frames,
    };
    let ready = host.frames.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_eq!(ready["version"], 72);
    let start = json!({"kind":"start","runId":"native-loop","cwd":project,"options":null});
    let started = host.request("start", start.clone());
    assert_eq!(started["runs"][0]["state"], "running", "{started}");
    let deadline = Instant::now() + Duration::from_secs(20);
    let finished = loop {
        let state = host.request("inspect", json!({"kind":"inspect"}));
        if state["runs"][0]["state"] != "running" {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "Away did not finish: {state}; protocol={}",
            std::fs::read_to_string(temp.path().join("protocol.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(finished["runs"][0]["state"], "completed", "{finished}");
    let status = host.request("status", json!({"kind":"status"}));
    assert_eq!(status["version"], 2);
    assert_eq!(status["revision"], finished["revision"]);
    assert_eq!(status["runIds"], json!(["native-loop"]));
    assert_eq!(status["activeRuns"], json!([]));
    assert!(status.get("runs").is_none());
    let turns = finished["runs"][0]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_ne!(turns[0]["sessionKey"], turns[1]["sessionKey"]);
    assert!(turns.iter().all(|turn| turn["state"] == "completed"));
    // The work session is the loop's thread, its round the thread's turn 0;
    // both rounds ran in the loop's own worktree, never the project's checkout.
    let thread = &finished["runs"][0]["thread"];
    assert_eq!(thread["key"], turns[0]["sessionKey"], "{finished}");
    assert_eq!(turns[0]["threadTurn"], 0, "{finished}");
    assert!(turns[1].get("threadTurn").is_none());
    let worktree = std::path::PathBuf::from(thread["worktree"].as_str().unwrap());
    assert_ne!(worktree, project);
    assert!(
        thread["branch"]
            .as_str()
            .unwrap()
            .starts_with("iface/away-")
    );
    assert_eq!(
        std::fs::read_to_string(project.join("fixture.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("fixture.txt")).unwrap(),
        "after"
    );
    host.request("same-start", start);
    assert_eq!(
        std::fs::read_to_string(worktree.join("native-turns.log")).unwrap(),
        "work\nreview\n"
    );
    // Threads lists the loop as a thread in that worktree, with its rounds.
    let deadline = Instant::now() + Duration::from_secs(10);
    let view = loop {
        let view =
            host.call("threads", "iface:threadControl", json!({"kind":"read"}))["view"].clone();
        let away = view["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["key"] == thread["key"])
            .cloned();
        if away
            .as_ref()
            .is_some_and(|away| away["away"]["state"] == "completed")
        {
            break away.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "the thread never showed its loop: {view}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        view["workspace"]["worktreePath"], thread["worktree"],
        "{view}"
    );
    assert_eq!(view["turns"][0]["ordinal"], 0, "{view}");
    let rounds = view["away"]["rounds"].as_array().unwrap();
    assert_eq!(rounds[0]["turn"], 0, "{view}");
    assert_eq!(rounds[1]["verdict"], "complete", "{view}");
    assert_eq!(rounds[1]["sessionKey"], turns[1]["sessionKey"], "{view}");
    host.request(
        "stop-before-start",
        json!({"kind":"stop","runId":"cancel-first"}),
    );
    let stopped = host.request(
        "cancelled-start",
        json!({"kind":"start","runId":"cancel-first","cwd":project,"options":null}),
    );
    assert_eq!(stopped["runs"][1]["state"], "stopped");
}
