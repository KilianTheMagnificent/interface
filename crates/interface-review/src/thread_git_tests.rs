use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output as ProcessOutput};

use tempfile::TempDir;

use super::*;

const START: &str = "refs/interface/threads/0123456789abcdef/0/start";
const END: &str = "refs/interface/threads/0123456789abcdef/0/end";
const RECOVERY: &str = "refs/interface/threads/0123456789abcdef/recovery/1";

struct Repo {
    _dir: TempDir,
    /// The temporary folder holding the repository; thread worktrees land in
    /// its `.interface-worktrees`.
    parent: PathBuf,
    root: PathBuf,
}

fn raw_git(dir: &Path, args: &[&str]) -> ProcessOutput {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs")
}

fn setup_git(dir: &Path, args: &[&str]) -> String {
    let output = raw_git(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    raw_git(dir, args).status.success()
}

fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap()
}

/// `project/` with a committed `.gitignore` (`*.log`, `build/`), `src/lib.rs`,
/// `README.md`, `docs/guide.md` and a tracked `.env`.
fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let parent = fs::canonicalize(dir.path()).unwrap();
    let root = parent.join("project");
    fs::create_dir(&root).unwrap();
    setup_git(&root, &["init", "-q", "-b", "main"]);
    write(&root, ".gitignore", "*.log\nbuild/\n");
    write(&root, "src/lib.rs", "fn one() {}\nfn two() {}\n");
    write(&root, "README.md", "hello\n");
    write(
        &root,
        "docs/guide.md",
        (1..=10).fold(String::new(), |mut text, line| {
            let _ = writeln!(text, "guide line {line}");
            text
        }),
    );
    write(&root, ".env", "TOKEN=old\n");
    setup_git(&root, &["add", "-A"]);
    setup_git(&root, &["add", "-f", ".env"]);
    setup_git(&root, &["commit", "-q", "-m", "initial"]);
    Repo {
        _dir: dir,
        parent,
        root,
    }
}

fn status(root: &Path) -> String {
    setup_git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
}

fn captured(outcome: CaptureOutcome) -> String {
    match outcome {
        CaptureOutcome::Captured { commit } => commit,
        CaptureOutcome::Skipped { reason } => panic!("checkpoint skipped: {reason}"),
    }
}

fn skipped(outcome: CaptureOutcome) -> String {
    match outcome {
        CaptureOutcome::Skipped { reason } => reason,
        CaptureOutcome::Captured { commit } => panic!("checkpoint captured {commit}"),
    }
}

fn interface_refs(root: &Path) -> String {
    setup_git(
        root,
        &["for-each-ref", "--format=%(refname)", "refs/interface/"],
    )
}

fn file<'a>(diff: &'a ThreadDiff, path: &str) -> &'a DiffFile {
    diff.files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} missing from {:?}", diff.files))
}

fn worktree_request(project_root: &Path, hint: &str) -> GitWorkspaceRequest {
    GitWorkspaceRequest {
        project_root: project_root.to_path_buf(),
        mode: PrepareMode::Worktree,
        base_ref: None,
        worktree_path: None,
        hint: hint.to_owned(),
    }
}

#[test]
fn checkpoint_capture_never_changes_git_status_or_the_index() {
    let repo = repo();
    let root = &repo.root;
    write(root, "src/lib.rs", "fn one() {}\nfn two() { changed }\n");
    write(root, "notes.txt", "draft\n");
    write(root, "debug.log", "noise\n");
    write(root, "staged.txt", "staged\n");
    setup_git(root, &["add", "staged.txt"]);
    setup_git(root, &["update-index", "-q", "--refresh"]);
    let index = root.join(".git/index");
    let index_before = fs::read(&index).unwrap();
    let status_before = status(root);
    let head = setup_git(root, &["rev-parse", "HEAD"]);

    // From a subfolder: the whole repository is captured.
    let commit = captured(capture_checkpoint(&root.join("src"), START).unwrap());

    assert_eq!(fs::read(&index).unwrap(), index_before);
    assert_eq!(status(root), status_before);
    assert_eq!(setup_git(root, &["rev-parse", START]), commit);
    assert_eq!(setup_git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        setup_git(root, &["show", &format!("{commit}:notes.txt")]),
        "draft"
    );
    assert!(setup_git(root, &["show", &format!("{commit}:src/lib.rs")]).contains("changed"));
    assert!(git_ok(
        root,
        &["cat-file", "-e", &format!("{commit}:staged.txt")]
    ));
    assert!(!git_ok(
        root,
        &["cat-file", "-e", &format!("{commit}:debug.log")]
    ));
    assert_eq!(
        setup_git(root, &["rev-list", "--parents", "-n", "1", &commit]),
        format!("{commit} {head}")
    );
    assert_eq!(
        setup_git(
            root,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%s", &commit]
        ),
        "Interface <interface@localhost>|Interface <interface@localhost>|interface thread checkpoint"
    );
    // The working tree itself is untouched.
    assert!(read(root, "src/lib.rs").contains("changed"));
    assert_eq!(read(root, "debug.log"), "noise\n");
}

#[test]
fn checkpoint_capture_skips_non_repositories_and_over_limit_trees() {
    let plain = tempfile::tempdir().unwrap();
    assert!(
        skipped(capture_checkpoint(plain.path(), START).unwrap()).contains("not a git repository")
    );

    let repo = repo();
    let root = &repo.root;
    for name in ["a.txt", "b.txt", "c.txt"] {
        write(root, name, "x\n");
    }
    let few_paths = CaptureLimits {
        max_paths: 2,
        ..CAPTURE_LIMITS
    };
    assert!(skipped(capture_with(root, START, few_paths).unwrap()).contains("More than 2 files"));

    write(root, "big.bin", vec![b'x'; 2048]);
    let small_files = CaptureLimits {
        max_new_file_bytes: 1024,
        ..CAPTURE_LIMITS
    };
    assert!(
        skipped(capture_with(root, START, small_files).unwrap()).contains("big.bin is larger than")
    );

    let no_time = CaptureLimits {
        budget: Duration::ZERO,
        ..CAPTURE_LIMITS
    };
    assert!(skipped(capture_with(root, START, no_time).unwrap()).contains("longer than"));
    assert_eq!(interface_refs(root), "");

    assert_eq!(CAPTURE_LIMITS.max_paths, 20_000);
    assert_eq!(CAPTURE_LIMITS.max_new_file_bytes, 50 * 1024 * 1024);
    assert_eq!(CAPTURE_LIMITS.budget, Duration::from_secs(10));
}

