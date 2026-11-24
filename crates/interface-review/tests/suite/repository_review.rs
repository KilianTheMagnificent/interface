use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::temp_dir;
use interface_contracts::ipc::{
    RepositoryReviewErrorCode, RepositoryReviewOmission, RepositoryReviewState,
};
use interface_review::{
    REVIEW_GIT_TIMED_OUT, REVIEW_GIT_UNAVAILABLE, RepositoryReviewHooks, RepositoryReviewLimits,
    RepositoryReviewOptions, ReviewGitResult, ReviewGitRunner, read_repository_review,
    read_repository_review_with,
};

#[derive(Default)]
struct FakeGit {
    results: Mutex<VecDeque<ReviewGitResult>>,
    calls: Mutex<Vec<Vec<String>>>,
}

struct RootSwappingGit {
    backup: std::path::PathBuf,
    swapped: AtomicBool,
}

struct ParentSwappingHook {
    backup: std::path::PathBuf,
    swapped: AtomicBool,
}

impl RepositoryReviewHooks for ParentSwappingHook {
    fn after_untracked_parent_validated(&self, parent: &Path, file: &Path) {
        if !self.swapped.swap(true, Ordering::SeqCst) {
            fs::rename(parent, &self.backup).expect("move pinned parent");
            fs::create_dir(parent).expect("replacement parent");
            fs::write(
                parent.join(file.file_name().expect("file name")),
                "replacement bytes that must not be returned",
            )
            .expect("replacement file");
        }
    }
}

impl ReviewGitRunner for RootSwappingGit {
    fn run(&self, _args: &[String], cwd: &Path, _maximum: usize) -> ReviewGitResult {
        if !self.swapped.swap(true, Ordering::SeqCst) {
            fs::rename(cwd, &self.backup).expect("move authorized root");
            fs::create_dir(cwd).expect("replacement root");
        }
        result(0, "true\noutput that must be discarded")
    }
}

