//! Protocol 37's ship commands end to end: real git over a bare remote and a
//! fake `gh` that logs its calls and answers from files.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use interface_contracts::threads::{
    ChecksState, DiffFileStatus, DiffSide, GhState, LeftOutReason, PullRequestState, SettledReason,
    ShipAction, ShipCertainty, ShipFile, ShipLeftOut, ThreadSection, ThreadShip,
};
use interface_review::thread_git::ship::Gh;
use serde_json::json;

use super::*;

fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    let output = command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A bare `remote.git` holding `main` (README.md, src/lib.rs) and `name`, a
/// clone of it with an identity, under `parent`.
fn clone_of_remote(parent: &Path, name: &str) -> PathBuf {
    let bare = parent.join("remote.git");
    if !bare.exists() {
        let seed = parent.join("seed");
        fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["config", "user.email", "t@example.com"]);
        git(&seed, &["config", "user.name", "T"]);
        write(&seed, "README.md", "# atlas\n");
        write(&seed, "src/lib.rs", "pub fn one() {}\n");
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "seed"]);
        git(parent, &["clone", "-q", "--bare", "seed", "remote.git"]);
    }
    git(parent, &["clone", "-q", bare.to_str().unwrap(), name]);
    let work = parent.join(name);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "T"]);
    work
}

const FAKE_GH: &str = r#"#!/bin/sh
state="$(dirname "$0")/state"
printf '%s\n' "$*" >> "$state/calls.log"
answer() { if [ -f "$state/$1" ]; then cat "$state/$1"; else printf '%s' "$2"; fi; }
case "$1 $2" in
  "auth status")
    if [ -f "$state/signed-out" ]; then echo "You are not logged into any GitHub hosts." >&2; exit 1; fi
    exit 0 ;;
  "pr list") answer pr-list.json "[]"; exit 0 ;;
  "pr view") answer pr-view.json ""; exit 0 ;;
  "pr create")
    cat > "$state/create-body.txt"
    if [ -f "$state/create-hangs" ]; then sleep 5; fi
    cp "$state/created-list.json" "$state/pr-list.json"
    cp "$state/created-view.json" "$state/pr-view.json"
    echo "https://github.com/owner/atlas/pull/42"
    exit 0 ;;
  "pr checks") answer checks.json "[]"; exit "$(answer checks-exit 0)" ;;
  "run view") answer log.txt ""; exit 0 ;;
  "api graphql") answer graphql.json "{}"; exit 0 ;;
  "pr merge")
    cp "$state/merged-view.json" "$state/pr-view.json"
    printf '[]' > "$state/pr-list.json"
    exit 0 ;;
esac
exit 2
"#;

const PR: &str = r#"{"number":42,"url":"https://github.com/owner/atlas/pull/42","state":"OPEN","title":"Title local","baseRefName":"main","isDraft":false}"#;

struct FakeGh {
    dir: tempfile::TempDir,
}