/// 7 Oct 2026: a thread started in a project on iCloud Drive waited out the
/// whole 10 s budget and then ran without its launch checkpoint. iCloud had
/// evicted HEAD's loose tree objects, and seeding the private index with
/// `read-tree -m HEAD` opened every one of them. A FIFO in place of one stands
/// in for an evicted object: opening it blocks. The capture seeds from the
/// real index, so it never opens HEAD's trees and finishes at once.
#[test]
fn checkpoint_capture_never_reads_head_trees_so_an_evicted_object_cannot_stall_it() {
    let repo = repo();
    let root = &repo.root;
    let docs = setup_git(root, &["rev-parse", "HEAD:docs"]);
    let object = root.join(".git/objects").join(&docs[..2]).join(&docs[2..]);
    fs::remove_file(&object).unwrap();
    let made = Command::new("mkfifo").arg(&object).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    write(root, "src/lib.rs", "fn one() {}\nfn two() { changed }\n");
    write(root, "notes.txt", "draft\n");

    let commit = captured(capture_checkpoint(root, START).unwrap());

    assert_eq!(
        setup_git(root, &["rev-parse", &format!("{commit}:docs")]),
        docs
    );
    assert_eq!(
        setup_git(root, &["show", &format!("{commit}:notes.txt")]),
        "draft"
    );
    assert!(setup_git(root, &["show", &format!("{commit}:src/lib.rs")]).contains("changed"));
}

#[test]
fn checkpoint_capture_records_a_conflicted_merge_as_its_working_tree() {
    let repo = repo();
    let root = &repo.root;
    setup_git(root, &["checkout", "-q", "-b", "other"]);
    write(root, "README.md", "theirs\n");
    setup_git(root, &["commit", "-q", "-am", "theirs"]);
    setup_git(root, &["checkout", "-q", "main"]);
    write(root, "README.md", "ours\n");
    setup_git(root, &["commit", "-q", "-am", "ours"]);
    assert!(!git_ok(root, &["merge", "-q", "other"]));
    let status_before = status(root);
    assert!(status_before.contains("UU README.md"));

    let commit = captured(capture_checkpoint(root, START).unwrap());

    assert!(setup_git(root, &["show", &format!("{commit}:README.md")]).contains(">>>>>>> other"));
    assert_eq!(status(root), status_before);
}

#[test]
fn checkpoint_capture_works_before_the_first_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    setup_git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "first.txt", "one\n");
    let commit = captured(capture_checkpoint(&root, START).unwrap());
    assert_eq!(
        setup_git(&root, &["rev-list", "--parents", "-n", "1", &commit]),
        commit
    );
    assert_eq!(
        setup_git(&root, &["show", &format!("{commit}:first.txt")]),
        "one"
    );
    assert!(!root.join(".git/index").exists());
    assert_eq!(status(&root), "?? first.txt\0");
}

#[test]
fn only_refs_under_refs_interface_are_written_or_deleted() {
    let repo = repo();
    let root = &repo.root;
    for refname in [
        "refs/heads/main",
        "refs/interface/",
        "refs/interface/a/../b",
        "refs/interface/a b",
        "refs/interface/a.lock",
        "HEAD",
    ] {
        let error = capture_checkpoint(root, refname).unwrap_err();
        assert_eq!(error.code, ThreadGitError::INVALID_CODE, "{refname}");
    }
    assert_eq!(
        delete_refs(root, "refs/heads").unwrap_err().code,
        ThreadGitError::INVALID_CODE
    );
    captured(capture_checkpoint(root, START).unwrap());
    assert_eq!(
        restore_to(root, START, "refs/heads/recovery")
            .unwrap_err()
            .code,
        ThreadGitError::INVALID_CODE
    );
    assert!(!git_ok(
        root,
        &["rev-parse", "--verify", "refs/heads/recovery"]
    ));
}

/// A turn that edits, adds, deletes, renames, writes a binary and touches a
/// credential file.
fn edit_everything(root: &Path) {
    write(root, "src/lib.rs", "fn one() {}\nfn two() { changed }\n");
    write(root, "notes.txt", "draft\nmore\n");
    fs::remove_file(root.join("README.md")).unwrap();
    fs::create_dir_all(root.join("manual")).unwrap();
    fs::rename(root.join("docs/guide.md"), root.join("manual/guide.md")).unwrap();
    write(root, "assets/logo.png", [0_u8, 1, 2, 3, 0, 255]);
    write(root, ".env", "TOKEN=new\n");
    write(root, "debug.log", "ignored\n");
}