impl FakeGit {
    fn with(results: impl IntoIterator<Item = ReviewGitResult>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl ReviewGitRunner for FakeGit {
    fn run(&self, args: &[String], _cwd: &Path, _maximum: usize) -> ReviewGitResult {
        self.calls.lock().expect("calls lock").push(args.to_vec());
        self.results
            .lock()
            .expect("result lock")
            .pop_front()
            .expect("a fake result for every call")
    }
}

fn result(code: i32, stdout: &str) -> ReviewGitResult {
    ReviewGitResult {
        code,
        stdout: stdout.to_owned(),
        ..ReviewGitResult::default()
    }
}

fn base_results(status: &str) -> Vec<ReviewGitResult> {
    vec![
        result(0, "true\n"),
        result(0, "main\n"),
        result(0, "0123456789abcdef\n"),
        result(0, status),
    ]
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be available for the real-repository test");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn review_says_git_timed_out_and_names_icloud_instead_of_calling_git_missing() {
    let directory = temp_dir();
    let review_with = |runner: &FakeGit| {
        read_repository_review_with(
            directory.path(),
            &RepositoryReviewOptions {
                runner: Some(runner),
                ..RepositoryReviewOptions::default()
            },
        )
    };

    // The first read (is this a work tree?) ran out of time.
    let review = review_with(&FakeGit::with([result(REVIEW_GIT_TIMED_OUT, "")]));
    let error = review.error.expect("a timed-out read is an error");
    assert_eq!(error.code, RepositoryReviewErrorCode::GitError);
    assert!(
        error.message.contains("within 15 seconds"),
        "{}",
        error.message
    );
    assert!(error.message.contains("iCloud"), "{}", error.message);
    assert!(
        !error.message.contains("not available"),
        "{}",
        error.message
    );

    // Status ran out of time inside a repository: still a repository.
    let mut results = base_results("");
    results[3] = result(REVIEW_GIT_TIMED_OUT, "");
    let review = review_with(&FakeGit::with(results));
    assert!(review.repository);
    let error = review.error.expect("a timed-out status is an error");
    assert_eq!(error.code, RepositoryReviewErrorCode::GitError);
    assert!(error.message.contains("iCloud"), "{}", error.message);

    // A git that could not start at all still says so.
    let review = review_with(&FakeGit::with([result(REVIEW_GIT_UNAVAILABLE, "")]));
    let error = review.error.expect("missing git is an error");
    assert_eq!(error.code, RepositoryReviewErrorCode::GitUnavailable);
    assert_eq!(error.message, "Git is not available on this machine.");
}

#[test]
fn real_git_preserves_staged_and_unstaged_layers_without_rewriting_files() {
    let directory = temp_dir();
    let root = directory.path();
    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.email", "review@example.invalid"]);
    git(root, &["config", "user.name", "Review Test"]);
    let file = root.join("file name.txt");
    fs::write(&file, "one\n").expect("baseline file");
    git(root, &["add", "--", "file name.txt"]);
    git(root, &["commit", "--quiet", "-m", "baseline"]);
    fs::write(&file, "one\ntwo\n").expect("staged file");
    git(root, &["add", "--", "file name.txt"]);
    fs::write(&file, "one\ntwo\nthree\n").expect("unstaged file");
    let before = fs::metadata(&file).expect("metadata before");
    let before_content = fs::read(&file).expect("content before");

    let review = read_repository_review(root);

    assert!(review.repository);
    assert_eq!(review.clean, Some(false));
    assert_eq!(review.files.len(), 1);
    let changed = &review.files[0];
    assert_eq!(changed.path, "file name.txt");
    assert_eq!(
        changed.states,
        vec![
            RepositoryReviewState::Staged,
            RepositoryReviewState::Unstaged
        ]
    );
    assert!(changed.patch.contains("Interface layer: staged\n"));
    assert!(changed.patch.contains("Interface layer: unstaged\n"));
    assert_eq!(changed.additions, Some(2));
    assert_eq!(changed.deletions, Some(0));
    assert_eq!(fs::read(&file).expect("content after"), before_content);
    let after = fs::metadata(&file).expect("metadata after");
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().ok(), before.modified().ok());
}

#[test]
fn fake_runner_receives_literal_pathspecs_and_safe_diff_flags() {
    let directory = temp_dir();
    let mut results = base_results(" M src/file name.txt\0");
    results.push(result(0, "1\t0\tsrc/file name.txt\0"));
    results.push(result(
        0,
        "diff --git a/src/file name.txt b/src/file name.txt\n",
    ));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 1);
    let calls = runner.calls();
    assert_eq!(calls.len(), 6);
    for call in &calls[4..] {
        assert!(call.iter().any(|arg| arg == "--no-ext-diff"));
        assert!(call.iter().any(|arg| arg == "--no-textconv"));
        assert!(call.iter().any(|arg| arg == ":(literal)src/file name.txt"));
        let marker = call
            .iter()
            .position(|arg| arg == "--")
            .expect("path marker");
        assert!(
            call[marker + 1..]
                .iter()
                .all(|arg| arg.starts_with(":(literal)"))
        );
    }
}

#[test]
fn credential_and_posix_backslash_paths_never_reach_diff() {
    let directory = temp_dir();
    let runner = FakeGit::with(base_results(" M .env.production\0 M src\\App.tsx\0"));
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(runner.calls().len(), 4);
    assert_eq!(review.files.len(), 2);
    assert_eq!(
        review.files[0].omitted,
        Some(RepositoryReviewOmission::Credential)
    );
    assert_eq!(
        review.files[1].omitted,
        Some(RepositoryReviewOmission::UnsafePath)
    );
}

#[test]
fn known_and_numstat_binary_files_never_reach_patch_diff() {
    let directory = temp_dir();
    let mut results = base_results(" M asset.png\0 M extensionless\0");
    results.push(result(0, "-\t-\textensionless\0"));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(runner.calls().len(), 5);
    assert_eq!(review.files.len(), 2);
    assert!(review.files.iter().all(|file| {
        file.binary
            && file.patch.is_empty()
            && file.omitted == Some(RepositoryReviewOmission::Binary)
    }));
}