impl FakeGh {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("state")).unwrap();
        let program = dir.path().join("gh");
        fs::write(&program, FAKE_GH).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let fake = Self { dir };
        fake.set("created-list.json", &format!("[{PR}]"));
        fake.set("created-view.json", PR);
        fake.set("merged-view.json", &PR.replace("OPEN", "MERGED"));
        fake
    }

    fn gh(&self) -> Gh {
        Gh::at(self.dir.path().join("gh"))
    }

    fn set(&self, name: &str, contents: &str) {
        fs::write(self.dir.path().join("state").join(name), contents).unwrap();
    }

    fn calls(&self, prefix: &str) -> usize {
        fs::read_to_string(self.dir.path().join("state/calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .count()
    }
}

impl Harness {
    /// Ends `key`'s run after its edits named `paths`, as the tap would.
    async fn edited(&mut self, key: &str, paths: &[String]) {
        self.host.open_run(key);
        let mut ops = lock(&self.host.inner.ops_receiver).take().unwrap();
        for path in paths {
            self.host.observe_run(
                key,
                &EngineEventKind::DiffProduced {
                    call_id: "edit".into(),
                    path: path.clone(),
                    before: String::new(),
                    after: "x\n".into(),
                    agent_key: None,
                },
            );
        }
        self.host.observe_run(
            key,
            &EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
        );
        self.host.apply_run(ops.try_recv().unwrap()).await;
        settle_tasks(&self.host).await;
        *lock(&self.host.inner.ops_receiver) = Some(ops);
    }

    async fn ship_status(&self, key: &str, remote: bool) -> ThreadShip {
        self.command(json!({"kind": "shipStatus", "key": key, "remote": remote}))
            .await
            .unwrap()
            .ship
            .unwrap()
    }
}

/// The owner's ship flow from a Local thread: Commit stages only the
/// thread's own files (never `.env`), a push to the default branch is
/// refused until a branch is made, Push & open PR looks for an open pull
/// request before opening one, the CI row's prompt quotes the failed log
/// without credentials, review comments come back for the draft, and a
/// confirmed merge settles the thread through settle-on-merge.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One flow, commit to merge.
async fn a_local_thread_commits_its_own_files_opens_one_pull_request_and_merges_to_settle() {
    let mut harness = harness();
    let fake = FakeGh::new();
    *lock(&harness.ship.0) = Some(fake.gh());
    let parent = PathBuf::from(harness.dir("ship"));
    let work = clone_of_remote(&parent, "work");
    let root = work.display().to_string();
    let key = "claude#local";
    harness
        .git_thread(key, &root, None, "Fix the login test")
        .await;
    harness
        .edited(
            key,
            &[
                format!("{root}/src/lib.rs"),
                "src/new.rs".into(),
                ".env".into(),
            ],
        )
        .await;
    write(&work, "src/lib.rs", "pub fn one() { fixed }\n");
    write(&work, "src/new.rs", "pub fn two() {}\n");
    write(&work, "README.md", "the owner's own edit\n");
    write(&work, ".env", "TOKEN=1\n");

    let ship = harness.ship_status(key, true).await;
    assert_eq!(
        ship.files,
        [
            ShipFile {
                path: "src/lib.rs".into(),
                status: DiffFileStatus::Modified
            },
            ShipFile {
                path: "src/new.rs".into(),
                status: DiffFileStatus::Added
            },
        ]
    );
    assert_eq!(
        ship.left_out,
        [
            ShipLeftOut {
                path: ".env".into(),
                reason: LeftOutReason::Credential
            },
            ShipLeftOut {
                path: "README.md".into(),
                reason: LeftOutReason::NotThisThread
            },
        ]
    );
    assert!(!ship.files_unknown);
    assert_eq!(ship.branch.as_deref(), Some("main"));
    assert!(
        ship.push_blocked
            .as_deref()
            .unwrap()
            .contains("default branch")
    );
    let suggested = ship.suggested_branch.clone().unwrap();
    assert!(suggested.starts_with("iface/title-local-"), "{suggested}");
    assert_eq!((ship.gh, ship.gh_fix.as_deref()), (GhState::Ready, None));
    assert!(
        ship.message
            .starts_with("Title local\n\n- Fix the login test")
    );

    // What the owner saw is what is committed; anything else is stale.
    let stale = harness
        .command(json!({"kind": "commit", "key": key, "message": "Fix", "paths": ["src/lib.rs"]}))
        .await
        .unwrap_err();
    assert_eq!(stale.code, THREADS_SHIP_STALE_CODE);
    let committed = harness
        .command(
            json!({"kind": "commit", "key": key, "message": ship.message,
            "paths": ["src/new.rs", "src/lib.rs"]}),
        )
        .await
        .unwrap();
    let shipped = committed.shipped.unwrap();
    assert_eq!(
        (shipped.action, shipped.certainty),
        (ShipAction::Commit, ShipCertainty::Done)
    );
    assert!(shipped.message.starts_with("Committed 2 files as "));
    assert_eq!(
        git(&work, &["show", "--name-only", "--format=", "HEAD"]),
        "src/lib.rs\nsrc/new.rs"
    );
    assert_eq!(
        git(
            &work,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        ),
        "M README.md\n?? .env"
    );
    assert!(committed.ship.unwrap().files.is_empty());

    // Never to the default branch: refused, nothing pushed or opened.
    let blocked = harness
        .command(json!({"kind": "openPullRequest", "key": key, "title": "Fix", "body": ""}))
        .await
        .unwrap_err();
    assert_eq!(blocked.code, THREADS_SHIP_BLOCKED_CODE);
    assert!(blocked.replay_safe);
    assert_eq!(fake.calls("pr create"), 0);

    let branched = harness
        .command(json!({"kind": "createBranch", "key": key, "branch": suggested}))
        .await
        .unwrap();
    assert_eq!(
        branched.ship.unwrap().branch.as_deref(),
        Some(suggested.as_str())
    );

    let opened = harness
        .command(json!({"kind": "openPullRequest", "key": key,
            "title": "Title local", "body": "- Fix the login test"}))
        .await
        .unwrap();
    let shipped = opened.shipped.unwrap();
    assert_eq!(shipped.certainty, ShipCertainty::Done);
    assert!(
        shipped.message.contains("opened #42"),
        "{}",
        shipped.message
    );
    assert_eq!(
        git(
            &work,
            &["ls-remote", "origin", &format!("refs/heads/{suggested}")]
        )
        .split('\t')
        .next(),
        Some(git(&work, &["rev-parse", "HEAD"]).as_str())
    );
    let pull_request = thread(&opened.view, key).pull_request.clone().unwrap();
    assert_eq!(
        (pull_request.number, pull_request.state),
        (42, PullRequestState::Open)
    );
    // Opened here, so tracked here, even with background tracking off.
    harness
        .command(
            json!({"kind": "setSettings", "settings": {"autoSettleDays": 3,
            "settleOnMerge": true, "generatedTitles": false, "trackPullRequests": false}}),
        )
        .await
        .unwrap();
    let document = harness.document().await;
    assert!(document.record(key).unwrap().pull_request_tracked);
    assert_eq!(document.pull_requests_due(NOW + DAY, Some(key)).len(), 1);
    assert_eq!(opened.ship.unwrap().pull_request.unwrap().number, 42);

    // Again: the open pull request is found first, never a second one.
    let again = harness
        .command(json!({"kind": "openPullRequest", "key": key, "title": "Again", "body": ""}))
        .await
        .unwrap();
    assert!(again.shipped.unwrap().message.contains("already open"));
    assert_eq!(fake.calls("pr create"), 1);

    // The CI row: failing checks become a prompt with the failed log's tail.
    fake.set(
        "checks.json",
        r#"[{"name":"build","state":"FAILURE","bucket":"fail","link":"https://github.com/owner/atlas/actions/runs/7/job/8","workflow":"CI"},
            {"name":"lint","state":"SUCCESS","bucket":"pass","link":"https://github.com/owner/atlas/actions/runs/7/job/9","workflow":"CI"}]"#,
    );
    fake.set("checks-exit", "1");
    fake.set(
        "log.txt",
        "build\tTest\t2026-10-14T12:00:00.0000000Z error: expected 2, got 3\nbuild\tTest\t2026-10-14T12:00:01.0000000Z GITHUB_TOKEN=ghs_abcdefghijklmnopqrstuvwxyz0123\n",
    );
    let ship = harness.ship_status(key, true).await;
    let checks = ship.checks.unwrap();
    assert_eq!(
        (checks.state, checks.failed, checks.passed),
        (ChecksState::Failing, 1, 1)
    );
    let prompt = harness
        .command(json!({"kind": "failingChecksPrompt", "key": key}))
        .await
        .unwrap()
        .prompt
        .unwrap();
    assert!(
        prompt.contains("#42") && prompt.contains("CI / build"),
        "{prompt}"
    );
    assert!(prompt.contains("error: expected 2, got 3"), "{prompt}");
    assert!(!prompt.contains("ghs_"), "{prompt}");
    assert!(prompt.contains("[redacted]"), "{prompt}");

    fake.set(
        "graphql.json",
        r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
          {"isResolved":false,"isOutdated":false,"path":"src/lib.rs","line":1,"startLine":null,"diffSide":"RIGHT",
           "comments":{"nodes":[{"author":{"login":"reviewer"},"body":"Say why.","url":"u1","diffHunk":"@@ -1 +1 @@\n-pub fn one() {}\n+pub fn one() { fixed }"}]}}]}}}}}"#,
    );
    let comments = harness
        .command(json!({"kind": "reviewComments", "key": key}))
        .await
        .unwrap()
        .review_comments
        .unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(
        (
            comments[0].path.as_str(),
            comments[0].end_line,
            comments[0].side
        ),
        ("src/lib.rs", Some(1), DiffSide::New)
    );
    assert_eq!(comments[0].comment, "@reviewer: Say why.");

    // Merge only the pull request the owner confirmed, then settle.
    let wrong = harness
        .command(json!({"kind": "mergePullRequest", "key": key, "number": 41, "method": "squash"}))
        .await
        .unwrap_err();
    assert_eq!(wrong.code, THREADS_NO_PULL_REQUEST_CODE);
    assert_eq!(fake.calls("pr merge"), 0);
    let merged = harness
        .command(json!({"kind": "mergePullRequest", "key": key, "number": 42, "method": "squash"}))
        .await
        .unwrap();
    assert_eq!(merged.shipped.unwrap().message, "Merged #42.");
    assert_eq!(fake.calls("pr merge 42 --squash"), 1);
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    let settled = thread(&view, key);
    assert_eq!(settled.section, ThreadSection::Settled);
    assert_eq!(settled.settled_reason, Some(SettledReason::Merged));
    assert_eq!(
        settled.pull_request.as_ref().unwrap().state,
        PullRequestState::Merged
    );
}