#[test]
fn turn_numstat_and_diff_cover_tracked_untracked_renamed_binary_and_sensitive_files() {
    let repo = repo();
    let root = &repo.root;
    let start = captured(capture_checkpoint(root, START).unwrap());
    edit_everything(root);
    let end = captured(capture_checkpoint(root, END).unwrap());

    // lib.rs 1/1, notes.txt 2/0, README.md 0/1, the rename 0/0, the PNG and
    // `.env` 1/1.
    assert_eq!(numstat(root, START, Some(END)).unwrap(), (6, 4, 3));
    assert_eq!(numstat(root, START, None).unwrap(), (6, 4, 3));
    assert_eq!(numstat(root, END, None).unwrap(), (0, 0, 0));

    let turn = diff(
        root,
        &DiffRequest {
            scope: DiffScope::Turn,
            base: START.to_owned(),
            head: Some(END.to_owned()),
            paths: None,
        },
    )
    .unwrap();
    assert_eq!(turn.scope, DiffScope::Turn);
    assert_eq!(turn.base, start);
    assert_eq!(turn.head, end);
    assert!(!turn.truncated);
    assert_eq!(turn.files.len(), 6);

    let lib = file(&turn, "src/lib.rs");
    assert_eq!(lib.status, DiffFileStatus::Modified);
    assert_eq!((lib.added, lib.removed, lib.binary), (1, 1, false));
    let patch = lib.patch.as_deref().unwrap();
    assert!(
        patch.starts_with("diff --git a/src/lib.rs b/src/lib.rs\n"),
        "{patch}"
    );
    assert!(patch.contains("\n+fn two() { changed }\n"), "{patch}");
    assert!(lib.omitted.is_none());

    let notes = file(&turn, "notes.txt");
    assert_eq!(notes.status, DiffFileStatus::Added);
    assert!(notes.patch.as_deref().unwrap().contains("+draft\n+more\n"));

    let readme = file(&turn, "README.md");
    assert_eq!(readme.status, DiffFileStatus::Deleted);
    assert_eq!((readme.added, readme.removed), (0, 1));

    let renamed = file(&turn, "manual/guide.md");
    assert_eq!(renamed.status, DiffFileStatus::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("docs/guide.md"));
    assert!(
        renamed
            .patch
            .as_deref()
            .unwrap()
            .contains("rename from docs/guide.md")
    );

    let logo = file(&turn, "assets/logo.png");
    assert!(logo.binary);
    assert_eq!(logo.omitted, Some(DiffOmitted::Binary));
    assert!(logo.patch.is_none());

    let secret = file(&turn, ".env");
    assert_eq!(secret.omitted, Some(DiffOmitted::Sensitive));
    assert!(secret.patch.is_none());
    assert_eq!((secret.added, secret.removed), (1, 1));
    assert!(!turn.files.iter().any(|file| file.path == "debug.log"));

    let live = diff(
        root,
        &DiffRequest {
            scope: DiffScope::Thread,
            base: START.to_owned(),
            head: None,
            paths: None,
        },
    )
    .unwrap();
    assert_eq!(live.head, "working-tree");
    assert_eq!(live.base, start);
    let mut live_paths = live
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let mut turn_paths = turn
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    live_paths.sort();
    turn_paths.sort();
    assert_eq!(live_paths, turn_paths);
    assert_eq!(
        file(&live, "notes.txt").patch,
        file(&turn, "notes.txt").patch
    );
}

/// A turn in a checkout others edit too counts only the files its own edits
/// named.
#[test]
fn scoped_turn_numstat_counts_only_named_files_inside_the_repository() {
    let repo = repo();
    let root = &repo.root;
    captured(capture_checkpoint(root, START).unwrap());
    edit_everything(root);
    captured(capture_checkpoint(root, END).unwrap());

    // Scoped to the files a turn's own edits named (relative to the folder or
    // absolute): paths outside the repository, or climbing out of it, drop.
    let scoped = |paths: &[&str]| {
        let paths = paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();
        numstat_paths(root, START, Some(END), &paths).unwrap()
    };
    write(&repo.parent, "outside.txt", "not the repository's\n");
    let outside = repo.parent.join("outside.txt").display().to_string();
    let notes = root.join("notes.txt").display().to_string();
    assert_eq!(
        scoped(&[
            "src/lib.rs",
            &notes,
            &outside,
            "../outside.txt",
            "/../../etc/passwd"
        ]),
        (2, 3, 1)
    );
    let paths = ["src/lib.rs".to_owned(), notes.clone()];
    assert_eq!(numstat_paths(root, START, None, &paths).unwrap(), (2, 3, 1));
    // A deleted file still counts; a rename is one file only with both sides.
    assert_eq!(scoped(&["README.md"]), (1, 0, 1));
    assert_eq!(scoped(&["manual/guide.md"]), (1, 10, 0));
    assert_eq!(scoped(&["docs/guide.md", "manual/guide.md"]), (1, 0, 0));
    // Nothing left inside never widens to the whole tree.
    assert_eq!(scoped(&[&outside, "../outside.txt", ".", ""]), (0, 0, 0));
    assert_eq!(scoped(&[]), (0, 0, 0));
    // A symlinked spelling of the repository matches; a symlink out does not.
    std::os::unix::fs::symlink(root, repo.parent.join("alias")).unwrap();
    std::os::unix::fs::symlink(&repo.parent, root.join("out")).unwrap();
    let alias = repo.parent.join("alias/src/lib.rs").display().to_string();
    assert_eq!(scoped(&[&alias, "out/outside.txt"]), (1, 1, 1));
}