#[cfg(unix)]
#[test]
fn untracked_symlinks_and_hardlinks_are_refused_without_returning_bytes() {
    use std::os::unix::fs::symlink;

    let directory = temp_dir();
    let root = directory.path().join("repo");
    fs::create_dir(&root).expect("repository root");
    git(&root, &["init", "--quiet"]);
    let outside = directory.path().join("outside.txt");
    fs::write(&outside, "outside marker").expect("outside fixture");
    symlink(&outside, root.join("linked.txt")).expect("symlink fixture");
    fs::hard_link(&outside, root.join("aliased.txt")).expect("hardlink fixture");

    let review = read_repository_review(&root);

    let linked = review
        .files
        .iter()
        .find(|file| file.path == "linked.txt")
        .expect("symlink result");
    assert_eq!(linked.omitted, Some(RepositoryReviewOmission::Symlink));
    assert!(linked.patch.is_empty());
    let aliased = review
        .files
        .iter()
        .find(|file| file.path == "aliased.txt")
        .expect("hardlink result");
    assert_eq!(aliased.omitted, Some(RepositoryReviewOmission::UnsafePath));
    assert!(aliased.patch.is_empty());
}

#[test]
fn status_and_patch_bounds_are_explicit() {
    let directory = temp_dir();
    fs::write(directory.path().join("one.txt"), "one\n").expect("untracked fixture");
    let mut status = result(0, "?? one.txt\0?? incomplete.txt");
    status.truncated = true;
    let runner = FakeGit::with(vec![
        result(0, "true\n"),
        result(0, "main\n"),
        result(0, "head\n"),
        status,
    ]);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        limits: RepositoryReviewLimits {
            max_patch_bytes_per_file: 24,
            ..RepositoryReviewLimits::default()
        },
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 1);
    assert_eq!(review.files[0].path, "one.txt");
    assert!(review.files[0].patch_truncated);
    assert_eq!(review.clean, None);
    assert!(review.truncated);
    assert_eq!(
        review.error.as_ref().map(|error| error.code),
        Some(RepositoryReviewErrorCode::OutputLimit)
    );
}

#[test]
fn root_identity_swap_discards_runner_output() {
    let directory = temp_dir();
    let root = directory.path().to_path_buf();
    let backup = root.with_extension("authorized-backup");
    let runner = RootSwappingGit {
        backup: backup.clone(),
        swapped: AtomicBool::new(false),
    };
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(&root, &options);

    assert!(!review.repository);
    assert!(review.files.is_empty());
    assert_eq!(
        review.error.as_ref().map(|error| error.code),
        Some(RepositoryReviewErrorCode::GitError)
    );
    assert!(
        !serde_json::to_string(&review)
            .expect("wire JSON")
            .contains("output that must be discarded")
    );

    fs::remove_dir(&root).expect("remove replacement root");
    fs::rename(&backup, &root).expect("restore fixture root");
}

#[test]
fn untracked_parent_swap_is_refused_before_replacement_bytes_are_read() {
    let directory = temp_dir();
    let root = directory.path();
    let parent = root.join("sub");
    fs::create_dir(&parent).expect("original parent");
    fs::write(parent.join("file.txt"), "original bytes").expect("original file");
    let backup = root.join("sub-authorized-backup");
    let hooks = ParentSwappingHook {
        backup: backup.clone(),
        swapped: AtomicBool::new(false),
    };
    let runner = FakeGit::with(base_results("?? sub/file.txt\0"));
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        hooks: Some(&hooks),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(root, &options);

    assert_eq!(review.files.len(), 1);
    assert_eq!(
        review.files[0].omitted,
        Some(RepositoryReviewOmission::UnsafePath)
    );
    assert!(review.files[0].patch.is_empty());
    assert!(
        !serde_json::to_string(&review)
            .expect("wire JSON")
            .contains("replacement bytes")
    );

    fs::remove_dir_all(&parent).expect("remove replacement parent");
    fs::rename(&backup, &parent).expect("restore original parent");
}

#[test]
fn credential_omission_serializes_with_exact_contract_field_names() {
    let directory = temp_dir();
    let runner = FakeGit::with(base_results(" M .env.local\0"));
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);
    let wire = serde_json::to_value(&review).expect("wire JSON");

    assert_eq!(wire["metadataComplete"], true);
    assert!(wire.get("metadata_complete").is_none());
    assert_eq!(wire["files"][0]["path"], ".env.local");
    assert_eq!(wire["files"][0]["states"][0], "unstaged");
    assert_eq!(wire["files"][0]["omitted"], "credential");
    assert_eq!(wire["files"][0]["patchTruncated"], false);
    assert!(wire["files"][0]["additions"].is_null());
}

