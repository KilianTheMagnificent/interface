use super::*;
use crate::argv::{CliCommand, parse_argv};
use std::{os::unix::fs::PermissionsExt, path::Path};

fn store(root: &Path) -> Store {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    Store::create(root, root.join("project"), "codex/test".into(), tasks()).unwrap()
}

fn status_args(root: &Path) -> Vec<String> {
    vec![
        "status".into(),
        "--directory".into(),
        root.display().to_string(),
    ]
}

#[test]
fn cli_grammar_preserves_away_arguments_and_rejects_ambiguous_options() {
    let args = [
        "away",
        "demo",
        "--directory",
        "/private/tmp/new-pilot",
        "--model",
        "codex/test",
    ]
    .map(String::from);
    let parsed = parse_argv(&args).unwrap();
    assert_eq!(parsed.command, Some(CliCommand::Away));
    assert_eq!(
        options(&parsed.away_args).unwrap().model.as_deref(),
        Some("codex/test")
    );
    assert!(
        options(&[
            "demo".into(),
            "--directory".into(),
            "/private/tmp/test".into()
        ])
        .is_err()
    );
    assert!(
        options(&[
            "demo".into(),
            "--directory".into(),
            "relative".into(),
            "--model".into(),
            "codex/test".into()
        ])
        .is_err()
    );
    assert!(
        options(&[
            "status".into(),
            "--directory".into(),
            "/private/tmp/test".into(),
            "--model".into(),
            "codex/test".into()
        ])
        .is_err()
    );
}

#[test]
fn demo_never_overwrites_an_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("owner.txt");
    std::fs::write(&marker, "preserve").unwrap();
    assert!(initialize(dir.path()).is_err());
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "preserve");
}

#[tokio::test]
async fn status_is_fail_closed_for_durable_unknown_work() {
    let dir = tempfile::tempdir().unwrap();
    drop(store(dir.path()));
    let mut record = Store::read(dir.path()).unwrap();
    record.finished = true;
    record.tasks[0].status = Status::Unknown;
    std::fs::write(
        dir.path().join("away-run.v1.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let result = run(&status_args(dir.path()), Path::new("/"), None).await;
    assert_eq!(result.exit_code, CLI_EXIT.unknown);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("\"status\": \"unknown\""));
}

#[tokio::test]
async fn status_remains_successful_for_a_known_stopped_run() {
    let dir = tempfile::tempdir().unwrap();
    drop(store(dir.path()));
    let recovered = Store::recover(dir.path()).unwrap();
    assert!(recovered.stopped);
    let result = run(&status_args(dir.path()), Path::new("/"), None).await;
    assert_eq!(result.exit_code, CLI_EXIT.success);
}

#[tokio::test]
async fn seeded_project_fails_real_gates_and_correct_candidate_passes() {
    let dir = tempfile::tempdir().unwrap();
    let project = initialize(&dir.path().join("demo")).unwrap();
    for task in tasks().iter().filter(|t| t.blocked_reason.is_none()) {
        let code = std::fs::read_to_string(project.join(&task.file)).unwrap();
        assert_eq!(grade(&task.gate, &code).await.unwrap()["passed"], false);
    }
    let good = "def normalize_tags(text):\n    return list(dict.fromkeys(t.strip().lower() for t in text.split(',') if t.strip()))\n";
    let result = grade("tags", good).await.unwrap();
    assert_eq!(result["passed"], true);
    assert_eq!(result["assertions"], 332);
    assert!(result["evaluatorSha256"].is_string());
    assert_eq!(
        grade(
            "tags",
            "import os\ndef normalize_tags(text):\n    return []"
        )
        .await
        .unwrap()["passed"],
        false
    );
}

#[test]
fn proposal_scope_rejects_extra_files_deletions_and_stale_inputs() {
    let task = tasks().remove(0);
    let mut change = json!({"path":"tags.py","before":"old","beforeSha256":"digest","after":"new",
        "sourceChanged":false,"beforeMode":384,"afterMode":384});
    assert!(changes(&task, &json!({"changes":[change.clone()]})).is_ok());
    assert!(changes(&task, &json!({"changes":[change.clone(),change.clone()]})).is_err());
    change["sourceChanged"] = json!(true);
    assert!(changes(&task, &json!({"changes":[change.clone()]})).is_err());
    change["sourceChanged"] = json!(false);
    change["after"] = Value::Null;
    assert!(changes(&task, &json!({"changes":[change]})).is_err());
}
