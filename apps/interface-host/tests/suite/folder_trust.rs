//! Folder trust end to end (protocol 72): the real host binary on an
//! isolated data root and home, the hands-free kit's fake Claude Code behind
//! a wrapper that logs each launch's arguments. A thread started in a folder
//! nobody trusted runs with the user's own settings only; once the owner
//! trusts the folder, the idle thread's process ends and its next message
//! resumes it with the settings it asked for. No account or network.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const LIMIT: Duration = Duration::from_secs(60);

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

    fn thread(&mut self, id: &str, key: &str) -> Value {
        let view = self.threads(id, json!({"kind": "read"}));
        view["view"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .cloned()
            .unwrap_or(Value::Null)
    }
}

fn git(project: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(project)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The structured Claude launches the wrapper logged, and how many of them
/// have ended.
fn launches(log: &Path) -> (Vec<String>, usize) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let structured = |line: &&str| line.contains("--input-format stream-json");
    let started = text
        .lines()
        .filter_map(|line| line.strip_prefix("start: "))
        .filter(structured)
        .map(ToOwned::to_owned)
        .collect();
    let ended = text
        .lines()
        .filter_map(|line| line.strip_prefix("exit: "))
        .filter(structured)
        .count();
    (started, ended)
}

fn wait_for(log: &Path, what: &str, done: impl Fn(&(Vec<String>, usize)) -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !done(&launches(log)) {
        assert!(Instant::now() < deadline, "{what}: {:?}", launches(log));
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn setting_sources(arguments: &str) -> Option<&str> {
    arguments
        .split(' ')
        .find_map(|argument| argument.strip_prefix("--setting-sources="))
}

#[test]
#[allow(clippy::too_many_lines)] // One untrusted thread's whole life.
fn an_untrusted_folders_thread_runs_on_user_settings_until_trusted_then_resumes_with_its_own() {
    let temporary = tempfile::Builder::new()
        .prefix("itrust-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temporary.path().join("data");
    let home = temporary.path().join("home");
    let project = home.join("Desktop/Projects/cloned");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        project.join(".claude/settings.json"),
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch pwned"}]}]}}"#,
    )
    .unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "."]);
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
    let fake = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../electron/scripts/handsfree-fake-provider.sh");
    assert!(fake.is_file(), "{} is missing", fake.display());
    let log = temporary.path().join("launches.log");
    let wrapper = temporary.path().join("claude");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf 'start: %s\\n' \"$*\" >> '{log}'\n'{fake}' \"$@\"\nstatus=$?\nprintf 'exit: %s\\n' \"$*\" >> '{log}'\nexit $status\n",
            log = log.display(),
            fake = std::fs::canonicalize(&fake).unwrap().display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_interface-host"))
        .env("INTERFACE_HOST_TEST_DATA_ROOT", &root)
        .env("INTERFACE_HOST_TEST_HOME", &home)
        .env("CLAUDE_CODE_EXECUTABLE", &wrapper)
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
    assert_eq!(ready["version"], 72);
    host.call(
        "default-directory",
        "iface:getDefaultDirectory",
        Value::Null,
    );

    // Nobody has answered for the folder yet; the question names the
    // repository's top folder, asked from a folder inside it.
    let asked = host.threads(
        "ask",
        json!({"kind": "folderTrust", "folder": project.join(".claude")}),
    );
    assert_eq!(
        asked["folderTrust"],
        json!({"folder": project, "state": "unasked"})
    );

    // A thread that asks for every settings source.
    let prepared = host.threads(
        "prepare",
        json!({"kind": "prepareWorkspace", "projectRoot": project, "mode": "local", "hint": "x"}),
    );
    let token = prepared["prepared"]["token"].as_str().unwrap().to_owned();
    let created = host.call(
        "create",
        "iface:createSession",
        json!({"engine": "claude", "options": {"cwd": project, "prompt": "look around",
            "kind": "code", "settingSources": ["user", "project", "local"]}}),
    );
    let key = created["key"].as_str().unwrap().to_owned();
    host.threads(
        "bind",
        json!({"kind": "bind", "key": key, "token": token, "prompt": "look around"}),
    );
    wait_for(&log, "the first launch", |(started, _)| started.len() == 1);
    let first = launches(&log).0.remove(0);
    assert_eq!(setting_sources(&first), Some("user"), "{first}");
    assert!(first.contains("--strict-mcp-config"), "{first}");
    assert!(!first.contains("--resume"), "{first}");
    assert_eq!(
        host.thread("untrusted", &key)["folderTrust"],
        json!({"folder": project, "asked": false})
    );

    // Trusted: the thread no longer says so, and once its turn is over its
    // process ends by itself.
    let trusted = host.threads(
        "trust",
        json!({"kind": "trustFolder", "folder": project, "trust": true}),
    );
    assert_eq!(trusted["folderTrust"]["state"], "trusted");
    assert!(host.thread("trusted", &key).get("folderTrust").is_none());
    wait_for(&log, "the restart", |(_, ended)| *ended == 1);
    assert_eq!(launches(&log).0.len(), 1, "nothing started it again yet");

    // The next message resumes it with the sources it asked for: the
    // restriction was never written into the registry.
    host.call(
        "send",
        "iface:send",
        json!({"sessionKey": key, "text": "and now?"}),
    );
    wait_for(&log, "the resume", |(started, _)| started.len() == 2);
    let resumed = launches(&log).0.remove(1);
    assert!(resumed.contains("--resume"), "{resumed}");
    assert_eq!(
        setting_sources(&resumed),
        Some("user,project,local"),
        "{resumed}"
    );
    // Recorded once, for the repository.
    let saved: Value =
        serde_json::from_slice(&std::fs::read(root.join("trusted-folders.v1.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved,
        json!({"version": 1, "trusted": [project], "declined": []})
    );
}