#[test]
fn skip_patches_skips_tracked_git_and_keeps_output_limit_omission() {
    let directory = temp_dir();
    let runner = FakeGit::with(base_results(" M a.txt\0 M b.txt\0"));
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        skip_patches: true,
        limits: RepositoryReviewLimits {
            max_patch_files: 1,
            ..RepositoryReviewLimits::default()
        },
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(runner.calls().len(), 4);
    assert_eq!(review.files.len(), 2);
    assert!(review.files[0].patch.is_empty());
    assert_eq!(review.files[0].additions, None);
    assert_eq!(review.files[0].omitted, None);
    assert_eq!(
        review.files[1].omitted,
        Some(RepositoryReviewOmission::OutputLimit)
    );
}

#[test]
fn batched_numstat_uses_one_spawn_per_layer() {
    let directory = temp_dir();
    let mut results = base_results(" M a.txt\0 M b.txt\0");
    results.push(result(0, concat!("1\t0\ta.txt\0", "1\t0\tb.txt\0")));
    // git orders sections by path, not by argv; attribution is by header.
    results.push(result(
        0,
        "diff --git a/b.txt b/b.txt\n+b\ndiff --git a/a.txt b/a.txt\n+a\n",
    ));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 2);
    assert_eq!(review.files[0].additions, Some(1));
    assert_eq!(review.files[1].additions, Some(1));
    assert_eq!(
        review.files[0].patch,
        "Interface layer: unstaged\ndiff --git a/a.txt b/a.txt\n+a\n"
    );
    assert_eq!(
        review.files[1].patch,
        "Interface layer: unstaged\ndiff --git a/b.txt b/b.txt\n+b\n"
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 6);
    let numstat = &calls[4];
    assert!(numstat.iter().any(|arg| arg == "--numstat"));
    assert!(numstat.iter().any(|arg| arg == ":(literal)a.txt"));
    assert!(numstat.iter().any(|arg| arg == ":(literal)b.txt"));
    let diff = &calls[5];
    assert!(!diff.iter().any(|arg| arg == "--numstat"));
    assert!(diff.iter().any(|arg| arg == "--no-renames"));
    assert!(diff.iter().any(|arg| arg == ":(literal)a.txt"));
    assert!(diff.iter().any(|arg| arg == ":(literal)b.txt"));
}

#[test]
fn failed_batched_numstat_falls_back_per_file() {
    let directory = temp_dir();
    let mut results = base_results(" M a.txt\0 M b.txt\0");
    results.push(result(128, ""));
    results.push(result(
        0,
        "diff --git a/a.txt b/a.txt\ndiff --git a/b.txt b/b.txt\n",
    ));
    results.push(result(0, "1\t0\ta.txt\0"));
    results.push(result(0, "1\t0\tb.txt\0"));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 2);
    assert_eq!(review.files[0].additions, Some(1));
    assert_eq!(review.files[1].additions, Some(1));
    assert!(review.error.is_none());
    assert_eq!(runner.calls().len(), 8);
}

