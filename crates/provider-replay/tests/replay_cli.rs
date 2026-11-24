//! The two binaries as an adapter meets them: spawned with an argv, fed
//! stdin, read on stdout. Building this test is also what makes cargo build
//! `provider-replay` and `provider-record` for the host's replay suite.
#![cfg(unix)]

use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use provider_replay::{
    Entry, Identity, Meta, RECORD_OUT_ENV, RECORD_TARGET_ENV, TRANSCRIPT_ENV, Transcript,
};
use serde_json::{Value, json};

const REPLAY: &str = env!("CARGO_BIN_EXE_provider-replay");
const RECORD: &str = env!("CARGO_BIN_EXE_provider-record");

fn write_transcript(directory: &Path) -> PathBuf {
    let transcript = json!({
        "version": 1, "engine": "codex", "protocol": "app-server",
        "cliVersion": "codex-cli 0.0.0", "scenario": "cli", "recordedAt": "2026-11-14",
        "probes": {"--version": {"stdout": "codex-cli 0.0.0\n", "code": 0}},
        "entries": [
            {"dir": "launch", "argv": ["app-server"], "cwd": "/tmp/recorded"},
            {"dir": "out", "frame": {"id": 0, "method": "initialize", "params": {}}},
            {"dir": "in", "frame": {"id": 0, "result": {"cwd": "/tmp/recorded"}}},
            {"dir": "out", "frame": {"id": 1, "method": "thread/start", "params": {}}},
            {"dir": "in", "frame": {"id": 1, "result": {"thread": {"id": "t"}}}}
        ]
    });
    let path = directory.join("cli.json");
    std::fs::write(&path, transcript.to_string()).unwrap();
    path
}

#[test]
fn the_replayer_answers_probes_serves_frames_and_fails_loudly() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(directory.path()).unwrap();
    let transcript = write_transcript(&cwd);

    let probe = Command::new(REPLAY)
        .arg("--version")
        .env(TRANSCRIPT_ENV, &transcript)
        .output()
        .unwrap();
    assert!(probe.status.success());
    assert_eq!(String::from_utf8_lossy(&probe.stdout), "codex-cli 0.0.0\n");

    let mut child = Command::new(REPLAY)
        .arg("app-server")
        .current_dir(&cwd)
        .env(TRANSCRIPT_ENV, &transcript)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        stdin,
        "{}",
        json!({"id": 4, "method": "initialize", "params": {}})
    )
    .unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        answer,
        json!({"id": 4, "result": {"cwd": cwd.to_string_lossy()}}),
        "the host's id and the working directory are put in"
    );
    writeln!(
        stdin,
        "{}",
        json!({"id": 5, "method": "turn/start", "params": {}})
    )
    .unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected frame from Interface")
            && stderr.contains("expected: request thread/start")
            && stderr.contains("received: request turn/start"),
        "{stderr}"
    );

    let missing = Command::new(REPLAY).arg("app-server").output().unwrap();
    assert_eq!(missing.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&missing.stderr).contains(TRANSCRIPT_ENV));
}

#[test]
fn the_recorder_passes_frames_through_and_writes_a_scrubbed_transcript() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    // A stand-in CLI: answers `--version`, echoes each frame back with the
    // owner's identity in it.
    let target = root.join("codex");
    std::fs::write(
        &target,
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo 'codex-cli 9.9.9'; exit 0; fi\n\
         while IFS= read -r line; do\n\
           printf '{\"echo\":%s,\"home\":\"%s/x\",\"email\":\"a.person@gmail.com\"}\\n' \"$line\" \"$HOME\"\n\
         done\n",
    )
    .unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = root.join("simple.json");

    let version = Command::new(RECORD)
        .arg("--version")
        .env(RECORD_TARGET_ENV, &target)
        .env(RECORD_OUT_ENV, &out)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        "codex-cli 9.9.9\n"
    );

    let mut child = Command::new(RECORD)
        .arg("app-server")
        .current_dir(&root)
        .env(RECORD_TARGET_ENV, &target)
        .env(RECORD_OUT_ENV, &out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let session = "6f1c0e8e-2f7a-4f8b-9c55-0e1d2c3b4a59";
    writeln!(
        stdin,
        "{}",
        json!({"id": 1, "method": "thread/start", "params": {"session": session}})
    )
    .unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let home = std::env::var("HOME").unwrap();
    assert!(
        line.contains(&home) && line.contains(session),
        "passed through untouched: {line}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());

    let meta = Meta {
        engine: "codex".into(),
        protocol: "app-server".into(),
        cli_version: "codex-cli 9.9.9".into(),
        scenario: "simple".into(),
        recorded_at: "2026-11-14".into(),
    };
    let transcript = provider_replay::finish(&out, &meta, &Identity::current()).unwrap();
    assert_eq!(Transcript::read(&out).unwrap(), transcript);
    assert_eq!(transcript.probes["--version"].stdout, "codex-cli 9.9.9\n");
    let text = std::fs::read_to_string(&out).unwrap();
    let user = Identity::current().user;
    assert!(!text.contains(&home) && !text.contains(session) && !text.contains("gmail"));
    assert!(user.len() < 3 || !text.contains(&user), "{text}");
    let frames: Vec<&Entry> = transcript.entries.iter().collect();
    assert!(matches!(frames[0], Entry::Launch { argv, .. } if argv == &["app-server"]));
    assert!(
        matches!(frames[1], Entry::Out { frame } if frame["params"]["session"] == "00000000-0000-4000-8000-000000000001")
    );
    assert!(
        matches!(frames[2], Entry::In { frame } if frame["echo"]["params"]["session"] == "00000000-0000-4000-8000-000000000001"
            && frame["home"] == "/Users/owner/x" && frame["email"] == "owner@example.com")
    );
    assert!(matches!(frames[3], Entry::Exit { code: 0 }));
}
