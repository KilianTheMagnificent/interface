use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use tempfile::{TempDir, tempdir};

use super::*;

/// A fake `claude` that records its argv (one per line) and stdin, then runs
/// `body`.
fn fake_claude(body: &str) -> (TempDir, PathBuf) {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("claude");
    let args = directory.path().join("args.txt");
    let stdin = directory.path().join("stdin.txt");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\n{body}\n",
            args.display(),
            stdin.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    (directory, executable)
}

fn recorded(directory: &Path, name: &str) -> String {
    fs::read_to_string(directory.join(name)).unwrap_or_default()
}

#[tokio::test]
async fn a_title_is_one_tool_less_print_call_that_persists_no_session() {
    let (directory, executable) = fake_claude(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Title: \"Login test fixed.\"","session_id":"x"}'"#,
    );
    let answer = "a".repeat(5_000);
    let title = generate_title(
        &executable,
        directory.path(),
        "  Fix the flaky login test  ",
        &answer,
    )
    .await
    .unwrap();
    assert_eq!(title, "Login test fixed");

    let args = recorded(directory.path(), "args.txt");
    let args = args.lines().collect::<Vec<_>>();
    assert_eq!(args, title_arguments());
    for required in [
        "-p",
        "--no-session-persistence",
        "--safe-mode",
        "--strict-mcp-config",
    ] {
        assert!(args.contains(&required), "{required} missing from {args:?}");
    }
    let flag = |name: &str| args[args.iter().position(|arg| *arg == name).unwrap() + 1];
    assert_eq!(flag("--tools"), "", "every built-in tool is off");
    // Claude Haiku 5.5 by its exact id, not the CLI's moving alias.
    assert_eq!(flag("--model"), "claude-haiku-5-5");
    assert_eq!(TITLE_MODEL, "claude-haiku-5-5");
    assert_eq!(flag("--output-format"), "json");
    assert!(args.contains(&"--setting-sources="), "no settings sources");

    let stdin = recorded(directory.path(), "stdin.txt");
    assert!(stdin.starts_with(INSTRUCTION));
    assert!(stdin.contains("<first-prompt>\nFix the flaky login test\n</first-prompt>"));
    // Each excerpt is bounded.
    assert!(stdin.contains(&format!(
        "<first-answer>\n{}\n</first-answer>",
        "a".repeat(2_048)
    )));
    assert!(!stdin.contains(&"a".repeat(2_049)));
}

#[tokio::test]
async fn a_failed_empty_or_late_title_is_an_error_and_changes_nothing() {
    let cases = [
        (
            r#"printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}'"#,
            "did not complete",
        ),
        (
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"  \"\" "}'"#,
            "without a usable title",
        ),
        ("printf 'Not logged in\\n'; exit 1", "exited with an error"),
        ("printf 'not json\\n'", "did not answer with a result"),
    ];
    for (body, reason) in cases {
        let (directory, executable) = fake_claude(body);
        let error = generate_title(&executable, directory.path(), "hello", "")
            .await
            .unwrap_err();
        assert!(error.contains(reason), "{body}: {error}");
        assert!(
            !recorded(directory.path(), "stdin.txt").contains("<first-answer>"),
            "an empty answer is left out"
        );
    }

    let (directory, executable) = fake_claude("sleep 20");
    let started = std::time::Instant::now();
    let error = generate_title_within(
        &executable,
        directory.path(),
        "hello",
        "",
        &[],
        Duration::from_millis(300),
    )
    .await
    .unwrap_err();
    assert!(error.contains("did not answer within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));

    let (directory, _) = fake_claude("exit 0");
    assert!(
        generate_title(Path::new("claude"), directory.path(), "hello", "")
            .await
            .unwrap_err()
            .contains("absolute")
    );
    assert!(
        generate_title(&directory.path().join("claude"), directory.path(), "  ", "")
            .await
            .unwrap_err()
            .contains("no prompt")
    );
}

#[tokio::test]
async fn a_title_is_named_from_the_first_prompt_alone() {
    let (directory, executable) = fake_claude(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Release checklist for atlas"}'"#,
    );
    let title = generate_title(
        &executable,
        directory.path(),
        "Write the release checklist for atlas",
        "",
    )
    .await
    .unwrap();
    assert_eq!(title, "Release checklist for atlas");
    let stdin = recorded(directory.path(), "stdin.txt");
    // The instruction asks about the first message, not a conversation
    // that has not happened yet, and no empty answer is sent.
    assert!(stdin.starts_with(INSTRUCTION));
    assert!(INSTRUCTION.contains("first message below"));
    assert!(!INSTRUCTION.contains("conversation below"));
    assert!(
        stdin.ends_with("<first-prompt>\nWrite the release checklist for atlas\n</first-prompt>\n")
    );
    assert!(!stdin.contains("<first-answer>"));
}

#[test]
fn titles_are_cleaned_to_one_short_line() {
    for (answer, title) in [
        ("Login test fixed", Some("Login test fixed")),
        (
            "\n\n**Login test fixed.**\nBecause…",
            Some("Login test fixed"),
        ),
        ("title: `Ref rename`", Some("Ref rename")),
        ("“Quoted title”", Some("Quoted title")),
        ("  spaced    out   words ", Some("spaced out words")),
        ("\"\"", None),
        ("", None),
    ] {
        assert_eq!(clean_title(answer).as_deref(), title, "{answer:?}");
    }
    let long = "word ".repeat(30);
    let cut = clean_title(&long).unwrap();
    assert!(cut.chars().count() <= 60 && cut.ends_with("word"), "{cut}");
}