/// A turn in a shared checkout reads only the files its own edits named,
/// stored relative to the repository root however the edits spelled them.
#[test]
fn a_scoped_turn_diff_reads_only_the_named_repository_relative_files() {
    let repo = repo();
    let root = &repo.root;
    captured(capture_checkpoint(root, START).unwrap());
    edit_everything(root);
    captured(capture_checkpoint(root, END).unwrap());

    write(&repo.parent, "outside.txt", "not the repository's\n");
    let outside = repo.parent.join("outside.txt").display().to_string();
    let notes = root.join("notes.txt").display().to_string();
    let named = [
        "src/lib.rs".to_owned(),
        notes,
        outside,
        "../outside.txt".to_owned(),
        "src/lib.rs".to_owned(),
        "./README.md".to_owned(),
    ];
    let relative = repo_relative_paths(root, &named).unwrap();
    assert_eq!(relative, ["README.md", "notes.txt", "src/lib.rs"]);
    // From a folder inside the repository, a relative path is that folder's.
    assert_eq!(
        repo_relative_paths(&root.join("src"), &["lib.rs".to_owned()]).unwrap(),
        ["src/lib.rs"]
    );

    let scoped = |paths: Option<Vec<String>>| {
        diff(
            root,
            &DiffRequest {
                scope: DiffScope::Turn,
                base: START.to_owned(),
                head: Some(END.to_owned()),
                paths,
            },
        )
        .unwrap()
    };
    let turn = scoped(Some(relative));
    let mut paths = turn
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert_eq!(paths, ["README.md", "notes.txt", "src/lib.rs"]);
    assert!(file(&turn, "src/lib.rs").patch.is_some());
    assert_eq!(
        scoped(None).files.len(),
        6,
        "unscoped is the whole checkout"
    );
    // Nothing usable named never widens to the whole tree.
    for paths in [
        Vec::new(),
        vec!["/etc/passwd".to_owned(), "../x".to_owned(), String::new()],
    ] {
        assert!(scoped(Some(paths)).files.is_empty());
    }
    // A literal path: no glob magic.
    write(root, "src/[x].rs", "x\n");
    assert!(scoped(Some(vec!["src/*.rs".to_owned()])).files.is_empty());
}

#[test]
fn diff_omits_large_patches_and_stops_at_the_total_budget() {
    let repo = repo();
    let root = &repo.root;
    captured(capture_checkpoint(root, START).unwrap());
    write(root, "a0-big.txt", "big line\n".repeat(400));
    for name in ["a1.txt", "a2.txt", "a3.txt"] {
        write(root, name, "small line\n".repeat(25));
    }
    let request = DiffRequest {
        scope: DiffScope::Uncommitted,
        base: START.to_owned(),
        head: None,
        paths: None,
    };
    let limits = DiffLimits {
        files: 100,
        file_patch_bytes: 1_000,
        total_patch_bytes: 1_000,
    };
    let bounded = diff_with(root, &request, limits).unwrap();
    assert!(bounded.truncated);
    let big = file(&bounded, "a0-big.txt");
    assert_eq!(big.omitted, Some(DiffOmitted::Large));
    assert!(big.patch.is_none());
    assert_eq!(big.added, 400);
    assert!(file(&bounded, "a1.txt").patch.is_some());
    assert!(file(&bounded, "a2.txt").patch.is_some());
    let cut = file(&bounded, "a3.txt");
    assert!(cut.patch.is_none());
    assert!(cut.omitted.is_none());
    let total = bounded
        .files
        .iter()
        .filter_map(|file| file.patch.as_ref())
        .map(String::len)
        .sum::<usize>();
    assert!(total <= 1_000);

    let few = diff_with(
        root,
        &request,
        DiffLimits {
            files: 2,
            ..DIFF_LIMITS
        },
    )
    .unwrap();
    assert!(few.truncated);
    assert_eq!(few.files.len(), 2);

    // The shipped per-file limit: 256 KiB.
    write(root, "huge.txt", "0123456789abcdef\n".repeat(20_000));
    let shipped = diff(root, &request).unwrap();
    assert_eq!(file(&shipped, "huge.txt").omitted, Some(DiffOmitted::Large));
    assert!(file(&shipped, "a1.txt").patch.is_some());
    assert!(!shipped.truncated);
    assert_eq!(DIFF_LIMITS.file_patch_bytes, 256 * 1024);
    assert_eq!(DIFF_LIMITS.total_patch_bytes, 2 * 1024 * 1024);
}

#[test]
fn restore_returns_to_a_checkpoint_and_keeps_a_recovery_ref() {
    let repo = repo();
    let root = &repo.root;
    captured(capture_checkpoint(root, START).unwrap());
    write(root, "src/lib.rs", "fn one() {}\nfn two() { changed }\n");
    write(root, "notes.txt", "draft\n");
    write(root, "scratch/inner.txt", "nested\n");
    write(root, "debug.log", "ignored\n");
    write(root, "build/out.txt", "ignored too\n");
    fs::remove_file(root.join("README.md")).unwrap();

    let restored = restore_to(root, START, RECOVERY).unwrap();

    assert_eq!(restored.recovery_ref, RECOVERY);
    assert_eq!(restored.restored, 4);
    assert_eq!(read(root, "src/lib.rs"), "fn one() {}\nfn two() {}\n");
    assert_eq!(read(root, "README.md"), "hello\n");
    assert!(!root.join("notes.txt").exists());
    assert!(!root.join("scratch").exists());
    assert_eq!(read(root, "debug.log"), "ignored\n");
    assert_eq!(read(root, "build/out.txt"), "ignored too\n");
    assert_eq!(status(root), "");

    // The recovery ref holds what was there, and restoring it brings it back.
    let back = restore_to(
        root,
        RECOVERY,
        "refs/interface/threads/0123456789abcdef/recovery/2",
    )
    .unwrap();
    assert_eq!(back.restored, 4);
    assert!(read(root, "src/lib.rs").contains("changed"));
    assert_eq!(read(root, "notes.txt"), "draft\n");
    assert_eq!(read(root, "scratch/inner.txt"), "nested\n");
    assert!(!root.join("README.md").exists());
    assert_eq!(read(root, "debug.log"), "ignored\n");
}

#[test]
fn restore_refuses_without_touching_files_when_the_recovery_capture_is_skipped() {
    let repo = repo();
    let root = &repo.root;
    captured(capture_checkpoint(root, START).unwrap());
    write(root, "src/lib.rs", "fn one() {}\nfn two() { changed }\n");
    write(root, "notes.txt", "draft\n");
    let status_before = status(root);
    let no_paths = CaptureLimits {
        max_paths: 0,
        ..CAPTURE_LIMITS
    };
    let error = restore_with(root, START, RECOVERY, no_paths).unwrap_err();
    assert_eq!(error.code, ThreadGitError::TOO_LARGE_CODE);
    assert!(
        error.message.contains("nothing was restored"),
        "{}",
        error.message
    );
    assert!(read(root, "src/lib.rs").contains("changed"));
    assert_eq!(read(root, "notes.txt"), "draft\n");
    assert_eq!(status(root), status_before);
    assert!(!git_ok(root, &["rev-parse", "--verify", RECOVERY]));

    let missing = restore_to(root, "refs/interface/threads/none/0/start", RECOVERY).unwrap_err();
    assert_eq!(missing.code, ThreadGitError::INVALID_CODE);
    assert!(read(root, "src/lib.rs").contains("changed"));
}