#[test]
fn renames_keep_their_own_numstat_while_the_batch_counts_a_delete_and_add_apart() {
    let directory = temp_dir();
    let mut results = base_results("R  new.txt\0old.txt\0D  gone.txt\0A  fresh.txt\0");
    // `--no-renames` makes the batch report the pair as what the status says it is.
    results.push(result(0, concat!("0\t3\tgone.txt\0", "2\t0\tfresh.txt\0")));
    results.push(result(
        0,
        "diff --git a/fresh.txt b/fresh.txt\ndiff --git a/gone.txt b/gone.txt\n",
    ));
    results.push(result(0, "2\t1\t\0old.txt\0new.txt\0"));
    results.push(result(0, "diff --git a/old.txt b/new.txt\n"));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 3);
    // The rename keeps rename detection, so it reports the edit, not a whole rewrite.
    assert_eq!(review.files[0].path, "new.txt");
    assert_eq!(review.files[0].additions, Some(2));
    assert_eq!(review.files[0].deletions, Some(1));
    // Both halves of the delete+add pair keep their own real counts.
    assert_eq!(review.files[1].path, "gone.txt");
    assert_eq!(review.files[1].additions, Some(0));
    assert_eq!(review.files[1].deletions, Some(3));
    assert_eq!(review.files[2].path, "fresh.txt");
    assert_eq!(review.files[2].additions, Some(2));
    assert_eq!(review.files[2].deletions, Some(0));
    assert!(review.error.is_none());

    let calls = runner.calls();
    assert_eq!(calls.len(), 8);
    let batched = &calls[4];
    assert!(batched.iter().any(|arg| arg == "--no-renames"));
    assert!(batched.iter().any(|arg| arg == ":(literal)gone.txt"));
    assert!(batched.iter().any(|arg| arg == ":(literal)fresh.txt"));
    assert!(!batched.iter().any(|arg| arg == ":(literal)new.txt"));
    assert!(!batched.iter().any(|arg| arg == ":(literal)old.txt"));
    // The patch batch leaves the rename out too.
    let batched_diff = &calls[5];
    assert!(!batched_diff.iter().any(|arg| arg == "--numstat"));
    assert!(batched_diff.iter().any(|arg| arg == ":(literal)gone.txt"));
    assert!(!batched_diff.iter().any(|arg| arg == ":(literal)new.txt"));
    // The per-file argv is unchanged: rename detection stays on for a rename.
    for per_file in &calls[6..] {
        assert!(!per_file.iter().any(|arg| arg == "--no-renames"));
        assert!(per_file.iter().any(|arg| arg == ":(literal)new.txt"));
        assert!(per_file.iter().any(|arg| arg == ":(literal)old.txt"));
    }
    assert!(calls[6].iter().any(|arg| arg == "--numstat"));
}
#[test]
fn truncated_batched_numstat_falls_back_per_file() {
    let directory = temp_dir();
    let mut results = base_results(" M a.txt\0 M b.txt\0");
    let mut truncated = result(0, "1\t0\ta.txt\0");
    truncated.truncated = true;
    results.push(truncated);
    results.push(result(
        0,
        "diff --git a/a.txt b/a.txt\ndiff --git a/b.txt b/b.txt\n",
    ));
    results.push(result(0, "1\t0\ta.txt\0"));
    results.push(result(0, "1\t0\tb.txt\0"));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(review.files.len(), 2);
    assert_eq!(review.files[0].additions, Some(1));
    assert_eq!(review.files[1].additions, Some(1));
    let calls = runner.calls();
    assert_eq!(calls.len(), 8);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.iter().any(|arg| arg == "--numstat"))
            .count(),
        3
    );
}

#[test]
fn an_unusable_diff_batch_falls_back_to_exact_per_file_diffs() {
    let with_stderr = ReviewGitResult {
        stderr: "warning: in the working copy of 'a.txt', LF will be replaced by CRLF".into(),
        ..result(0, "diff --git a/a.txt b/a.txt\n+a\n")
    };
    let mut cut = result(
        0,
        "diff --git a/a.txt b/a.txt\n+a\ndiff --git a/b.txt b/b.txt\n+b\ndiff --git a/c.t",
    );
    cut.truncated = true;
    let cases = [
        // (batch result, files that still need their own diff call)
        (result(128, ""), vec!["a", "b", "c"]),
        (with_stderr, vec!["a", "b", "c"]),
        (
            result(0, "diff --git a/other.txt b/other.txt\n"),
            vec!["a", "b", "c"],
        ),
        (
            result(
                0,
                "diff --git a/a.txt b/a.txt\ndiff --git a/b.txt b/b.txt\ndiff --git a/a.txt b/a.txt\n",
            ),
            vec!["a", "b", "c"],
        ),
        (
            result(0, "Submodule sub 1234567..89abcde:\n"),
            vec!["a", "b", "c"],
        ),
        // Only sections followed by another file's section are complete.
        (cut, vec!["b", "c"]),
        // A file the batch printed nothing for keeps its own call.
        (
            result(0, "diff --git a/b.txt b/b.txt\n+b\n"),
            vec!["a", "c"],
        ),
    ];
    for (batch, per_file) in cases {
        let directory = temp_dir();
        let mut results = base_results(" M a.txt\0 M b.txt\0 M c.txt\0");
        results.push(result(
            0,
            concat!("1\t0\ta.txt\0", "1\t0\tb.txt\0", "1\t0\tc.txt\0"),
        ));
        results.push(batch);
        for name in &per_file {
            results.push(result(
                0,
                &format!("diff --git a/{name}.txt b/{name}.txt\n+{name}\n"),
            ));
        }
        let runner = FakeGit::with(results);
        let options = RepositoryReviewOptions {
            runner: Some(&runner),
            ..RepositoryReviewOptions::default()
        };

        let review = read_repository_review_with(directory.path(), &options);

        assert!(review.error.is_none(), "{per_file:?}");
        for (file, name) in review.files.iter().zip(["a", "b", "c"]) {
            assert_eq!(
                file.patch,
                format!(
                    "Interface layer: unstaged\ndiff --git a/{name}.txt b/{name}.txt\n+{name}\n"
                ),
                "{per_file:?}"
            );
        }
        let calls = runner.calls();
        assert_eq!(calls.len(), 6 + per_file.len(), "{per_file:?}");
        for (call, name) in calls[6..].iter().zip(&per_file) {
            assert!(!call.iter().any(|arg| arg == "--no-renames"));
            assert_eq!(call.last(), Some(&format!(":(literal){name}.txt")));
        }
    }
}

