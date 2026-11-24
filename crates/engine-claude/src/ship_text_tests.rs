use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use tempfile::{TempDir, tempdir};

use super::*;
use crate::title::title_arguments;

/// A fake `claude` that records its argv (one per line) and stdin, then runs
/// `body`.
fn fake_claude(body: &str) -> (TempDir, PathBuf) {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("claude");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\n{body}\n",
            directory.path().join("args.txt").display(),
            directory.path().join("stdin.txt").display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    (directory, executable)
}

#[tokio::test]
async fn write_with_ai_is_one_tool_less_print_call_like_a_title() {
    let (directory, executable) = fake_claude(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"```text\nFix the login test\n\nIt waited on the wrong element.\n```"}'"#,
    );
    let text = write_ship_text(
        &executable,
        directory.path(),
        ShipTextKind::Commit,
        "<changes>\n M src/login.rs | +2 -1\n</changes>",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        text,
        "Fix the login test\n\nIt waited on the wrong element."
    );

    let args = fs::read_to_string(directory.path().join("args.txt")).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert_eq!(args, ship_text_arguments());
    // The same call as a title, but for its own system prompt.
    let title = title_arguments();
    assert_eq!(args[..args.len() - 1], title[..title.len() - 1]);
    let flag = |name: &str| args[args.iter().position(|arg| *arg == name).unwrap() + 1];
    assert_eq!(flag("--tools"), "", "every built-in tool is off");
    // Write with AI shares the titles' pinned model.
    assert_eq!(flag("--model"), "claude-haiku-5-5");
    assert!(args.contains(&"--no-session-persistence"));
    assert!(args.contains(&"--strict-mcp-config"));
    assert!(args.contains(&"--setting-sources="));
    let stdin = fs::read_to_string(directory.path().join("stdin.txt")).unwrap();
    assert!(stdin.starts_with(COMMIT_INSTRUCTION), "{stdin}");
    assert!(stdin.ends_with("<changes>\n M src/login.rs | +2 -1\n</changes>"));

    let (directory, executable) = fake_claude(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Fix login\n\n- waits for the form"}'"#,
    );
    write_ship_text(
        &executable,
        directory.path(),
        ShipTextKind::PullRequest,
        "changes",
        &[],
    )
    .await
    .unwrap();
    let stdin = fs::read_to_string(directory.path().join("stdin.txt")).unwrap();
    assert!(stdin.starts_with(PULL_REQUEST_INSTRUCTION), "{stdin}");
}

#[tokio::test]
async fn a_failed_empty_or_late_answer_is_an_error() {
    for (body, reason) in [
        (
            r#"printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}'"#,
            "did not complete",
        ),
        (
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"  "}'"#,
            "without usable text",
        ),
        ("printf 'Not logged in\\n'; exit 1", "exited with an error"),
    ] {
        let (directory, executable) = fake_claude(body);
        let error = write_ship_text(
            &executable,
            directory.path(),
            ShipTextKind::Commit,
            "x",
            &[],
        )
        .await
        .unwrap_err();
        assert!(error.contains(reason), "{body}: {error}");
    }
    let (directory, executable) = fake_claude("sleep 20");
    let started = std::time::Instant::now();
    let error = write_ship_text_within(
        &executable,
        directory.path(),
        ShipTextKind::Commit,
        "x",
        &[],
        Duration::from_millis(300),
    )
    .await
    .unwrap_err();
    assert!(error.contains("did not answer within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // Nothing to write about: Claude is never started.
    let (directory, executable) = fake_claude("exit 0");
    let error = write_ship_text(
        &executable,
        directory.path(),
        ShipTextKind::Commit,
        "  ",
        &[],
    )
    .await
    .unwrap_err();
    assert!(error.contains("no changes"), "{error}");
    assert!(!directory.path().join("args.txt").exists());
}

#[test]
fn answers_are_cleaned_and_bounded() {
    assert_eq!(clean_ship_text("  Fix it \n").as_deref(), Some("Fix it"));
    assert_eq!(
        clean_ship_text("```\nFix it\n\nBody\n```").as_deref(),
        Some("Fix it\n\nBody")
    );
    assert_eq!(clean_ship_text("```\n```"), None);
    assert_eq!(clean_ship_text(""), None);
    let long = "é".repeat(SHIP_TEXT_MAX_BYTES);
    let cut = clean_ship_text(&long).unwrap();
    assert!(cut.len() <= SHIP_TEXT_MAX_BYTES && cut.chars().all(|c| c == 'é'));
}