/// `gh` missing or signed out: the controls say how to fix it, never silently
/// nothing. A worktree thread commits every change but credentials. A pull
/// request create that runs out of time is re-read, answered unknown when it
/// is not there, and never tried again by itself.
#[tokio::test]
async fn ship_says_how_to_fix_gh_and_never_retries_an_unknown_pull_request() {
    let mut harness = harness();
    let parent = PathBuf::from(harness.dir("ship"));
    let root = clone_of_remote(&parent, "work");
    let worktree = clone_of_remote(&parent, "worktree");
    git(&worktree, &["switch", "-q", "-c", "iface/fix-1a2b3c"]);
    let key = "codex#wt";
    harness
        .git_thread(
            key,
            &root.display().to_string(),
            Some(&worktree.display().to_string()),
            "Fix it",
        )
        .await;

    let ship = harness.ship_status(key, true).await;
    assert_eq!(ship.gh, GhState::Missing);
    let fix = ship.gh_fix.unwrap();
    assert!(
        fix.contains("brew install gh") && fix.contains("gh auth login"),
        "{fix}"
    );
    let refused = harness
        .command(json!({"kind": "openPullRequest", "key": key, "title": "Fix", "body": ""}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, THREADS_GH_UNAVAILABLE_CODE);
    assert!(refused.message.contains("gh auth login"));

    let fake = FakeGh::new();
    fake.set("signed-out", "");
    *lock(&harness.ship.0) = Some(fake.gh());
    let ship = harness.ship_status(key, true).await;
    assert_eq!(ship.gh, GhState::SignedOut);
    assert!(ship.gh_fix.unwrap().contains("gh auth login"));
    fs::remove_file(fake.dir.path().join("state/signed-out")).unwrap();

    // A worktree is the thread's own: every change, but never `.env`.
    write(&worktree, "src/lib.rs", "pub fn one() { fixed }\n");
    write(&worktree, "notes.md", "made by a shell command\n");
    write(&worktree, ".env", "TOKEN=1\n");
    let ship = harness.ship_status(key, false).await;
    assert_eq!(
        ship.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["notes.md", "src/lib.rs"]
    );
    assert_eq!(ship.push_blocked, None);
    assert_eq!(ship.suggested_branch, None);
    harness
        .command(json!({"kind": "commit", "key": key, "message": "Fix it",
            "paths": ["notes.md", "src/lib.rs"]}))
        .await
        .unwrap();
    let not_local = harness
        .command(json!({"kind": "createBranch", "key": key, "branch": "other"}))
        .await
        .unwrap_err();
    assert_eq!(not_local.code, THREADS_SHIP_BLOCKED_CODE);

    // gh runs out of time creating it and nothing is there on a re-read:
    // unknown, once.
    fake.set("create-hangs", "");
    fake.set("created-list.json", "[]");
    *lock(&harness.ship.0) = Some(
        fake.gh()
            .with_timeouts(Duration::from_secs(10), Duration::from_millis(500)),
    );
    let unknown = harness
        .command(json!({"kind": "openPullRequest", "key": key, "title": "Fix it", "body": ""}))
        .await
        .unwrap();
    let shipped = unknown.shipped.unwrap();
    assert_eq!(shipped.certainty, ShipCertainty::Unknown);
    assert!(
        shipped.message.contains("Check again"),
        "{}",
        shipped.message
    );
    assert_eq!(fake.calls("pr create"), 1, "never retried");
    assert_eq!(
        fake.calls("pr list"),
        3,
        "looked before, re-read after, and in the answer"
    );
    assert!(thread(&unknown.view, key).pull_request.is_none());
}

/// Write with AI (protocol 67, T3 section 12): the commit message is written
/// from the files being committed (never a credential-shaped one) and the
/// thread's prompts; the pull request's from the branch's commits since its
/// default branch. Nothing is committed or pushed, Claude missing refuses
/// before anything is read, and a failed call changes nothing.
#[tokio::test]
async fn write_with_ai_reads_only_what_ships_and_changes_nothing() {
    let mut harness = harness();
    let parent = PathBuf::from(harness.dir("ship-text"));
    let work = clone_of_remote(&parent, "work");
    let root = work.display().to_string();
    let key = "claude#local";
    harness
        .git_thread(key, &root, None, "Fix the login test")
        .await;
    harness
        .edited(
            key,
            &[
                format!("{root}/src/lib.rs"),
                "src/new.rs".into(),
                ".env".into(),
            ],
        )
        .await;
    write(&work, "src/lib.rs", "pub fn one() { fixed }\n");
    write(&work, "src/new.rs", "pub fn two() {}\n");
    write(&work, "README.md", "the owner's own edit\n");
    write(&work, ".env", "TOKEN=hunter2-secret\n");
    *lock(&harness.titles.reply) = Ok("Fix the login test\n\nIt waited too little.".into());
    let head = git(&work, &["rev-parse", "HEAD"]);

    let answer = harness
        .command(json!({"kind": "suggestShipText", "key": key, "what": "commit"}))
        .await
        .unwrap();
    assert_eq!(
        answer.ship_text.as_deref(),
        Some("Fix the login test\n\nIt waited too little.")
    );
    let (what, material) = lock(&harness.titles.ship_calls).pop().unwrap();
    assert_eq!(what, interface_contracts::threads::ShipTextKind::Commit);
    assert!(material.contains(" M src/lib.rs | +1 -1\n"), "{material}");
    assert!(material.contains("+pub fn one() { fixed }"), "{material}");
    assert!(
        !material.contains("README.md"),
        "only the thread's files: {material}"
    );
    assert!(!material.contains("hunter2"), "{material}");
    assert!(material.contains("<prompts>\n- Fix the login test\n</prompts>"));
    // Paths the owner picked, filtered to what changed and is not a credential.
    harness
        .command(
            json!({"kind": "suggestShipText", "key": key, "what": "commit",
            "paths": ["README.md", ".env", "nope.rs"]}),
        )
        .await
        .unwrap();
    let (_, material) = lock(&harness.titles.ship_calls).pop().unwrap();
    assert!(material.contains(" M README.md | +1 -1\n"), "{material}");
    assert!(
        !material.contains(".env") && !material.contains("hunter2"),
        "{material}"
    );
    // Nothing was committed, staged or pushed.
    assert_eq!(git(&work, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&work, &["diff", "--cached", "--name-only"]), "");

    // A pull request: the branch's committed work since origin/main.
    git(&work, &["switch", "-q", "-c", "iface/login"]);
    git(&work, &["add", "src/lib.rs"]);
    git(&work, &["commit", "-q", "-m", "Fix one"]);
    harness
        .command(json!({"kind": "suggestShipText", "key": key, "what": "pullRequest"}))
        .await
        .unwrap();
    let (what, material) = lock(&harness.titles.ship_calls).pop().unwrap();
    assert_eq!(
        what,
        interface_contracts::threads::ShipTextKind::PullRequest
    );
    assert!(
        material.starts_with("<changes>\n M src/lib.rs | +1 -1\n</changes>"),
        "{material}"
    );

    // A failed call changes nothing and may be asked again.
    *lock(&harness.titles.reply) = Err("Claude exited with an error".into());
    let failed = harness
        .command(json!({"kind": "suggestShipText", "key": key, "what": "commit"}))
        .await
        .unwrap_err();
    assert_eq!(failed.code, THREADS_SHIP_TEXT_FAILED_CODE);
    assert!(failed.replay_safe && failed.message.contains("exited with an error"));
    // Claude missing: refused before anything is read.
    harness.titles.available.store(false, Ordering::SeqCst);
    let calls = lock(&harness.titles.ship_calls).len();
    let missing = harness
        .command(json!({"kind": "suggestShipText", "key": key, "what": "commit"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, THREADS_SHIP_TEXT_UNAVAILABLE_CODE);
    assert!(missing.replay_safe && missing.message.contains("not installed"));
    assert_eq!(lock(&harness.titles.ship_calls).len(), calls);
}