#[test]
fn a_batched_patch_is_capped_exactly_like_its_own_call() {
    let directory = temp_dir();
    let mut results = base_results(" M a.txt\0 M b.txt\0");
    results.push(result(0, concat!("3\t0\ta.txt\0", "1\t0\tb.txt\0")));
    results.push(result(
        0,
        "diff --git a/a.txt b/a.txt\n+\u{e9}\u{e9}\u{e9}\ndiff --git a/b.txt b/b.txt\n+b\n",
    ));
    let runner = FakeGit::with(results);
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        limits: RepositoryReviewLimits {
            // Cuts inside the second two-byte character of a.txt's patch.
            max_patch_bytes_per_file: 60,
            ..RepositoryReviewLimits::default()
        },
        ..RepositoryReviewOptions::default()
    };

    let review = read_repository_review_with(directory.path(), &options);

    assert_eq!(runner.calls().len(), 6);
    let capped = &review.files[0];
    assert!(capped.patch_truncated);
    assert_eq!(capped.omitted, Some(RepositoryReviewOmission::OutputLimit));
    assert!(capped.patch.len() <= 60);
    assert!(
        "Interface layer: unstaged\ndiff --git a/a.txt b/a.txt\n+\u{e9}\u{e9}\u{e9}\n"
            .starts_with(&capped.patch)
    );
    assert_eq!(
        review.files[1].patch,
        "Interface layer: unstaged\ndiff --git a/b.txt b/b.txt\n+b\n"
    );
}

/// Runs real git, counting spawns; with `batch: false` it refuses every
/// batched patch call so the review takes today's per-file path.
struct CountingGit {
    batch: bool,
    calls: std::sync::atomic::AtomicUsize,
}