#[test]
fn worktree_is_created_named_recreated_after_removal_and_removed_keeping_its_branch() {
    let repo = repo();
    let root = &repo.root;
    let main = setup_git(root, &["rev-parse", "main"]);
    let prepared = prepare_workspace(&worktree_request(
        &root.join("src"),
        "Fix the flaky login test!",
    ))
    .unwrap();

    assert!(prepared.created);
    assert_eq!(prepared.repo_root.as_deref(), Some(root.as_path()));
    assert_eq!(prepared.base_ref.as_deref(), Some("main"));
    let path = prepared.worktree_path.clone().unwrap();
    assert_eq!(
        path.parent().unwrap(),
        repo.parent.join(".interface-worktrees")
    );
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    let suffix = name.strip_prefix("fix-the-flaky-login-test-").unwrap();
    assert_eq!(suffix.len(), 6);
    assert!(
        suffix
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    let branch = prepared.branch.clone().unwrap();
    assert_eq!(branch, format!("iface/{name}"));
    assert_eq!(prepared.cwd, path.join("src"));
    assert!(prepared.cwd.is_dir());
    assert_eq!(setup_git(&path, &["rev-parse", "HEAD"]), main);
    assert_eq!(
        setup_git(&path, &["symbolic-ref", "--short", "HEAD"]),
        branch
    );
    assert!(!git_ok(
        root,
        &["config", &format!("branch.{branch}.merge")]
    ));

    let listed = list_worktrees(root).unwrap();
    assert_eq!(listed[0], (root.clone(), Some("main".to_owned())));
    assert!(listed.contains(&(path.clone(), Some(branch.clone()))));

    // A checkpoint inside the worktree leaves its own index alone too.
    write(&path, "src/wip.rs", "wip\n");
    let worktree_index = PathBuf::from(setup_git(&path, &["rev-parse", "--git-path", "index"]));
    let index_before = fs::read(&worktree_index).unwrap();
    captured(capture_checkpoint(&path, START).unwrap());
    assert_eq!(fs::read(&worktree_index).unwrap(), index_before);
    fs::remove_file(path.join("src/wip.rs")).unwrap();

    assert!(!ensure_worktree(root, &path, &branch).unwrap());
    fs::remove_dir_all(&path).unwrap();
    assert!(ensure_worktree(root, &path, &branch).unwrap());
    assert_eq!(
        setup_git(&path, &["symbolic-ref", "--short", "HEAD"]),
        branch
    );
    assert_eq!(
        ensure_worktree(root, &repo.parent.join("elsewhere"), "iface/gone")
            .unwrap_err()
            .code,
        ThreadGitError::FAILED_CODE
    );
    let stranger = repo.parent.join("stranger");
    fs::create_dir(&stranger).unwrap();
    assert_eq!(
        ensure_worktree(root, &stranger, &branch).unwrap_err().code,
        ThreadGitError::INVALID_CODE
    );

    write(&path, "scratch.txt", "work in progress\n");
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::KeptDirty
    );
    assert!(path.join("scratch.txt").exists());
    fs::remove_file(path.join("scratch.txt")).unwrap();
    // Ignored files are not changes, but a release would delete them.
    write(&path, "debug.log", "noise\n");
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::KeptIgnored
    );
    fs::remove_file(path.join("debug.log")).unwrap();
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::Removed
    );
    assert!(!path.exists());
    assert!(git_ok(
        root,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
    ));
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::Missing
    );
    assert_eq!(
        remove_worktree(root, root).unwrap_err().code,
        ThreadGitError::INVALID_CODE
    );
}

/// 6 Oct 2026 (Threads review): a clean worktree's ignored files (its
/// dependencies, build output, a `.env`) went with it on release or discard.
/// Rebuildable or not, they keep it: the owner decides.
#[test]
fn ignored_files_keep_a_worktree_on_release_and_discard() {
    let repo = repo();
    let root = &repo.root;
    let prepared = prepare_workspace(&worktree_request(root, "ignored")).unwrap();
    let path = prepared.worktree_path.clone().unwrap();
    let branch = format!("refs/heads/{}", prepared.branch.as_ref().unwrap());
    assert!(!has_ignored_files(&path).unwrap());
    write(&path, "build/cache/out.o", "object\n");
    assert!(has_ignored_files(&path).unwrap());
    assert!(!is_dirty(&path).unwrap(), "ignored files are not changes");
    assert_eq!(
        discard_workspace(&prepared).unwrap(),
        RemoveOutcome::KeptIgnored
    );
    assert!(path.join("build/cache/out.o").exists());
    assert!(git_ok(root, &["rev-parse", "--verify", &branch]));
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::KeptIgnored
    );
    fs::remove_dir_all(path.join("build")).unwrap();
    assert_eq!(
        remove_worktree(root, &path).unwrap(),
        RemoveOutcome::Removed
    );
    assert!(!path.exists());
}

