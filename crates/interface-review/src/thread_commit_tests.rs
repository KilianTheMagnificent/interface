use super::*;
use std::{fmt::Write as _, os::unix::fs::PermissionsExt, process::Command};

fn raw(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim_end().into()
}
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    raw(dir.path(), &["init", "-q", "-b", "main"]);
    raw(dir.path(), &["config", "user.name", "Test"]);
    raw(dir.path(), &["config", "user.email", "test@example.com"]);
    raw(dir.path(), &["config", "commit.gpgsign", "false"]);
    fs::write(
        dir.path().join("one.txt"),
        (1..=30).fold(String::new(), |mut text, n| {
            writeln!(text, "line {n}").unwrap();
            text
        }),
    )
    .unwrap();
    fs::write(dir.path().join("two.txt"), "unchanged\n").unwrap();
    raw(dir.path(), &["add", "."]);
    raw(dir.path(), &["commit", "-qm", "seed"]);
    dir
}
fn modify(repo: &Path) -> String {
    let contents = fs::read_to_string(repo.join("one.txt"))
        .unwrap()
        .replace("line 2\n", "first change\n")
        .replace("line 28\n", "second change\n");
    fs::write(repo.join("one.txt"), &contents).unwrap();
    contents
}
fn choose(path: &str, hunks: Option<Vec<u32>>) -> CommitChoice {
    CommitChoice {
        path: path.into(),
        hunks,
    }
}

#[test]
fn selected_hunk_preserves_unselected_content_and_unrelated_staging() {
    let repo = repo();
    let root = repo.path();
    let contents = modify(root);
    fs::write(root.join("two.txt"), "staged elsewhere\n").unwrap();
    raw(root, &["add", "two.txt"]);
    let review = review(root, Some(&BTreeSet::new())).unwrap();
    assert_eq!(review.files.len(), 2);
    assert!(!review.files[0].attributed);
    assert_eq!(review.files[0].hunks.len(), 2);
    let result = selected(
        root,
        &review.revision,
        "first only",
        &[choose("one.txt", Some(vec![0]))],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Done(_)), "{result:?}");
    let committed = raw(root, &["show", "HEAD:one.txt"]);
    assert!(committed.contains("first change"));
    assert!(committed.contains("line 28"));
    assert!(!committed.contains("second change"));
    assert_eq!(fs::read_to_string(root.join("one.txt")).unwrap(), contents);
    assert_eq!(raw(root, &["diff", "--cached", "--name-only"]), "two.txt");
    assert_eq!(raw(root, &["show", ":two.txt"]), "staged elsewhere");
    assert_eq!(raw(root, &["show", "HEAD:two.txt"]), "unchanged");
}

#[test]
fn attribution_requires_explicit_file_evidence_even_in_an_owned_worktree() {
    let repo = repo();
    modify(repo.path());
    fs::write(repo.path().join("two.txt"), "shell edit\n").unwrap();
    let unknown = review(repo.path(), None).unwrap();
    assert!(unknown.files.iter().all(|file| !file.attributed));
    let named = BTreeSet::from(["one.txt".into()]);
    let known = review(repo.path(), Some(&named)).unwrap();
    assert!(
        known
            .files
            .iter()
            .find(|file| file.path == "one.txt")
            .unwrap()
            .attributed
    );
    assert!(
        !known
            .files
            .iter()
            .find(|file| file.path == "two.txt")
            .unwrap()
            .attributed
    );
}