impl ReviewGitRunner for CountingGit {
    fn run(&self, args: &[String], cwd: &Path, maximum: usize) -> ReviewGitResult {
        if !self.batch && args.iter().any(|arg| arg == "--submodule=short") {
            return result(128, "");
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        interface_review::SystemReviewGitRunner.run(args, cwd, maximum)
    }
}

fn git_allowing_failure(root: &Path, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be available for the real-repository test");
}

#[cfg(unix)]
#[test]
#[allow(clippy::too_many_lines)] // One fixture covering every diff shape.
fn batched_patches_match_per_file_patches_byte_for_byte_in_a_real_repository() {
    use std::fmt::Write as _;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = temp_dir();
    let root = directory.path();
    git(root, &["init", "--quiet", "--initial-branch=main"]);
    for (key, value) in [
        ("user.email", "review@example.invalid"),
        ("user.name", "Review Test"),
        // Pin what a developer's global config could otherwise vary.
        ("diff.noprefix", "false"),
        ("diff.mnemonicPrefix", "false"),
        ("core.autocrlf", "false"),
        ("commit.gpgsign", "false"),
    ] {
        git(root, &["config", key, value]);
    }
    let lines = |tag: &str, count: usize| {
        (0..count).fold(String::new(), |mut text, line| {
            let _ = writeln!(text, "{tag} line {line}");
            text
        })
    };
    let names = [
        "plain.txt",
        "both.txt",
        "gone.txt",
        "mode.sh",
        "type.txt",
        "type-unstaged.txt",
        "tab\tname.txt",
        "quote\"name.txt",
        "x b/y.txt",
        "old.txt",
        "conflict.txt",
        "latin1.txt",
        "big.txt",
        "z-last.txt",
    ];
    fs::create_dir(root.join("x b")).expect("spaced directory");
    for name in names {
        fs::write(root.join(name), lines(name, 12)).expect("baseline file");
    }
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", "baseline"]);
    // A conflicted path, merged from a side branch.
    git(root, &["checkout", "--quiet", "-b", "side"]);
    fs::write(root.join("conflict.txt"), "side\n").expect("side edit");
    git(root, &["commit", "--quiet", "-am", "side"]);
    git(root, &["checkout", "--quiet", "main"]);
    fs::write(root.join("conflict.txt"), "main\n").expect("main edit");
    git(root, &["commit", "--quiet", "-am", "main"]);
    git_allowing_failure(root, &["merge", "--quiet", "side"]);

    let edit = |name: &str, tag: &str| {
        let mut text = lines(name, 12);
        text.insert_str(text.len() / 2, &format!("{tag} inserted\n"));
        fs::write(root.join(name), text).expect("edit");
    };
    edit("plain.txt", "unstaged");
    edit("both.txt", "staged");
    git(root, &["add", "--", "both.txt"]);
    edit("both.txt", "unstaged");
    git(root, &["rm", "--quiet", "--", "gone.txt"]);
    fs::set_permissions(root.join("mode.sh"), fs::Permissions::from_mode(0o755))
        .expect("mode change");
    fs::remove_file(root.join("type.txt")).expect("typechange");
    symlink("plain.txt", root.join("type.txt")).expect("staged symlink");
    git(root, &["add", "--", "type.txt"]);
    fs::remove_file(root.join("type-unstaged.txt")).expect("typechange");
    symlink("plain.txt", root.join("type-unstaged.txt")).expect("unstaged symlink");
    for name in [
        "tab\tname.txt",
        "quote\"name.txt",
        "x b/y.txt",
        "z-last.txt",
    ] {
        edit(name, "special");
    }
    git(root, &["mv", "old.txt", "renamed.txt"]);
    fs::write(root.join("fresh.txt"), lines("fresh", 5)).expect("added file");
    git(root, &["add", "--", "fresh.txt"]);
    fs::write(root.join("latin1.txt"), b"caf\xe9 au lait\n").expect("non-UTF-8 edit");
    fs::write(root.join("big.txt"), lines("big", 400)).expect("large edit");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("head");
    let head = String::from_utf8(head.stdout).expect("utf-8 head");
    git(
        root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},sub", head.trim()),
        ],
    );

    let tight = RepositoryReviewLimits {
        max_patch_bytes_per_file: 700,
        max_total_patch_bytes: 4_000,
        ..RepositoryReviewLimits::default()
    };
    for limits in [RepositoryReviewLimits::default(), tight] {
        let review = |batch: bool| {
            let runner = CountingGit {
                batch,
                calls: std::sync::atomic::AtomicUsize::new(0),
            };
            let options = RepositoryReviewOptions {
                runner: Some(&runner),
                limits,
                ..RepositoryReviewOptions::default()
            };
            let review = read_repository_review_with(root, &options);
            (
                serde_json::to_value(&review).expect("wire JSON"),
                runner.calls.load(Ordering::SeqCst),
            )
        };
        let (per_file, per_file_calls) = review(false);
        let (batched, batched_calls) = review(true);
        assert_eq!(batched, per_file);
        assert!(
            batched_calls < per_file_calls,
            "{batched_calls} batched vs {per_file_calls} per-file spawns"
        );
        let paths = per_file["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|file| file["path"].as_str().expect("path").to_owned())
            .collect::<Vec<_>>();
        for expected in [
            "type.txt",
            "tab\tname.txt",
            "x b/y.txt",
            "sub",
            "conflict.txt",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} in {paths:?}"
            );
        }
    }
}

/// A runner that may overlap reads, answering by subcommand like real git.
struct OverlappingGit {
    repository: bool,
    calls: Mutex<Vec<String>>,
}

impl ReviewGitRunner for OverlappingGit {
    fn overlaps_reads(&self) -> bool {
        true
    }