/// 6 Oct 2026 (Threads review): recreating a missing worktree ran a
/// repository-wide `git worktree prune`, forgetting every other worktree
/// whose folder was away (on an unplugged drive, say).
#[test]
fn recreating_a_missing_worktree_forgets_only_its_own_registration() {
    let repo = repo();
    let root = &repo.root;
    let made = |hint: &str| {
        let prepared = prepare_workspace(&worktree_request(root, hint)).unwrap();
        (prepared.worktree_path.unwrap(), prepared.branch.unwrap())
    };
    let (mine, my_branch) = made("mine");
    let (other, _) = made("other");
    let (locked, locked_branch) = made("locked");
    setup_git(root, &["worktree", "lock", locked.to_str().unwrap()]);
    for path in [&mine, &other, &locked] {
        fs::remove_dir_all(path).unwrap();
    }
    assert!(ensure_worktree(root, &mine, &my_branch).unwrap());
    assert!(mine.join("src/lib.rs").exists());
    let listed = setup_git(root, &["worktree", "list", "--porcelain"]);
    assert!(
        listed.contains(other.to_str().unwrap()),
        "another missing worktree stays registered: {listed}"
    );
    // A locked one is the owner's to unlock.
    let error = ensure_worktree(root, &locked, &locked_branch).unwrap_err();
    assert!(error.message.contains("locked"), "{}", error.message);
    assert!(!locked.exists());
}

/// 6 Oct 2026 (Threads review): a `git worktree add` that ran out of time
/// returned early and left its folder, registration and new branch behind.
#[test]
fn a_worktree_add_that_runs_out_of_time_leaves_nothing_behind() {
    use std::os::unix::fs::PermissionsExt;

    let repo = repo();
    let root = &repo.root;
    let hooks = repo.parent.join("hooks");
    let slow_hook = || {
        write(&hooks, "post-checkout", "#!/bin/sh\nsleep 30\n");
        fs::set_permissions(
            hooks.join("post-checkout"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    };
    // Set in the repository, so a global hooks path cannot hide it.
    setup_git(root, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    slow_hook();
    let short = Call::checkout().within(Duration::from_millis(1500));
    let error = create_thread_worktree(&worktree_request(root, "slow"), short).unwrap_err();
    assert_eq!(
        error.code,
        ThreadGitError::TIMEOUT_CODE,
        "{}",
        error.message
    );
    let parent = repo.parent.join(".interface-worktrees");
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 0, "no folder");
    assert_eq!(worktree_records(root).unwrap().len(), 1, "no registration");
    assert_eq!(
        setup_git(
            root,
            &["for-each-ref", "--format=%(refname)", "refs/heads/iface/"]
        ),
        "",
        "no branch"
    );

    // Recreating a missing worktree that runs out of time removes what it
    // made and keeps the branch, which holds the thread's commits.
    fs::remove_file(hooks.join("post-checkout")).unwrap();
    let prepared = prepare_workspace(&worktree_request(root, "recreate")).unwrap();
    let path = prepared.worktree_path.unwrap();
    let branch = prepared.branch.unwrap();
    fs::remove_dir_all(&path).unwrap();
    slow_hook();
    let error = ensure_worktree_with(root, &path, &branch, short).unwrap_err();
    assert_eq!(
        error.code,
        ThreadGitError::TIMEOUT_CODE,
        "{}",
        error.message
    );
    assert!(!path.exists());
    assert!(find_missing_worktree(&worktree_records(root).unwrap(), &path).is_none());
    assert!(git_ok(
        root,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
    ));
    fs::remove_file(hooks.join("post-checkout")).unwrap();
    assert!(ensure_worktree(root, &path, &branch).unwrap());
}

#[test]
fn locked_worktrees_are_in_use() {
    let repo = repo();
    let root = &repo.root;
    let prepared = prepare_workspace(&worktree_request(root, "locked")).unwrap();
    let path = prepared.worktree_path.unwrap();
    setup_git(root, &["worktree", "lock", path.to_str().unwrap()]);
    assert_eq!(remove_worktree(root, &path).unwrap(), RemoveOutcome::InUse);
    assert!(path.exists());
}

#[test]
fn discard_removes_only_a_clean_new_worktree_and_a_branch_without_commits() {
    let repo = repo();
    let root = &repo.root;

    let clean = prepare_workspace(&worktree_request(root, "clean")).unwrap();
    assert_eq!(discard_workspace(&clean).unwrap(), RemoveOutcome::Removed);
    assert!(!clean.worktree_path.as_ref().unwrap().exists());
    let clean_branch = format!("refs/heads/{}", clean.branch.as_ref().unwrap());
    assert!(!git_ok(root, &["rev-parse", "--verify", &clean_branch]));

    let committed = prepare_workspace(&worktree_request(root, "committed")).unwrap();
    let committed_path = committed.worktree_path.clone().unwrap();
    write(&committed_path, "feature.txt", "done\n");
    setup_git(&committed_path, &["add", "feature.txt"]);
    setup_git(&committed_path, &["commit", "-q", "-m", "feature"]);
    assert_eq!(
        discard_workspace(&committed).unwrap(),
        RemoveOutcome::Removed
    );
    let committed_branch = format!("refs/heads/{}", committed.branch.as_ref().unwrap());
    assert!(git_ok(root, &["rev-parse", "--verify", &committed_branch]));

    let dirty = prepare_workspace(&worktree_request(root, "dirty")).unwrap();
    let dirty_path = dirty.worktree_path.clone().unwrap();
    write(&dirty_path, "unsaved.txt", "keep me\n");
    assert_eq!(discard_workspace(&dirty).unwrap(), RemoveOutcome::KeptDirty);
    assert!(dirty_path.join("unsaved.txt").exists());
    let dirty_branch = format!("refs/heads/{}", dirty.branch.as_ref().unwrap());
    assert!(git_ok(root, &["rev-parse", "--verify", &dirty_branch]));

    let local = prepare_workspace(&GitWorkspaceRequest {
        project_root: root.clone(),
        mode: PrepareMode::Local,
        base_ref: None,
        worktree_path: None,
        hint: String::new(),
    })
    .unwrap();
    assert_eq!(discard_workspace(&local).unwrap(), RemoveOutcome::Missing);
    assert!(root.join("src/lib.rs").exists());
}

fn request(
    project_root: &Path,
    mode: PrepareMode,
    base_ref: Option<&str>,
    worktree_path: Option<&Path>,
) -> GitWorkspaceRequest {
    GitWorkspaceRequest {
        project_root: project_root.to_path_buf(),
        mode,
        base_ref: base_ref.map(ToOwned::to_owned),
        worktree_path: worktree_path.map(Path::to_path_buf),
        hint: "Hint".to_owned(),
    }
}

fn refused(request: &GitWorkspaceRequest) -> &'static str {
    prepare_workspace(request).unwrap_err().code
}

#[test]
fn prepare_validates_local_and_worktree_requests() {
    let repo = repo();
    let root = &repo.root;
    let local =
        prepare_workspace(&request(&root.join("src"), PrepareMode::Local, None, None)).unwrap();
    assert_eq!(local.cwd, root.join("src"));
    assert_eq!(local.repo_root.as_deref(), Some(root.as_path()));
    assert!(!local.created);
    assert!(local.worktree_path.is_none());

    let plain = tempfile::tempdir().unwrap();
    let folder = prepare_workspace(&request(plain.path(), PrepareMode::Local, None, None)).unwrap();
    assert_eq!(folder.repo_root, None);
    assert_eq!(folder.cwd, plain.path());
    let missing = root.join("missing");
    for (request, code) in [
        (
            request(plain.path(), PrepareMode::Worktree, None, None),
            ThreadGitError::NOT_REPOSITORY_CODE,
        ),
        (
            request(&missing, PrepareMode::Local, None, None),
            ThreadGitError::INVALID_CODE,
        ),
        (
            request(root, PrepareMode::Worktree, Some("no-such-branch"), None),
            ThreadGitError::INVALID_CODE,
        ),
        (
            request(root, PrepareMode::Worktree, Some("--orphan"), None),
            ThreadGitError::INVALID_CODE,
        ),
    ] {
        assert_eq!(refused(&request), code, "{request:?}");
    }

    setup_git(root, &["branch", "release"]);
    let from_release =
        prepare_workspace(&request(root, PrepareMode::Worktree, Some("release"), None)).unwrap();
    assert_eq!(from_release.base_ref.as_deref(), Some("release"));
    assert_eq!(
        from_release.cwd,
        from_release.worktree_path.clone().unwrap()
    );

    let unborn = tempfile::tempdir().unwrap();
    setup_git(unborn.path(), &["init", "-q"]);
    let error =
        prepare_workspace(&request(unborn.path(), PrepareMode::Worktree, None, None)).unwrap_err();
    assert!(
        error.message.contains("no commits yet"),
        "{}",
        error.message
    );
}

#[test]
fn prepare_existing_requires_a_worktree_of_the_same_repository() {
    let elsewhere = repo();
    let repo = repo();
    let root = &repo.root;
    let other = repo.parent.join("other");
    setup_git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            other.to_str().unwrap(),
        ],
    );
    let existing =
        prepare_workspace(&request(root, PrepareMode::Existing, None, Some(&other))).unwrap();
    assert!(!existing.created);
    assert_eq!(existing.cwd, other);
    assert_eq!(existing.worktree_path.as_deref(), Some(other.as_path()));
    assert_eq!(existing.branch.as_deref(), Some("other"));
    assert_eq!(
        discard_workspace(&existing).unwrap(),
        RemoveOutcome::Missing
    );
    assert!(other.exists());

    let plain = tempfile::tempdir().unwrap();
    for worktree in [Some(plain.path()), Some(elsewhere.root.as_path()), None] {
        assert_eq!(
            refused(&request(root, PrepareMode::Existing, None, worktree)),
            ThreadGitError::INVALID_CODE,
            "{worktree:?}"
        );
    }
}