#[test]
fn selected_refuses_stale_content_even_when_the_file_list_is_unchanged() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    let head = raw(root, &["rev-parse", "HEAD"]);
    fs::write(root.join("one.txt"), "another agent edited this\n").unwrap();
    assert!(matches!(
        selected(
            root,
            &reviewed.revision,
            "stale",
            &[choose("one.txt", None)]
        )
        .unwrap(),
        ShipEffect::Refused(_)
    ));
    assert_eq!(raw(root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn selection_does_not_replace_conflicting_staged_content() {
    let repo = repo();
    let root = repo.path();
    fs::write(root.join("one.txt"), "staged version\n").unwrap();
    raw(root, &["add", "one.txt"]);
    fs::write(root.join("one.txt"), "working version\n").unwrap();
    let reviewed = review(root, None).unwrap();
    let result = selected(
        root,
        &reviewed.revision,
        "conflict",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Refused(_)), "{result:?}");
    assert_eq!(raw(root, &["show", ":one.txt"]), "staged version");
    assert_eq!(
        fs::read_to_string(root.join("one.txt")).unwrap(),
        "working version\n"
    );
}

#[test]
fn explicit_unattributed_new_file_can_be_committed_and_other_files_stay_untracked() {
    let repo = repo();
    let root = repo.path();
    fs::write(root.join("shell.txt"), "shell output\n").unwrap();
    fs::write(root.join("other.txt"), "other output\n").unwrap();
    fs::write(root.join(".env"), "TOKEN=secret\n").unwrap();
    let reviewed = review(root, Some(&BTreeSet::new())).unwrap();
    assert!(
        reviewed
            .files
            .iter()
            .find(|f| f.path == ".env")
            .unwrap()
            .blocked
            .is_some()
    );
    assert!(matches!(
        selected(
            root,
            &reviewed.revision,
            "shell result",
            &[choose("shell.txt", None)]
        )
        .unwrap(),
        ShipEffect::Done(_)
    ));
    assert_eq!(raw(root, &["show", "HEAD:shell.txt"]), "shell output");
    assert_eq!(raw(root, &["ls-files", "other.txt"]), "");
}

#[test]
fn hooks_run_and_cannot_silently_commit_unreviewed_changes() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let hook = root.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\nprintf 'hook changed it\\n' > one.txt\ngit add one.txt\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let reviewed = review(root, None).unwrap();
    let head = raw(root, &["rev-parse", "HEAD"]);
    assert!(matches!(
        selected(root, &reviewed.revision, "hook", &[choose("one.txt", None)]).unwrap(),
        ShipEffect::Refused(_)
    ));
    assert_eq!(raw(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(raw(root, &["diff", "--cached", "--name-only"]), "");
    assert!(!root.join(".git/index.lock").exists());
}

#[test]
fn existing_index_lock_and_duplicate_hunks_are_refused_without_moving_head() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    let head = raw(root, &["rev-parse", "HEAD"]);
    assert!(
        selected(
            root,
            &reviewed.revision,
            "bad",
            &[choose("one.txt", Some(vec![0, 0]))]
        )
        .is_err()
    );
    fs::write(root.join(".git/index.lock"), "someone else's lock").unwrap();
    assert!(selected(root, &reviewed.revision, "busy", &[choose("one.txt", None)]).is_err());
    assert_eq!(
        fs::read_to_string(root.join(".git/index.lock")).unwrap(),
        "someone else's lock"
    );
    assert_eq!(raw(root, &["rev-parse", "HEAD"]), head);
}

fn install_hook(root: &Path, name: &str, script: &str) {
    let hook = root.join(".git/hooks").join(name);
    fs::write(&hook, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
}

fn prepared_index_contains(root: &Path, expected: &str) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_INDEX_FILE", root.join(".git/index.lock"))
        .args(["show", ":one.txt"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains(expected));
}

#[test]
fn replaced_index_lock_is_never_removed_or_installed_when_a_hook_returns_or_fails() {
    for exit in [0, 1] {
        let repo = repo();
        let root = repo.path();
        modify(root);
        let reviewed = review(root, None).unwrap();
        let head = raw(root, &["rev-parse", "HEAD"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        install_hook(
            root,
            "pre-commit",
            &format!("rm .git/index.lock\nprintf 'foreign lock' > .git/index.lock\nexit {exit}"),
        );
        let result = selected(
            root,
            &reviewed.revision,
            "hook replaced lock",
            &[choose("one.txt", None)],
        );
        assert!(
            matches!(result, Err(_) | Ok(ShipEffect::Refused(_))),
            "{result:?}"
        );
        assert_eq!(raw(root, &["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read_to_string(root.join(".git/index.lock")).unwrap(),
            "foreign lock"
        );
    }
}

#[test]
fn preparing_the_index_truncates_same_inode_hook_writes() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    install_hook(
        root,
        "pre-commit",
        "dd if=/dev/zero of=.git/index.lock bs=8192 count=1 2>/dev/null",
    );
    let result = selected(
        root,
        &reviewed.revision,
        "prepare own lock",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Done(_)), "{result:?}");
    assert!(fs::metadata(root.join(".git/index")).unwrap().len() < 8192);
    assert_eq!(raw(root, &["diff", "--cached", "--name-only"]), "");
    assert!(!root.join(".git/index.lock").exists());
}

#[test]
fn head_switch_after_branch_update_keeps_the_prepared_index_for_manual_recovery() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    raw(root, &["branch", "other"]);
    let reviewed = review(root, None).unwrap();
    let original = fs::read(root.join(".git/index")).unwrap();
    install_hook(
        root,
        "reference-transaction",
        "if [ \"$1\" = committed ]; then printf 'ref: refs/heads/other\\n' > .git/HEAD; fi",
    );
    let result = selected(
        root,
        &reviewed.revision,
        "ref switched",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Unknown(_)), "{result:?}");
    assert_eq!(raw(root, &["symbolic-ref", "HEAD"]), "refs/heads/other");
    assert!(raw(root, &["show", "main:one.txt"]).contains("first change"));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
    prepared_index_contains(root, "first change");
}

#[test]
fn unobservable_branch_after_update_retains_the_prepared_index_and_is_never_retry_safe() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    let original = fs::read(root.join(".git/index")).unwrap();
    install_hook(
        root,
        "reference-transaction",
        "if [ \"$1\" = committed ]; then rm .git/refs/heads/main; fi",
    );
    let result = selected(
        root,
        &reviewed.revision,
        "ref disappeared",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Unknown(_)), "{result:?}");
    assert!(!root.join(".git/refs/heads/main").exists());
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
    prepared_index_contains(root, "first change");
}

#[test]
fn foreign_lock_after_branch_update_is_never_installed_or_deleted() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    let original = fs::read(root.join(".git/index")).unwrap();
    install_hook(
        root,
        "reference-transaction",
        "if [ \"$1\" = committed ]; then rm .git/index.lock; printf 'foreign lock' > .git/index.lock; fi",
    );
    let result = selected(
        root,
        &reviewed.revision,
        "ref replaced lock",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Unknown(_)), "{result:?}");
    assert!(raw(root, &["show", "HEAD:one.txt"]).contains("first change"));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), original);
    assert_eq!(
        fs::read_to_string(root.join(".git/index.lock")).unwrap(),
        "foreign lock"
    );
}

#[test]
fn external_index_write_after_branch_update_is_preserved() {
    let repo = repo();
    let root = repo.path();
    modify(root);
    let reviewed = review(root, None).unwrap();
    install_hook(
        root,
        "reference-transaction",
        "if [ \"$1\" = committed ]; then printf 'externally replaced index' > .git/index; fi",
    );
    let result = selected(
        root,
        &reviewed.revision,
        "ref changed index",
        &[choose("one.txt", None)],
    )
    .unwrap();
    assert!(matches!(result, ShipEffect::Unknown(_)), "{result:?}");
    assert_eq!(
        fs::read_to_string(root.join(".git/index")).unwrap(),
        "externally replaced index"
    );
    prepared_index_contains(root, "first change");
}