    fn run(&self, args: &[String], _cwd: &Path, _maximum: usize) -> ReviewGitResult {
        let subcommand = args
            .iter()
            .find(|arg| ["rev-parse", "symbolic-ref", "status"].contains(&arg.as_str()))
            .expect("a review subcommand")
            .clone();
        self.calls.lock().expect("calls lock").push(subcommand);
        if !self.repository {
            return result(128, "");
        }
        match (args.contains(&"--is-inside-work-tree".to_owned()), args) {
            (true, _) => result(0, "true\n"),
            (false, args) if args.contains(&"symbolic-ref".to_owned()) => result(0, "main\n"),
            (false, args) if args.contains(&"--verify".to_owned()) => {
                result(0, "0123456789abcdef\n")
            }
            _ => result(0, "## main\0"),
        }
    }
}

#[test]
fn overlapped_metadata_reads_are_judged_in_the_same_order() {
    let directory = temp_dir();
    let overlapping = OverlappingGit {
        repository: true,
        calls: Mutex::new(Vec::new()),
    };
    let review = read_repository_review_with(
        directory.path(),
        &RepositoryReviewOptions {
            runner: Some(&overlapping),
            ..RepositoryReviewOptions::default()
        },
    );
    let sequential = FakeGit::with(base_results("## main\0"));
    let expected = read_repository_review_with(
        directory.path(),
        &RepositoryReviewOptions {
            runner: Some(&sequential),
            ..RepositoryReviewOptions::default()
        },
    );
    assert_eq!(review, expected);
    assert_eq!(review.branch.as_deref(), Some("main"));
    assert_eq!(overlapping.calls.lock().expect("calls lock").len(), 4);

    // Outside a repository the overlapped reads are discarded unread.
    let outside = OverlappingGit {
        repository: false,
        calls: Mutex::new(Vec::new()),
    };
    let review = read_repository_review_with(
        directory.path(),
        &RepositoryReviewOptions {
            runner: Some(&outside),
            ..RepositoryReviewOptions::default()
        },
    );
    assert!(!review.repository);
    assert_eq!(
        review.error.map(|error| error.code),
        Some(RepositoryReviewErrorCode::NotRepository)
    );
}

/// Turns each untracked file into a FIFO right before the review opens it:
/// opening one blocks forever, as a file iCloud has evicted waits for its
/// download.
struct EvictingHook {
    opened: Mutex<Vec<std::path::PathBuf>>,
}

impl RepositoryReviewHooks for EvictingHook {
    fn before_untracked_open(&self, file: &Path) {
        self.opened
            .lock()
            .expect("opened lock")
            .push(file.to_path_buf());
        fs::remove_file(file).expect("remove the downloaded copy");
        assert!(
            Command::new("/usr/bin/mkfifo")
                .arg(file)
                .status()
                .expect("mkfifo")
                .success()
        );
    }
}

#[test]
fn untracked_files_still_downloading_from_icloud_are_omitted_at_their_deadline() {
    let directory = temp_dir();
    let root = directory.path();
    for name in ["first.txt", "second.txt", "third.txt"] {
        fs::write(root.join(name), "bytes on another Mac").expect("untracked file");
    }
    let hooks = EvictingHook {
        opened: Mutex::new(Vec::new()),
    };
    let runner = FakeGit::with(base_results("?? first.txt\0?? second.txt\0?? third.txt\0"));
    let options = RepositoryReviewOptions {
        runner: Some(&runner),
        hooks: Some(&hooks),
        ..RepositoryReviewOptions::default()
    };

    let started = std::time::Instant::now();
    let review = read_repository_review_with(root, &options);

    // One read waits out its deadline; the rest of an evicted project is not
    // read at all, so the review answers instead of hanging.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(9),
        "the review waited {:?}",
        started.elapsed()
    );
    assert_eq!(hooks.opened.lock().expect("opened lock").len(), 1);
    assert_eq!(review.files.len(), 3);
    for file in &review.files {
        assert_eq!(file.omitted, Some(RepositoryReviewOmission::NotRegular));
        assert!(file.patch.is_empty());
    }
    // Let the abandoned reader go.
    for opened in hooks.opened.lock().expect("opened lock").iter() {
        let _ = fs::OpenOptions::new().write(true).open(opened);
    }
}