#[test]
fn branches_list_current_and_origin_default() {
    let repo = repo();
    let root = &repo.root;
    setup_git(root, &["branch", "feature"]);
    setup_git(root, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
    setup_git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    let branches = list_branches(root).unwrap();
    assert_eq!(branches.current.as_deref(), Some("main"));
    assert_eq!(branches.default_branch.as_deref(), Some("trunk"));
    let mut names = branches.branches.clone();
    names.sort();
    assert_eq!(names, ["feature", "main"]);

    setup_git(root, &["checkout", "-q", "--detach"]);
    let detached = list_branches(root).unwrap();
    assert_eq!(detached.current, None);

    let plain = tempfile::tempdir().unwrap();
    assert_eq!(list_branches(plain.path()).unwrap(), BranchList::default());
    assert!(list_worktrees(plain.path()).unwrap().is_empty());
    assert_eq!(repo_root(plain.path()).unwrap(), None);
    assert_eq!(
        repo_root(&root.join("src")).unwrap().as_deref(),
        Some(root.as_path())
    );
    assert_eq!(
        repo_root(Path::new("/nonexistent/interface-thread-git")),
        Ok(None)
    );
}

#[test]
fn merge_base_dirty_state_and_ref_deletion() {
    let repo = repo();
    let root = &repo.root;
    let head = setup_git(root, &["rev-parse", "HEAD"]);
    setup_git(root, &["branch", "base"]);
    write(root, "next.txt", "next\n");
    setup_git(root, &["add", "next.txt"]);
    setup_git(root, &["commit", "-q", "-m", "next"]);
    assert_eq!(merge_base(root, "base").unwrap(), Some(head));
    let lonely = setup_git(root, &["commit-tree", "HEAD^{tree}", "-m", "lonely"]);
    setup_git(root, &["update-ref", "refs/heads/lonely", &lonely]);
    assert_eq!(merge_base(root, "lonely").unwrap(), None);
    assert_eq!(
        merge_base(root, "no-such-ref").unwrap_err().code,
        ThreadGitError::FAILED_CODE
    );

    assert!(!is_dirty(root).unwrap());
    write(root, "debug.log", "ignored\n");
    assert!(!is_dirty(root).unwrap());
    write(root, "new.txt", "new\n");
    assert!(is_dirty(&root.join("src")).unwrap());
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(
        is_dirty(plain.path()).unwrap_err().code,
        ThreadGitError::NOT_REPOSITORY_CODE
    );

    captured(capture_checkpoint(root, "refs/interface/threads/aaa/0/start").unwrap());
    captured(capture_checkpoint(root, "refs/interface/threads/aaa/0/end").unwrap());
    captured(capture_checkpoint(root, "refs/interface/threads/aaab/0/start").unwrap());
    assert_eq!(delete_refs(root, "refs/interface/threads/aaa").unwrap(), 2);
    assert_eq!(interface_refs(root), "refs/interface/threads/aaab/0/start");
    assert_eq!(delete_refs(root, "refs/interface/threads/aaa/").unwrap(), 0);
    assert_eq!(
        delete_refs(plain.path(), "refs/interface/threads/aaab").unwrap(),
        0
    );
}

#[test]
fn move_ref_files_a_pending_baseline_under_its_thread_in_one_step() {
    let repo = repo();
    let root = &repo.root;
    let pending = "refs/interface/threads/pending/prep-0123456789abcdef";
    write(root, "draft.txt", "before the launch\n");
    let before = status(root);
    let commit = captured(capture_checkpoint(root, pending).unwrap());

    move_ref(&root.join("src"), pending, START).unwrap();
    assert_eq!(interface_refs(root), START, "the pending name is gone");
    assert_eq!(setup_git(root, &["rev-parse", START]), commit);
    assert_eq!(status(root), before, "the working tree is untouched");
    // A discard after the bind finds no pending baseline to delete.
    assert_eq!(delete_refs(root, pending).unwrap(), 0);
    assert_eq!(interface_refs(root), START);

    // A missing source, a ref outside refs/interface/ and a self-move all
    // refuse without touching any ref.
    assert_eq!(
        move_ref(root, pending, END).unwrap_err().code,
        ThreadGitError::INVALID_CODE
    );
    for (from, to) in [
        (START, "refs/heads/stolen"),
        ("refs/heads/main", END),
        (START, START),
    ] {
        assert_eq!(
            move_ref(root, from, to).unwrap_err().code,
            ThreadGitError::INVALID_CODE,
            "{from} -> {to}"
        );
    }
    assert!(!git_ok(
        root,
        &["rev-parse", "--verify", "refs/heads/stolen"]
    ));
    assert_eq!(interface_refs(root), START);
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(
        move_ref(plain.path(), START, END).unwrap_err().code,
        ThreadGitError::NOT_REPOSITORY_CODE
    );
}

#[test]
fn slugs_are_short_lowercase_ascii_words() {
    assert_eq!(
        thread_slug("Fix the flaky login test"),
        "fix-the-flaky-login-test"
    );
    assert_eq!(
        thread_slug("  --Add OAuth2 (PKCE) support!! "),
        "add-oauth2-pkce-support"
    );
    assert_eq!(thread_slug(""), "thread");
    assert_eq!(thread_slug("¿¡…!"), "thread");
    assert_eq!(thread_slug("Ünïcode café"), "n-code-caf");
    assert_eq!(
        thread_slug("one two three four five six seven eight"),
        "one-two-three-four-five-six"
    );
    assert_eq!(thread_slug(&"x".repeat(40)), "x".repeat(32));
    let suffix = random_hex6();
    assert_eq!(suffix.len(), 6);
    assert!(
        suffix
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
    );
}

#[test]
fn raw_and_numstat_records_pair_up() {
    let output = b":100644 100644 aaa bbb M\0src/a.rs\0\
:100644 100644 ccc ccc R100\0old name.txt\0new name.txt\0\
:000000 100644 000 ddd A\0img.png\0\
:100644 000000 eee 000 D\0gone.txt\0\
3\t1\tsrc/a.rs\0\
0\t0\t\0old name.txt\0new name.txt\0\
-\t-\timg.png\0\
0\t7\tgone.txt\0";
    let changes = parse_changes(output);
    assert_eq!(changes.len(), 4);
    assert_eq!(changes[0].status, DiffFileStatus::Modified);
    assert_eq!((changes[0].added, changes[0].removed), (3, 1));
    assert_eq!(changes[1].status, DiffFileStatus::Renamed);
    assert_eq!(changes[1].path, "new name.txt");
    assert_eq!(changes[1].old_path.as_deref(), Some("old name.txt"));
    assert_eq!(changes[2].status, DiffFileStatus::Added);
    assert!(changes[2].binary);
    assert_eq!(changes[3].status, DiffFileStatus::Deleted);
    assert_eq!(changes[3].removed, 7);
}

/// A project file iCloud has evicted blocks the first read until it is
/// downloaded. The one plain file read in Threads' git — the real index the
/// checkpoint seeds from — goes through the bounded project read: a FIFO in
/// its place (opening it blocks forever, as an evicted file waits) fails at
/// the copy's deadline, saying the index is still downloading, and nothing
/// waits for it.
#[test]
fn an_index_still_downloading_from_icloud_fails_clearly_at_its_deadline() {
    let repo = repo();
    let root = &repo.root;
    let index = root.join(".git/index");
    fs::remove_file(&index).unwrap();
    let made = Command::new("mkfifo").arg(&index).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    let scratch = tempfile::tempdir().unwrap();

    let started = std::time::Instant::now();
    let refused = super::copy_real_index(
        root,
        &scratch.path().join("index"),
        std::time::Instant::now() + Duration::from_secs(60),
    )
    .expect_err("an index that never arrives is not copied");
    assert_eq!(refused.code, ThreadGitError::DOWNLOADING_CODE);
    assert!(
        refused.message.contains("still downloading from iCloud"),
        "{}",
        refused.message
    );
    assert!(
        started.elapsed() < super::INDEX_COPY_LIMIT + Duration::from_secs(3),
        "the copy waited {:?}",
        started.elapsed()
    );
    // Let the abandoned reader go.
    let _ = fs::OpenOptions::new().write(true).open(&index);
}
