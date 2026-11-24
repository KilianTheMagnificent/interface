//! Git mechanics behind Threads (`docs/superpowers/specs/2026-10-06-threads-design.md`):
//! thread worktrees, hidden per-turn checkpoint refs, diffs, numstat and
//! restore. Synchronous; the host calls these on its blocking pool.
//!
//! Every git child is bounded (deadline, output cap, process-group teardown)
//! and runs with the inherited `GIT_*` environment removed, terminal prompts
//! and optional locks off, and hooks off — except `git worktree add`, which
//! runs the repository's own checkout hooks exactly as a terminal checkout
//! would, and the explicit commit and push. A child with hooks off also runs
//! none of the programs the repository's settings name (filter drivers, ssh,
//! credential and signing programs; `interface_common::git_hardening`). Nothing here writes the user's index, HEAD, branches or stash: a
//! checkpoint stages into a private temporary index, and only refs under
//! `refs/interface/` (plus a thread's own new `iface/` branch) are created or
//! deleted. [`restore_to`] is the one deliberate exception: it rewrites the
//! working tree and index of the folder it is given, after saving a recovery
//! checkpoint first.

use std::ffi::OsStr;
use std::fmt::{self, Write as _};
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use interface_common::bounded_read;
use interface_common::git_hardening;
use interface_contracts::threads::{
    BranchList, DiffFile, DiffFileStatus, DiffOmitted, DiffScope, RestoreResult, ThreadDiff,
};
use interface_workspace::{is_likely_binary_path, is_search_excluded_path};

use crate::process::{BoundedOutput, run_bounded_command};

/// How `prepare_workspace` gives a thread its folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareMode {
    /// The project's own checkout.
    Local,
    /// A new worktree on a new branch.
    Worktree,
    /// An existing worktree of the same repository.
    Existing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitWorkspaceRequest {
    pub project_root: PathBuf,
    pub mode: PrepareMode,
    pub base_ref: Option<String>,
    /// Required for [`PrepareMode::Existing`].
    pub worktree_path: Option<PathBuf>,
    /// The first prompt; a new worktree's slug comes from it.
    pub hint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedGitWorkspace {
    pub cwd: PathBuf,
    /// `None` for a folder that is not a git repository.
    pub repo_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    /// This call created the worktree (and its branch).
    pub created: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureOutcome {
    Captured {
        commit: String,
    },
    /// Over the path or file-size limits, or not a repository; never blocks a send.
    Skipped {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoveOutcome {
    Removed,
    /// The worktree has changes, so it stays.
    KeptDirty,
    /// The worktree holds ignored files (dependencies, build output, local
    /// settings such as `.env`), which `git worktree remove` would delete
    /// with it, so it stays for the owner to remove.
    KeptIgnored,
    Missing,
    /// A live session or another thread still uses it.
    InUse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffRequest {
    pub scope: DiffScope,
    pub base: String,
    /// `None` = the working tree.
    pub head: Option<String>,
    /// `None` = every changed file; else only these repository-relative
    /// paths (none listed: no file). A turn in a shared checkout passes the
    /// files its own edits named.
    pub paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadGitError {
    pub code: &'static str,
    pub message: String,
}

impl ThreadGitError {
    pub const UNSUPPORTED_CODE: &'static str = "thread-git-unsupported";
    /// git could not be started.
    pub const UNAVAILABLE_CODE: &'static str = "thread-git-unavailable";
    /// git did not answer within its deadline.
    pub const TIMEOUT_CODE: &'static str = "thread-git-timeout";
    /// The folder is not inside a git repository.
    pub const NOT_REPOSITORY_CODE: &'static str = "thread-git-not-repository";
    /// A ref, path or name was refused before git ran.
    pub const INVALID_CODE: &'static str = "thread-git-invalid";
    /// git ran and refused.
    pub const FAILED_CODE: &'static str = "thread-git-failed";
    /// The working tree is over the checkpoint limits, so it cannot be read
    /// as a tree, compared or restored.
    pub const TOO_LARGE_CODE: &'static str = "thread-git-too-large";
    /// A file git needs is still downloading from iCloud, so the read
    /// stopped at its deadline instead of waiting for the download.
    pub const DOWNLOADING_CODE: &'static str = "thread-git-downloading";

    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The operation is not available in this build.
    #[must_use]
    pub fn unsupported() -> Self {
        Self::new(
            Self::UNSUPPORTED_CODE,
            "Thread git operations are not available in this build yet.",
        )
    }
}

impl fmt::Display for ThreadGitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ThreadGitError {}

/// Checkpoint bounds (design "Checkpoint capture").
#[derive(Clone, Copy, Debug)]
struct CaptureLimits {
    max_paths: usize,
    max_new_file_bytes: u64,
    budget: Duration,
}

const CAPTURE_LIMITS: CaptureLimits = CaptureLimits {
    max_paths: 20_000,
    max_new_file_bytes: 50 * 1024 * 1024,
    budget: Duration::from_secs(10),
};

/// Diff bounds (design "Diffs"). The file cap keeps one answer inside an IPC
/// frame however large the change is.
#[derive(Clone, Copy, Debug)]
struct DiffLimits {
    files: usize,
    file_patch_bytes: usize,
    total_patch_bytes: usize,
}

const DIFF_LIMITS: DiffLimits = DiffLimits {
    files: 3_000,
    file_patch_bytes: 256 * 1024,
    total_patch_bytes: 2 * 1024 * 1024,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// A checkout (or its removal) writes every file of the project.
const WORKTREE_TIMEOUT: Duration = Duration::from_secs(120);
const METADATA_BYTES: usize = 64 * 1024;
const LISTING_BYTES: usize = 8 * 1024 * 1024;
const DIRTY_PROBE_BYTES: usize = 4 * 1024;
const MAX_BRANCHES: usize = 1_000;
/// Per-file patches run this many git children at a time.
const PATCH_WORKERS: usize = 4;
const CHECKPOINT_MESSAGE: &str = "interface thread checkpoint";
const CHECKPOINT_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Interface"),
    ("GIT_AUTHOR_EMAIL", "interface@localhost"),
    ("GIT_COMMITTER_NAME", "Interface"),
    ("GIT_COMMITTER_EMAIL", "interface@localhost"),
];
const THREAD_REF_ROOT: &str = "refs/interface/";
const WORKTREE_DIRECTORY: &str = ".interface-worktrees";
const BRANCH_PREFIX: &str = "iface/";
const SLUG_MAX: usize = 32;
const NAME_ATTEMPTS: usize = 3;
/// `ThreadDiff.head` when the newer side is the working tree.
const WORKING_TREE: &str = "working-tree";

/// The repository root containing `path`, or `None` outside a repository.
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn repo_root(path: &Path) -> Result<Option<PathBuf>, ThreadGitError> {
    toplevel(path, Call::read())
}

/// Validates and prepares a thread's folder: the checkout itself, a new
/// worktree, or an existing worktree of the same repository.
///
/// # Errors
///
/// Fails when the folder or worktree cannot be used.
pub fn prepare_workspace(
    request: &GitWorkspaceRequest,
) -> Result<PreparedGitWorkspace, ThreadGitError> {
    if !request.project_root.is_dir() {
        return Err(invalid(format!(
            "{} is not a folder Interface can open.",
            request.project_root.display()
        )));
    }
    match request.mode {
        PrepareMode::Local => Ok(PreparedGitWorkspace {
            cwd: request.project_root.clone(),
            repo_root: repo_root(&request.project_root)?,
            worktree_path: None,
            branch: None,
            base_ref: None,
            created: false,
        }),
        PrepareMode::Worktree => create_thread_worktree(request, Call::checkout()),
        PrepareMode::Existing => existing_worktree(request),
    }
}

/// Undoes a `prepare_workspace` whose launch failed: removes a just-created
/// clean worktree, and its branch when it has no commits. A workspace the
/// preparation did not create is left alone and answers
/// [`RemoveOutcome::Missing`]: there is nothing of the thread's to remove.
///
/// # Errors
///
/// Fails when git cannot inspect or remove the worktree.
pub fn discard_workspace(prepared: &PreparedGitWorkspace) -> Result<RemoveOutcome, ThreadGitError> {
    if !prepared.created {
        return Ok(RemoveOutcome::Missing);
    }
    let (Some(repo), Some(path)) = (&prepared.repo_root, &prepared.worktree_path) else {
        return Err(invalid(
            "That prepared workspace does not name its repository and worktree.",
        ));
    };
    let outcome = remove_worktree(repo, path)?;
    if outcome == RemoveOutcome::Removed
        && let (Some(branch), Some(base)) = (&prepared.branch, &prepared.base_ref)
    {
        delete_branch_without_commits(repo, branch, base)?;
    }
    Ok(outcome)
}

/// Recreates a missing worktree from its branch. Answers `true` when it was recreated.
///
/// # Errors
///
/// Fails when the worktree is missing and cannot be recreated.
pub fn ensure_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<bool, ThreadGitError> {
    ensure_worktree_with(repo_root, worktree_path, branch, Call::checkout())
}

/// [`ensure_worktree`] with its checkout bounded by `checkout`.
fn ensure_worktree_with(
    repo_root: &Path,
    worktree_path: &Path,
    branch: &str,
    checkout: Call<'_>,
) -> Result<bool, ThreadGitError> {
    validate_branch(branch)?;
    let path = text_path(worktree_path)?;
    if fs::symlink_metadata(worktree_path).is_ok() {
        let records = worktree_records(repo_root)?;
        return if find_worktree(&records, worktree_path).is_some() {
            Ok(false)
        } else {
            Err(invalid(format!(
                "{path} exists but is not a worktree of {}.",
                repo_root.display()
            )))
        };
    }
    let reference = format!("refs/heads/{branch}");
    if !git(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &reference],
        Call::read(),
    )?
    .succeeded()
    {
        return Err(ThreadGitError::new(
            ThreadGitError::FAILED_CODE,
            format!("The branch {branch} no longer exists, so its worktree cannot be recreated."),
        ));
    }
    // Only this path's stale registration goes (`git worktree remove` takes
    // a missing folder's); a repository-wide prune would also forget other
    // worktrees that are only away for now, on a drive that is unplugged.
    let records = worktree_records(repo_root)?;
    if let Some(record) = find_missing_worktree(&records, worktree_path) {
        if record.locked {
            return Err(ThreadGitError::new(
                ThreadGitError::FAILED_CODE,
                format!(
                    "git has the worktree at {path} locked, so Interface left it alone. Unlock it (git worktree unlock), then try again."
                ),
            ));
        }
        let forgotten = git(
            repo_root,
            &["worktree", "remove", "--force", path],
            Call::read(),
        )?;
        if !forgotten.succeeded() {
            return Err(failed(
                &format!("forget the missing worktree at {path}"),
                &forgotten,
            ));
        }
    }
    let added = git(repo_root, &["worktree", "add", path, branch], checkout);
    let error = match added {
        Ok(added) if added.succeeded() => return Ok(true),
        Ok(added) => failed(&format!("recreate the worktree at {path}"), &added),
        Err(error) => error,
    };
    // A checkout that failed or ran out of time leaves no half-made folder
    // or registration behind; the branch, holding the thread's commits,
    // always stays.
    remove_partial_worktree(repo_root, worktree_path);
    Err(error)
}

/// Captures the working tree (untracked included, ignored excluded) as a
/// commit under `refname`, without touching the index, branch or stash.
///
/// # Errors
///
/// Fails when git fails; over-limit trees answer [`CaptureOutcome::Skipped`].
pub fn capture_checkpoint(cwd: &Path, refname: &str) -> Result<CaptureOutcome, ThreadGitError> {
    capture_with(cwd, refname, CAPTURE_LIMITS)
}

/// `(files, added, removed)` between `from_ref` and `to_ref` (`None` = the working tree).
///
/// # Errors
///
/// Fails when either side cannot be read.
pub fn numstat(
    cwd: &Path,
    from_ref: &str,
    to_ref: Option<&str>,
) -> Result<(u32, u32, u32), ThreadGitError> {
    let repo = require_repo(cwd)?;
    let from = resolve_commit(&repo, from_ref)?;
    let to = match to_ref {
        Some(to_ref) => resolve_commit(&repo, to_ref)?,
        None => working_tree(&repo)?,
    };
    Ok(totals(&changes_between(&repo, &from, &to, &[])?))
}

/// [`numstat`] over only `paths`: each absolute or relative to `cwd`, and
/// kept only when it names something inside the repository — a path outside
/// it, through a symlink out of it, or whose `..` climbs out of it is
/// dropped. A rename counts as one file only when both its sides are listed.
/// No path left counts nothing: the filter never widens to the whole tree.
///
/// # Errors
///
/// Fails when either side cannot be read.
pub fn numstat_paths(
    cwd: &Path,
    from_ref: &str,
    to_ref: Option<&str>,
    paths: &[String],
) -> Result<(u32, u32, u32), ThreadGitError> {
    let repo = require_repo(cwd)?;
    let from = resolve_commit(&repo, from_ref)?;
    let to = match to_ref {
        Some(to_ref) => resolve_commit(&repo, to_ref)?,
        None => working_tree(&repo)?,
    };
    let real_repo = fs::canonicalize(&repo).unwrap_or_else(|_| repo.clone());
    let mut specs = paths
        .iter()
        .filter_map(|path| repo_pathspec(&real_repo, cwd, path))
        .collect::<Vec<_>>();
    specs.sort_unstable();
    specs.dedup();
    if specs.is_empty() {
        return Ok((0, 0, 0));
    }
    Ok(totals(&changes_between(&repo, &from, &to, &specs)?))
}

/// `paths` (each absolute, or relative to `cwd`) as paths relative to the
/// root of the repository `cwd` is in, sorted and without repeats. A path
/// outside the repository, through a symlink out of it, or whose `..` climbs
/// out of it is dropped, as [`numstat_paths`] drops it.
///
/// # Errors
///
/// Fails when `cwd` is not in a repository git can read.
pub fn repo_relative_paths(cwd: &Path, paths: &[String]) -> Result<Vec<String>, ThreadGitError> {
    let repo = require_repo(cwd)?;
    let real_repo = fs::canonicalize(&repo).unwrap_or(repo);
    let mut relative = paths
        .iter()
        .filter_map(|path| repo_relative(&real_repo, cwd, path))
        .collect::<Vec<_>>();
    relative.sort_unstable();
    relative.dedup();
    Ok(relative)
}

/// A repository-relative path as a literal pathspec, or `None` for one that
/// is empty, absolute, climbs or holds a control character.
fn literal_spec(path: &str) -> Option<String> {
    let plain = !path.is_empty()
        && !path.starts_with('/')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|name| !name.is_empty() && name != "." && name != "..");
    plain.then(|| format!(":(literal){path}"))
}

/// `(files, added, removed)` over `changes`.
fn totals(changes: &[Change]) -> (u32, u32, u32) {
    let (added, removed) = changes
        .iter()
        .fold((0_u32, 0_u32), |(added, removed), change| {
            (
                added.saturating_add(change.added),
                removed.saturating_add(change.removed),
            )
        });
    (count_u32(changes.len()), added, removed)
}

/// `path` (absolute, or relative to `cwd`) as a literal pathspec relative to
/// `repo` (a resolved path), or `None` when it names nothing inside it. The
/// deepest folder of it that still exists is resolved, so a symlinked
/// spelling of the repository still matches and a symlink out of it does
/// not; the file itself may be gone (a turn that deleted it).
fn repo_pathspec(repo: &Path, cwd: &Path, path: &str) -> Option<String> {
    repo_relative(repo, cwd, path).map(|relative| format!(":(literal){relative}"))
}

/// [`repo_pathspec`]'s path, without the literal magic.
fn repo_relative(repo: &Path, cwd: &Path, path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let mut lexical = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => lexical.push(component),
            Component::CurDir => {}
            // Climbing above the root is climbing out, not staying at `/`.
            Component::ParentDir => {
                lexical.parent()?;
                lexical.pop();
            }
            Component::Normal(name) => lexical.push(name),
        }
    }
    if !lexical.is_absolute() {
        return None;
    }
    let (folder, real) = lexical
        .ancestors()
        .skip(1)
        .find_map(|folder| Some((folder, fs::canonicalize(folder).ok()?)))?;
    let resolved = real.join(lexical.strip_prefix(folder).ok()?);
    let names = resolved
        .strip_prefix(repo)
        .ok()?
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!names.is_empty()).then(|| names.join("/"))
}

/// The bounded, credential-filtered diff for `request`.
///
/// # Errors
///
/// Fails when either side cannot be read.
pub fn diff(cwd: &Path, request: &DiffRequest) -> Result<ThreadDiff, ThreadGitError> {
    diff_with(cwd, request, DIFF_LIMITS)
}

/// The merge base of `base_ref` and HEAD, or `None` when they share no history.
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn merge_base(cwd: &Path, base_ref: &str) -> Result<Option<String>, ThreadGitError> {
    validate_revision(base_ref)?;
    let repo = require_repo(cwd)?;
    let output = git(&repo, &["merge-base", base_ref, "HEAD"], Call::read())?;
    match output.code {
        Some(0) if !output.truncated => Ok(Some(output.line())),
        Some(1) => Ok(None),
        _ => Err(failed(
            &format!("find where {base_ref} and HEAD meet"),
            &output,
        )),
    }
}

/// Captures `recovery_ref`, then restores the working tree and index to `refname`.
///
/// # Errors
///
/// Fails before restoring when the recovery capture fails.
pub fn restore_to(
    cwd: &Path,
    refname: &str,
    recovery_ref: &str,
) -> Result<RestoreResult, ThreadGitError> {
    restore_with(cwd, refname, recovery_ref, CAPTURE_LIMITS)
}

/// Removes a clean worktree directory; its branch is always kept.
///
/// # Errors
///
/// Fails when git cannot inspect or remove the worktree.
pub fn remove_worktree(
    repo_root: &Path,
    worktree_path: &Path,
) -> Result<RemoveOutcome, ThreadGitError> {
    let path = text_path(worktree_path)?;
    if fs::symlink_metadata(worktree_path).is_err() {
        return Ok(RemoveOutcome::Missing);
    }
    let records = worktree_records(repo_root)?;
    let Some((index, record)) = find_worktree(&records, worktree_path) else {
        return Err(invalid(format!(
            "{path} is not a worktree of {}.",
            repo_root.display()
        )));
    };
    if index == 0 {
        return Err(invalid(format!(
            "{path} is the repository's own checkout, not a thread worktree."
        )));
    }
    if record.locked {
        return Ok(RemoveOutcome::InUse);
    }
    if is_dirty(worktree_path)? {
        return Ok(RemoveOutcome::KeptDirty);
    }
    if has_ignored_files(worktree_path)? {
        return Ok(RemoveOutcome::KeptIgnored);
    }
    let removed = git(repo_root, &["worktree", "remove", path], Call::long())?;
    if removed.succeeded() {
        return Ok(RemoveOutcome::Removed);
    }
    let detail = removed.detail();
    if detail.contains("modified or untracked files") {
        return Ok(RemoveOutcome::KeptDirty);
    }
    if detail.contains("locked") {
        return Ok(RemoveOutcome::InUse);
    }
    Err(failed(&format!("remove the worktree at {path}"), &removed))
}

/// Whether the working tree has changes (untracked files included).
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn is_dirty(cwd: &Path) -> Result<bool, ThreadGitError> {
    let repo = require_repo(cwd)?;
    let output = git(
        &repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        Call {
            max_output: DIRTY_PROBE_BYTES,
            ..Call::read()
        },
    )?;
    if output.truncated {
        return Ok(true);
    }
    if output.code != Some(0) {
        return Err(failed("read the working tree's status", &output));
    }
    Ok(!output.stdout.is_empty())
}

/// Whether the working tree holds ignored files or folders, which a clean
/// `git worktree remove` deletes without asking. Every kind counts, a
/// rebuildable `node_modules` or `target` as much as a `.env`: the owner
/// decides what is safe to lose.
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn has_ignored_files(cwd: &Path) -> Result<bool, ThreadGitError> {
    let repo = require_repo(cwd)?;
    let output = git(
        &repo,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
        Call {
            max_output: DIRTY_PROBE_BYTES,
            ..Call::read()
        },
    )?;
    if output.truncated {
        return Ok(true);
    }
    if output.code != Some(0) {
        return Err(failed("list the working tree's ignored files", &output));
    }
    Ok(!output.stdout.is_empty())
}

/// The project's local branches (most recently committed first), its current
/// branch and its default branch (from `origin/HEAD`).
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn list_branches(project_root: &Path) -> Result<BranchList, ThreadGitError> {
    let Some(repo) = repo_root(project_root)? else {
        return Ok(BranchList::default());
    };
    let count = format!("--count={MAX_BRANCHES}");
    let listed = git(
        &repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            &count,
            "refs/heads/",
        ],
        Call::listing(),
    )?;
    if !listed.succeeded() {
        return Err(failed("list the branches", &listed));
    }
    let branches = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("refs/heads/"))
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let current = symbolic_ref(&repo, "HEAD")?
        .and_then(|target| target.strip_prefix("refs/heads/").map(ToOwned::to_owned));
    let default_branch = symbolic_ref(&repo, "refs/remotes/origin/HEAD")?.and_then(|target| {
        target
            .strip_prefix("refs/remotes/origin/")
            .filter(|branch| !branch.is_empty())
            .map(ToOwned::to_owned)
    });
    Ok(BranchList {
        current,
        default_branch,
        branches,
    })
}

/// The repository's worktrees and their branches. The repository's own
/// checkout comes first; bare and prunable (missing) entries are left out.
///
/// # Errors
///
/// Fails when git cannot be run.
pub fn list_worktrees(
    project_root: &Path,
) -> Result<Vec<(PathBuf, Option<String>)>, ThreadGitError> {
    let Some(repo) = repo_root(project_root)? else {
        return Ok(Vec::new());
    };
    Ok(worktree_records(&repo)?
        .into_iter()
        .filter(|record| !record.bare && !record.prunable)
        .map(|record| (record.path, record.branch))
        .collect())
}

/// Deletes every ref under `prefix`; answers how many were deleted.
///
/// # Errors
///
/// Fails when git cannot be run, and refuses any prefix outside `refs/interface/`.
pub fn delete_refs(cwd: &Path, prefix: &str) -> Result<u32, ThreadGitError> {
    let pattern = prefix.trim_end_matches('/');
    validate_thread_ref(pattern)?;
    let Some(repo) = repo_root(cwd)? else {
        return Ok(0);
    };
    let listed = git(
        &repo,
        &["for-each-ref", "--format=%(refname)", pattern],
        Call::listing(),
    )?;
    if !listed.succeeded() {
        return Err(failed("list the thread's checkpoints", &listed));
    }
    let nested = format!("{pattern}/");
    let listed = String::from_utf8_lossy(&listed.stdout);
    let refs = listed
        .lines()
        .filter(|name| *name == pattern || name.starts_with(&nested))
        .collect::<Vec<_>>();
    if refs.is_empty() {
        return Ok(0);
    }
    let input = refs.iter().fold(String::new(), |mut input, name| {
        let _ = writeln!(input, "delete {name}");
        input
    });
    let deleted = git_with_input(
        &repo,
        &["update-ref", "--stdin"],
        Call::read(),
        Some(input.into_bytes()),
    )?;
    if !deleted.succeeded() {
        return Err(failed("delete the thread's checkpoints", &deleted));
    }
    Ok(count_u32(refs.len()))
}

/// Renames the checkpoint ref `from` to `to` (both under `refs/interface/`):
/// a launch baseline captured before its thread had a key is filed under the
/// thread. One `update-ref --stdin` transaction writes `to` and deletes
/// `from` only while `from` still names the commit it was read at, so either
/// both happen or neither does. An existing `to` is replaced.
///
/// # Errors
///
/// Refuses refs outside `refs/interface/` and a folder outside a repository;
/// fails when `from` does not name a commit or git refuses the transaction.
pub fn move_ref(cwd: &Path, from: &str, to: &str) -> Result<(), ThreadGitError> {
    validate_thread_ref(from)?;
    validate_thread_ref(to)?;
    if from == to {
        return Err(invalid(format!("{from:?} cannot be moved onto itself.")));
    }
    let repo = require_repo(cwd)?;
    let commit = resolve_commit(&repo, from)?;
    let input = format!("update {to} {commit}\ndelete {from} {commit}\n");
    let moved = git_with_input(
        &repo,
        &["update-ref", "--stdin"],
        Call::read(),
        Some(input.into_bytes()),
    )?;
    if !moved.succeeded() {
        return Err(failed("file the checkpoint under its thread", &moved));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspaces

fn create_thread_worktree(
    request: &GitWorkspaceRequest,
    checkout: Call<'_>,
) -> Result<PreparedGitWorkspace, ThreadGitError> {
    let project = fs::canonicalize(&request.project_root).map_err(|error| {
        invalid(format!(
            "Interface could not open {} ({error}).",
            request.project_root.display()
        ))
    })?;
    let repo = require_repo(&project)?;
    let (base_commit, base_name) = resolve_base(&repo, request.base_ref.as_deref())?;
    let parent = repo.parent().unwrap_or(&repo).join(WORKTREE_DIRECTORY);
    fs::create_dir_all(&parent).map_err(|error| {
        ThreadGitError::new(
            ThreadGitError::FAILED_CODE,
            format!("Interface could not create {} ({error}).", parent.display()),
        )
    })?;
    let (path, branch) = add_named_worktree(
        &repo,
        &parent,
        &thread_slug(&request.hint),
        &base_commit,
        checkout,
    )?;
    let relative = project.strip_prefix(&repo).unwrap_or(Path::new(""));
    let cwd = path.join(relative);
    if let Err(error) = fs::create_dir_all(&cwd) {
        abandon_new_worktree(&repo, &path, &branch, &base_commit);
        return Err(ThreadGitError::new(
            ThreadGitError::FAILED_CODE,
            format!(
                "Interface created {} but could not open {} inside it ({error}). The unused worktree was removed.",
                path.display(),
                cwd.display()
            ),
        ));
    }
    Ok(PreparedGitWorkspace {
        cwd,
        repo_root: Some(repo),
        worktree_path: Some(path),
        branch: Some(branch),
        base_ref: Some(base_name),
        created: true,
    })
}

/// Adds `<parent>/<slug>-<6 hex>` on the new branch `iface/<slug>-<6 hex>`
/// at `base_commit`, trying a fresh suffix when a name is taken. Whatever a
/// failed or timed-out add left (folder, registration, branch) is removed.
fn add_named_worktree(
    repo: &Path,
    parent: &Path,
    slug: &str,
    base_commit: &str,
    checkout: Call<'_>,
) -> Result<(PathBuf, String), ThreadGitError> {
    for attempt in 1..=NAME_ATTEMPTS {
        let unique = format!("{slug}-{}", random_hex6());
        let path = parent.join(&unique);
        let branch = format!("{BRANCH_PREFIX}{unique}");
        let reference = format!("refs/heads/{branch}");
        let taken = fs::symlink_metadata(&path).is_ok()
            || git(
                repo,
                &["rev-parse", "--verify", "--quiet", &reference],
                Call::read(),
            )?
            .succeeded();
        if taken {
            continue;
        }
        let target = text_path(&path)?;
        let added = git(
            repo,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                &branch,
                target,
                base_commit,
            ],
            checkout,
        );
        let added = match added {
            Ok(added) if added.succeeded() => return Ok((path, branch)),
            Ok(added) => added,
            Err(error) => {
                // Timed out or lost mid-checkout: nothing of it stays.
                abandon_new_worktree(repo, &path, &branch, base_commit);
                return Err(error);
            }
        };
        abandon_new_worktree(repo, &path, &branch, base_commit);
        if attempt == NAME_ATTEMPTS || !is_collision(&added.stderr) {
            return Err(failed(
                &format!("create a worktree of {}", repo.display()),
                &added,
            ));
        }
    }
    Err(ThreadGitError::new(
        ThreadGitError::FAILED_CODE,
        format!(
            "Every worktree name Interface tried under {} was already taken.",
            parent.display()
        ),
    ))
}

fn existing_worktree(
    request: &GitWorkspaceRequest,
) -> Result<PreparedGitWorkspace, ThreadGitError> {
    let Some(worktree) = &request.worktree_path else {
        return Err(invalid("Choose the worktree this thread should use."));
    };
    let repo = require_repo(&request.project_root)?;
    let records = worktree_records(&repo)?;
    let Some((_, record)) =
        find_worktree(&records, worktree).filter(|(_, record)| !record.bare && !record.prunable)
    else {
        return Err(invalid(format!(
            "{} is not a worktree of {}.",
            worktree.display(),
            repo.display()
        )));
    };
    let path = fs::canonicalize(worktree).unwrap_or_else(|_| record.path.clone());
    Ok(PreparedGitWorkspace {
        cwd: path.clone(),
        repo_root: Some(repo),
        worktree_path: Some(path),
        branch: record.branch.clone(),
        base_ref: None,
        created: false,
    })
}

/// The commit a new worktree starts from and the name it is recorded under:
/// the requested ref, or the current branch (its commit when detached).
fn resolve_base(repo: &Path, base_ref: Option<&str>) -> Result<(String, String), ThreadGitError> {
    if let Some(base_ref) = base_ref.filter(|base_ref| !base_ref.trim().is_empty()) {
        return Ok((resolve_commit(repo, base_ref)?, base_ref.to_owned()));
    }
    let Some(head) = head_commit(repo, Call::read())? else {
        return Err(ThreadGitError::new(
            ThreadGitError::FAILED_CODE,
            format!(
                "{} has no commits yet, so there is nothing to make a worktree of.",
                repo.display()
            ),
        ));
    };
    let name = symbolic_ref(repo, "HEAD")?
        .and_then(|target| target.strip_prefix("refs/heads/").map(ToOwned::to_owned))
        .unwrap_or_else(|| head.clone());
    Ok((head, name))
}

/// Best-effort cleanup of a worktree this module just tried to add: the
/// checkout (fresh, nothing of the owner's) and its branch while the branch
/// still points at the base it was made from.
fn abandon_new_worktree(repo: &Path, path: &Path, branch: &str, base_commit: &str) {
    remove_partial_worktree(repo, path);
    let reference = format!("refs/heads/{branch}");
    let _ = git(
        repo,
        &["update-ref", "-d", &reference, base_commit],
        Call::read(),
    );
}

/// Best-effort removal of a worktree folder and registration a `git worktree
/// add` into a path that did not exist left behind. Forced twice: a checkout
/// cut short keeps git's "initializing" lock.
fn remove_partial_worktree(repo: &Path, path: &Path) {
    let Ok(target) = text_path(path) else {
        return;
    };
    let registered =
        worktree_records(repo).is_ok_and(|records| find_missing_worktree(&records, path).is_some());
    if registered || fs::symlink_metadata(path).is_ok() {
        let _ = git(
            repo,
            &["worktree", "remove", "--force", "--force", target],
            Call::long(),
        );
    }
}

/// Deletes `branch` only when it holds no commit `base` lacks.
fn delete_branch_without_commits(
    repo: &Path,
    branch: &str,
    base: &str,
) -> Result<bool, ThreadGitError> {
    validate_branch(branch)?;
    validate_revision(base)?;
    let reference = format!("refs/heads/{branch}");
    let tip = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &reference],
        Call::read(),
    )?;
    if !tip.succeeded() {
        return Ok(false);
    }
    let tip = tip.line();
    let range = format!("{base}..{tip}");
    let ahead = git(repo, &["rev-list", "--count", &range], Call::read())?;
    if !ahead.succeeded() || ahead.line() != "0" {
        return Ok(false);
    }
    Ok(git(repo, &["update-ref", "-d", &reference, &tip], Call::read())?.succeeded())
}

fn is_collision(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("already exists")
        || lower.contains("is already checked out")
        || lower.contains("already used by worktree")
}

/// Lowercase ASCII words of `hint` joined by `-`, at most 32 characters,
/// `thread` when nothing is left.
fn thread_slug(hint: &str) -> String {
    let mut slug = String::new();
    for word in hint
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        let separator = usize::from(!slug.is_empty());
        if slug.len() + separator + word.len() > SLUG_MAX {
            if slug.is_empty() {
                slug.push_str(&word[..SLUG_MAX]);
            }
            break;
        }
        if separator == 1 {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    if slug.is_empty() {
        "thread".to_owned()
    } else {
        slug
    }
}

fn random_hex6() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos()),
    );
    hasher.write_u32(std::process::id());
    hasher.write_u64(SEQUENCE.fetch_add(1, Ordering::Relaxed));
    format!("{:06x}", hasher.finish() & 0x00ff_ffff)
}

#[derive(Clone, Debug, Default)]
struct WorktreeRecord {
    path: PathBuf,
    branch: Option<String>,
    bare: bool,
    locked: bool,
    prunable: bool,
}

/// `git worktree list --porcelain -z`: one NUL-terminated attribute per
/// record, an empty record between worktrees, the main checkout first.
fn worktree_records(repo: &Path) -> Result<Vec<WorktreeRecord>, ThreadGitError> {
    let output = git(
        repo,
        &["worktree", "list", "--porcelain", "-z"],
        Call::listing(),
    )?;
    if !output.succeeded() {
        return Err(failed("list the repository's worktrees", &output));
    }
    let mut records = Vec::<WorktreeRecord>::new();
    for attribute in output.stdout.split(|byte| *byte == 0) {
        if let Some(path) = attribute.strip_prefix(b"worktree ") {
            records.push(WorktreeRecord {
                path: PathBuf::from(OsStr::from_bytes(path)),
                ..WorktreeRecord::default()
            });
            continue;
        }
        let Some(record) = records.last_mut() else {
            continue;
        };
        let attribute = String::from_utf8_lossy(attribute);
        if let Some(reference) = attribute.strip_prefix("branch ") {
            record.branch = reference.strip_prefix("refs/heads/").map(ToOwned::to_owned);
        } else if attribute == "bare" {
            record.bare = true;
        } else if attribute == "locked" || attribute.starts_with("locked ") {
            record.locked = true;
        } else if attribute == "prunable" || attribute.starts_with("prunable ") {
            record.prunable = true;
        }
    }
    Ok(records)
}

/// The record (and its position; 0 is the main checkout) whose folder is
/// `path`, compared after resolving links on both sides.
fn find_worktree<'a>(
    records: &'a [WorktreeRecord],
    path: &Path,
) -> Option<(usize, &'a WorktreeRecord)> {
    let wanted = fs::canonicalize(path).ok()?;
    records.iter().enumerate().find(|(_, record)| {
        fs::canonicalize(&record.path).is_ok_and(|candidate| candidate == wanted)
    })
}

/// The record registered at `path`, which may no longer exist: compared as
/// written, else by its parent folder (links resolved) and its name.
fn find_missing_worktree<'a>(
    records: &'a [WorktreeRecord],
    path: &Path,
) -> Option<&'a WorktreeRecord> {
    let located = |path: &Path| {
        let parent = fs::canonicalize(path.parent()?).ok()?;
        Some(parent.join(path.file_name()?))
    };
    let wanted = located(path);
    records
        .iter()
        .skip(1)
        .find(|record| record.path == path || (wanted.is_some() && located(&record.path) == wanted))
}

// ---------------------------------------------------------------------------
// Checkpoints

enum Snapshot {
    Tree { tree: String, head: Option<String> },
    Skipped(String),
}

fn capture_with(
    cwd: &Path,
    refname: &str,
    limits: CaptureLimits,
) -> Result<CaptureOutcome, ThreadGitError> {
    validate_thread_ref(refname)?;
    let deadline = Instant::now() + limits.budget;
    let captured = remaining(deadline)
        .and_then(|left| toplevel(cwd, Call::read().within(left)))
        .and_then(|repo| match repo {
            Some(repo) => checkpoint_commit(&repo, refname, limits, deadline),
            None => Ok(CaptureOutcome::Skipped {
                reason: "This folder is not a git repository.".to_owned(),
            }),
        });
    match captured {
        Err(error) if error.code == ThreadGitError::TIMEOUT_CODE => Ok(CaptureOutcome::Skipped {
            reason: over_budget(limits),
        }),
        other => other,
    }
}

fn capture_repo_with(
    repo: &Path,
    refname: &str,
    limits: CaptureLimits,
) -> Result<CaptureOutcome, ThreadGitError> {
    validate_thread_ref(refname)?;
    let deadline = Instant::now() + limits.budget;
    match checkpoint_commit(repo, refname, limits, deadline) {
        Err(error) if error.code == ThreadGitError::TIMEOUT_CODE => Ok(CaptureOutcome::Skipped {
            reason: over_budget(limits),
        }),
        other => other,
    }
}

fn checkpoint_commit(
    repo: &Path,
    refname: &str,
    limits: CaptureLimits,
    deadline: Instant,
) -> Result<CaptureOutcome, ThreadGitError> {
    let (tree, head) = match snapshot(repo, limits, deadline)? {
        Snapshot::Tree { tree, head } => (tree, head),
        Snapshot::Skipped(reason) => return Ok(CaptureOutcome::Skipped { reason }),
    };
    let mut arguments = vec!["commit-tree", "--no-gpg-sign", "-m", CHECKPOINT_MESSAGE];
    if let Some(head) = &head {
        arguments.extend(["-p", head]);
    }
    arguments.push(&tree);
    let committed = git(
        repo,
        &arguments,
        Call {
            identity: true,
            ..Call::read().within(remaining(deadline)?)
        },
    )?;
    if !committed.succeeded() {
        return Err(failed("record the checkpoint", &committed));
    }
    let commit = committed.line();
    let updated = git(
        repo,
        &["update-ref", refname, &commit],
        Call::read().within(remaining(deadline)?),
    )?;
    if !updated.succeeded() {
        return Err(failed(
            &format!("save the checkpoint as {refname}"),
            &updated,
        ));
    }
    Ok(CaptureOutcome::Captured { commit })
}

/// The working tree as a tree object, staged through a private temporary
/// index: untracked files included, ignored files excluded, the real index
/// untouched. Over-limit trees answer [`Snapshot::Skipped`].
fn snapshot(
    repo: &Path,
    limits: CaptureLimits,
    deadline: Instant,
) -> Result<Snapshot, ThreadGitError> {
    if let Some(reason) = over_limits(repo, limits, deadline)? {
        return Ok(Snapshot::Skipped(reason));
    }
    let head = head_commit(repo, Call::read().within(remaining(deadline)?))?;
    let scratch = tempfile::Builder::new()
        .prefix("interface-thread-index-")
        .tempdir()
        .map_err(|error| {
            ThreadGitError::new(
                ThreadGitError::FAILED_CODE,
                format!("Interface could not make a private index for the checkpoint ({error})."),
            )
        })?;
    let index = scratch.path().join("index");
    if let Some(head) = &head
        && !copy_real_index(repo, &index, deadline)?
    {
        let seeded = git(
            repo,
            &["read-tree", head],
            Call::read().within(remaining(deadline)?).indexed(&index),
        )?;
        if !seeded.succeeded() {
            return Err(failed("read HEAD into the checkpoint index", &seeded));
        }
    }
    let staged = git(
        repo,
        &["add", "--all", "--", "."],
        Call::listing().within(remaining(deadline)?).indexed(&index),
    )?;
    if !staged.succeeded() {
        return Err(failed("stage the working tree for the checkpoint", &staged));
    }
    let written = git(
        repo,
        &["write-tree"],
        Call::read().within(remaining(deadline)?).indexed(&index),
    )?;
    if !written.succeeded() {
        return Err(failed("write the checkpoint tree", &written));
    }
    Ok(Snapshot::Tree {
        tree: written.line(),
        head,
    })
}

/// The design's skip rules, read from `git status` without refreshing the
/// real index: more than `max_paths` changed or untracked paths, or any
/// untracked file over `max_new_file_bytes`.
fn over_limits(
    repo: &Path,
    limits: CaptureLimits,
    deadline: Instant,
) -> Result<Option<String>, ThreadGitError> {
    let status = git(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
        Call::listing().within(remaining(deadline)?),
    )?;
    let too_many = || {
        Some(format!(
            "More than {} files changed, so this turn has no checkpoint.",
            limits.max_paths
        ))
    };
    if status.truncated {
        return Ok(too_many());
    }
    if status.code != Some(0) {
        return Err(failed("read the working tree's status", &status));
    }
    let mut paths = 0_usize;
    for record in status.stdout.split(|byte| *byte == 0) {
        if record.len() < 4 {
            continue;
        }
        paths += 1;
        if paths > limits.max_paths {
            return Ok(too_many());
        }
        if let Some(relative) = record.strip_prefix(b"?? ") {
            let relative = Path::new(OsStr::from_bytes(relative));
            if let Ok(metadata) = fs::symlink_metadata(repo.join(relative))
                && metadata.is_file()
                && metadata.len() > limits.max_new_file_bytes
            {
                return Ok(Some(format!(
                    "{} is larger than {} MiB, so this turn has no checkpoint.",
                    relative.display(),
                    limits.max_new_file_bytes / (1024 * 1024)
                )));
            }
        }
    }
    Ok(None)
}

/// Seeds `index` with a copy of the real index, stat data and all, so
/// `git add --all` re-reads only what changed and leaves the tree the working
/// tree's whatever was staged (conflicts included: the add resolves them).
/// Nothing reads HEAD's tree objects: on a repository whose loose objects
/// iCloud has evicted, each one is a download, and walking HEAD
/// (`read-tree -m HEAD`, as this did until 7 Oct 2026) held a thread's
/// launch for the whole budget and then skipped its checkpoint. The one
/// difference from a HEAD seed: a file untracked with `git rm --cached` and
/// ignored stays out. Answers `false` when there is no index to copy and the
/// caller seeds from HEAD.
///
/// The copy is a plain file read, so it goes through the bounded project
/// read: an index iCloud has evicted fails with
/// [`ThreadGitError::DOWNLOADING_CODE`] after [`INDEX_COPY_LIMIT`] instead
/// of holding the thread until the download finishes.
fn copy_real_index(repo: &Path, index: &Path, deadline: Instant) -> Result<bool, ThreadGitError> {
    let located = git(
        repo,
        &["rev-parse", "--git-path", "index"],
        Call::read().within(remaining(deadline)?),
    )?;
    if !located.succeeded() {
        return Ok(false);
    }
    let real = repo.join(located.raw_line());
    let destination = index.to_path_buf();
    match bounded_read::within(remaining(deadline)?.min(INDEX_COPY_LIMIT), move || {
        fs::copy(&real, &destination)
    }) {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(_)) => {
            let _ = fs::remove_file(index);
            Ok(false)
        }
        Err(bounded_read::ReadStalled) => Err(ThreadGitError::new(
            ThreadGitError::DOWNLOADING_CODE,
            INDEX_DOWNLOADING,
        )),
    }
}

/// How long copying the real index may take. A local index copies in well
/// under a second; one iCloud has evicted waits for its download.
const INDEX_COPY_LIMIT: Duration = Duration::from_secs(5);
const INDEX_DOWNLOADING: &str = "This repository's Git index is still downloading from iCloud. Try again once it has downloaded.";

/// A throwaway tree of the working tree (no ref), for comparisons.
fn working_tree(repo: &Path) -> Result<String, ThreadGitError> {
    let deadline = Instant::now() + CAPTURE_LIMITS.budget;
    match snapshot(repo, CAPTURE_LIMITS, deadline) {
        Ok(Snapshot::Tree { tree, .. }) => Ok(tree),
        Ok(Snapshot::Skipped(reason)) => {
            Err(ThreadGitError::new(ThreadGitError::TOO_LARGE_CODE, reason))
        }
        Err(error) if error.code == ThreadGitError::TIMEOUT_CODE => Err(ThreadGitError::new(
            ThreadGitError::TOO_LARGE_CODE,
            over_budget(CAPTURE_LIMITS),
        )),
        Err(error) => Err(error),
    }
}

fn over_budget(limits: CaptureLimits) -> String {
    format!(
        "Reading the working tree took longer than {} seconds, so this turn has no checkpoint.",
        limits.budget.as_secs()
    )
}

fn remaining(deadline: Instant) -> Result<Duration, ThreadGitError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(ThreadGitError::new(
            ThreadGitError::TIMEOUT_CODE,
            "The checkpoint ran out of time.",
        ))
    } else {
        Ok(left)
    }
}

// ---------------------------------------------------------------------------
// Restore

fn restore_with(
    cwd: &Path,
    refname: &str,
    recovery_ref: &str,
    limits: CaptureLimits,
) -> Result<RestoreResult, ThreadGitError> {
    validate_thread_ref(recovery_ref)?;
    let repo = require_repo(cwd)?;
    let target = resolve_commit(&repo, refname)?;
    let recovery = match capture_repo_with(&repo, recovery_ref, limits)? {
        CaptureOutcome::Captured { commit } => commit,
        CaptureOutcome::Skipped { reason } => {
            return Err(ThreadGitError::new(
                ThreadGitError::TOO_LARGE_CODE,
                format!(
                    "Interface could not save the current files first, so nothing was restored. {reason}"
                ),
            ));
        }
    };
    let changed = git(
        &repo,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            &recovery,
            &target,
        ],
        Call::listing(),
    )?;
    if !changed.succeeded() {
        return Err(failed("compare the files with the checkpoint", &changed));
    }
    let restored = changed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .count();
    let source = format!("--source={target}");
    let restored_files = git(
        &repo,
        &["restore", &source, "--worktree", "--staged", "--", "."],
        Call::long(),
    )?;
    if !restored_files.succeeded() {
        return Err(after_recovery(
            &failed("restore the checkpoint", &restored_files),
            recovery_ref,
        ));
    }
    let cleaned = git(&repo, &["clean", "-f", "-d", "--", "."], Call::long())?;
    if !cleaned.succeeded() {
        return Err(after_recovery(
            &failed("remove files added after the checkpoint", &cleaned),
            recovery_ref,
        ));
    }
    Ok(RestoreResult {
        restored: count_u32(restored),
        recovery_ref: recovery_ref.to_owned(),
    })
}

fn after_recovery(error: &ThreadGitError, recovery_ref: &str) -> ThreadGitError {
    ThreadGitError::new(
        error.code,
        format!(
            "{} The files from before the restore are saved as {recovery_ref}.",
            error.message
        ),
    )
}

// ---------------------------------------------------------------------------
// Diffs

#[derive(Clone, Debug)]
struct Change {
    status: DiffFileStatus,
    path: String,
    old_path: Option<String>,
    added: u32,
    removed: u32,
    binary: bool,
}

enum Patch {
    Text(String),
    Large,
}

fn diff_with(
    cwd: &Path,
    request: &DiffRequest,
    limits: DiffLimits,
) -> Result<ThreadDiff, ThreadGitError> {
    let repo = require_repo(cwd)?;
    let base = resolve_commit(&repo, &request.base)?;
    let (head, head_label) = match &request.head {
        Some(head) => {
            let commit = resolve_commit(&repo, head)?;
            (commit.clone(), commit)
        }
        None => (working_tree(&repo)?, WORKING_TREE.to_owned()),
    };
    let mut changes = match &request.paths {
        None => changes_between(&repo, &base, &head, &[])?,
        Some(paths) => {
            let specs = paths
                .iter()
                .filter_map(|path| literal_spec(path))
                .collect::<Vec<_>>();
            // No path left is no file: the filter never widens to the tree.
            if specs.is_empty() {
                Vec::new()
            } else {
                changes_between(&repo, &base, &head, &specs)?
            }
        }
    };
    let mut truncated = changes.len() > limits.files;
    changes.truncate(limits.files);
    let mut files = changes.into_iter().map(classified).collect::<Vec<_>>();
    let patchable = files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.omitted.is_none())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut budget = limits.total_patch_bytes;
    'batches: for batch in patchable.chunks(PATCH_WORKERS) {
        let patches = batch_patches(&repo, &base, &head, &files, batch, limits);
        for (&index, patch) in batch.iter().zip(patches) {
            match patch? {
                Patch::Large => files[index].omitted = Some(DiffOmitted::Large),
                Patch::Text(text) => {
                    if text.len() > budget {
                        truncated = true;
                        break 'batches;
                    }
                    budget -= text.len();
                    files[index].patch = Some(text);
                }
            }
        }
    }
    Ok(ThreadDiff {
        scope: request.scope,
        base,
        head: head_label,
        files,
        truncated,
    })
}

/// One patch per file in `batch`, each from its own bounded git child, run
/// side by side and answered in `batch` order.
fn batch_patches(
    repo: &Path,
    base: &str,
    head: &str,
    files: &[DiffFile],
    batch: &[usize],
    limits: DiffLimits,
) -> Vec<Result<Patch, ThreadGitError>> {
    std::thread::scope(|scope| {
        let workers = batch
            .iter()
            .map(|&index| {
                let file = &files[index];
                scope.spawn(move || {
                    file_patch(
                        repo,
                        base,
                        head,
                        &file.path,
                        file.old_path.as_deref(),
                        limits.file_patch_bytes,
                    )
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// A listed file before its patch: credential-shaped paths (either side of a
/// rename) are `sensitive`, binary content or binary-named paths `binary`.
fn classified(change: Change) -> DiffFile {
    let sensitive = is_search_excluded_path(&change.path, false)
        || change
            .old_path
            .as_deref()
            .is_some_and(|path| is_search_excluded_path(path, false));
    let binary = change.binary || is_likely_binary_path(&change.path);
    let omitted = if sensitive {
        Some(DiffOmitted::Sensitive)
    } else if binary {
        Some(DiffOmitted::Binary)
    } else {
        None
    };
    DiffFile {
        path: change.path,
        old_path: change.old_path,
        status: change.status,
        added: change.added,
        removed: change.removed,
        binary,
        patch: None,
        omitted,
    }
}

fn file_patch(
    repo: &Path,
    base: &str,
    head: &str,
    path: &str,
    old_path: Option<&str>,
    maximum: usize,
) -> Result<Patch, ThreadGitError> {
    let mut specs = vec![format!(":(literal){path}")];
    if let Some(old_path) = old_path {
        specs.push(format!(":(literal){old_path}"));
    }
    let mut arguments = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--unified=3",
        "-M",
        "--submodule=short",
        base,
        head,
        "--",
    ];
    arguments.extend(specs.iter().map(String::as_str));
    let output = git(
        repo,
        &arguments,
        Call {
            max_output: maximum.saturating_add(1),
            ..Call::read()
        },
    )?;
    if output.truncated || output.stdout.len() > maximum {
        return Ok(Patch::Large);
    }
    if output.code != Some(0) {
        return Err(failed(&format!("read the changes to {path}"), &output));
    }
    Ok(Patch::Text(
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// Every changed file between two tree-ish objects, renames paired, in git's
/// order, with line counts (binary files count 0/0). Non-empty `specs` (git
/// pathspecs from the repository root) limit it to the files they match.
fn changes_between(
    repo: &Path,
    from: &str,
    to: &str,
    specs: &[String],
) -> Result<Vec<Change>, ThreadGitError> {
    let mut arguments = vec![
        "diff",
        "--raw",
        "--numstat",
        "-z",
        "-M",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
        from,
        to,
    ];
    if !specs.is_empty() {
        arguments.push("--");
        arguments.extend(specs.iter().map(String::as_str));
    }
    let output = git(repo, &arguments, Call::listing())?;
    if !output.succeeded() {
        return Err(failed("list the changed files", &output));
    }
    Ok(parse_changes(&output.stdout))
}

/// `git diff --raw --numstat -z` prints every raw record (`:<modes> <ids>
/// <status>` then one path, or two for a rename or copy) before every numstat
/// record (`<added>\t<removed>\t<path>`, or an empty path then both paths).
/// Both sections list the files in the same order.
fn parse_changes(stdout: &[u8]) -> Vec<Change> {
    let mut tokens = stdout.split(|byte| *byte == 0);
    let mut changes = Vec::<Change>::new();
    let mut counted = 0_usize;
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        if token.starts_with(b":") {
            let meta = String::from_utf8_lossy(token);
            let letter = meta
                .rsplit(' ')
                .next()
                .and_then(|status| status.chars().next())
                .unwrap_or('M');
            let first = tokens.next().map(lossy).unwrap_or_default();
            let (path, old_path) = if matches!(letter, 'R' | 'C') {
                (tokens.next().map(lossy).unwrap_or_default(), Some(first))
            } else {
                (first, None)
            };
            changes.push(Change {
                status: match letter {
                    'A' | 'C' => DiffFileStatus::Added,
                    'D' => DiffFileStatus::Deleted,
                    'R' => DiffFileStatus::Renamed,
                    _ => DiffFileStatus::Modified,
                },
                path,
                old_path: old_path.filter(|_| letter == 'R'),
                added: 0,
                removed: 0,
                binary: false,
            });
            continue;
        }
        let record = lossy(token);
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            tokens.next();
            tokens.next().map(lossy).unwrap_or_default()
        } else {
            path.to_owned()
        };
        let position = if changes
            .get(counted)
            .is_some_and(|change| change.path == path)
        {
            Some(counted)
        } else {
            changes.iter().position(|change| change.path == path)
        };
        counted += 1;
        let Some(change) = position.and_then(|position| changes.get_mut(position)) else {
            continue;
        };
        match (added.parse::<u32>(), removed.parse::<u32>()) {
            (Ok(added), Ok(removed)) => {
                change.added = added;
                change.removed = removed;
            }
            _ => change.binary = true,
        }
    }
    changes
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Repository reads

fn toplevel(path: &Path, call: Call<'_>) -> Result<Option<PathBuf>, ThreadGitError> {
    if !path.is_dir() {
        return Ok(None);
    }
    let output = git(path, &["rev-parse", "--show-toplevel"], call)?;
    if !output.succeeded() {
        return Ok(None);
    }
    let top = output.raw_line();
    Ok((!top.is_empty()).then(|| PathBuf::from(top)))
}

fn require_repo(cwd: &Path) -> Result<PathBuf, ThreadGitError> {
    repo_root(cwd)?.ok_or_else(|| {
        ThreadGitError::new(
            ThreadGitError::NOT_REPOSITORY_CODE,
            format!("{} is not inside a git repository.", cwd.display()),
        )
    })
}

fn head_commit(repo: &Path, call: Call<'_>) -> Result<Option<String>, ThreadGitError> {
    let output = git(
        repo,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        call,
    )?;
    match output.code {
        Some(0) if !output.truncated => Ok(Some(output.line())),
        Some(1) => Ok(None),
        _ => Err(failed("read HEAD", &output)),
    }
}

fn resolve_commit(repo: &Path, revision: &str) -> Result<String, ThreadGitError> {
    validate_revision(revision)?;
    let spec = format!("{revision}^{{commit}}");
    let output = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &spec],
        Call::read(),
    )?;
    let commit = output.line();
    if !output.succeeded() || commit.is_empty() {
        return Err(invalid(format!(
            "{revision} does not name a commit in {}.",
            repo.display()
        )));
    }
    Ok(commit)
}

fn symbolic_ref(repo: &Path, name: &str) -> Result<Option<String>, ThreadGitError> {
    let output = git(repo, &["symbolic-ref", "--quiet", name], Call::read())?;
    Ok(output
        .succeeded()
        .then(|| output.line())
        .filter(|target| !target.is_empty()))
}

// ---------------------------------------------------------------------------
// Validation

fn invalid(message: impl Into<String>) -> ThreadGitError {
    ThreadGitError::new(ThreadGitError::INVALID_CODE, message)
}

fn failed(action: &str, output: &Output) -> ThreadGitError {
    let detail = output.detail();
    ThreadGitError::new(
        ThreadGitError::FAILED_CODE,
        if detail.is_empty() {
            format!("git could not {action}.")
        } else {
            format!("git could not {action} ({detail}).")
        },
    )
}

fn text_path(path: &Path) -> Result<&str, ThreadGitError> {
    path.to_str()
        .ok_or_else(|| invalid(format!("{} is not a UTF-8 path.", path.display())))
}

fn count_u32(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A revision handed to git as one argument: never an option, never a
/// second line.
fn validate_revision(revision: &str) -> Result<(), ThreadGitError> {
    if revision.is_empty()
        || revision.len() > 1024
        || revision.starts_with('-')
        || revision.chars().any(char::is_control)
    {
        return Err(invalid(format!(
            "{revision:?} is not a usable git revision."
        )));
    }
    Ok(())
}

/// git's ref-name rules, checked before git runs so a malformed name can
/// never reach `update-ref`.
fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && name != "@"
        && name
            .bytes()
            .all(|byte| byte > b' ' && byte != 0x7f && !b"~^:?*[\\".contains(&byte))
        && name.split('/').all(|component| {
            !component.is_empty()
                && !component.starts_with('.')
                && component.strip_suffix(".lock").is_none()
        })
}

/// Only refs under `refs/interface/` are ever written or deleted here.
fn validate_thread_ref(refname: &str) -> Result<(), ThreadGitError> {
    let nested = refname
        .strip_prefix(THREAD_REF_ROOT)
        .is_some_and(|rest| !rest.is_empty());
    if !nested || !valid_ref_name(refname) {
        return Err(invalid(format!(
            "{refname:?} is not a thread checkpoint ref; Interface only writes refs under {THREAD_REF_ROOT}."
        )));
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), ThreadGitError> {
    if branch.starts_with('-') || !valid_ref_name(&format!("refs/heads/{branch}")) {
        return Err(invalid(format!("{branch:?} is not a usable branch name.")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The bounded git child

#[derive(Clone, Copy)]
struct Call<'a> {
    timeout: Duration,
    max_output: usize,
    /// `GIT_INDEX_FILE`: the private checkpoint index.
    index: Option<&'a Path>,
    /// The fixed checkpoint author and committer.
    identity: bool,
    /// Only `git worktree add` runs the repository's hooks.
    hooks: bool,
}

impl<'a> Call<'a> {
    const fn read() -> Self {
        Self {
            timeout: GIT_TIMEOUT,
            max_output: METADATA_BYTES,
            index: None,
            identity: false,
            hooks: false,
        }
    }

    const fn listing() -> Self {
        Self {
            max_output: LISTING_BYTES,
            ..Self::read()
        }
    }

    /// Checkouts, restores and removals write the whole project.
    const fn long() -> Self {
        Self {
            timeout: WORKTREE_TIMEOUT,
            max_output: LISTING_BYTES,
            ..Self::read()
        }
    }

    /// `git worktree add`, with the repository's own checkout hooks.
    const fn checkout() -> Self {
        Self {
            hooks: true,
            ..Self::long()
        }
    }

    fn within(self, timeout: Duration) -> Self {
        Self {
            timeout: self.timeout.min(timeout),
            ..self
        }
    }

    const fn indexed(self, index: &'a Path) -> Self {
        Self {
            index: Some(index),
            ..self
        }
    }
}

struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    truncated: bool,
}

impl Output {
    fn succeeded(&self) -> bool {
        self.code == Some(0) && !self.truncated
    }

    /// The first line, trimmed: an object id or a ref name.
    fn line(&self) -> String {
        String::from_utf8_lossy(&self.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    /// The first line without its newline only: a path may end in a space.
    fn raw_line(&self) -> String {
        let end = self
            .stdout
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(self.stdout.len());
        lossy(&self.stdout[..end])
    }

    /// git's own explanation, without its `fatal:`/`error:` lead.
    fn detail(&self) -> String {
        let line = self
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        ["fatal:", "error:"]
            .iter()
            .find_map(|lead| line.strip_prefix(lead))
            .unwrap_or(line)
            .trim()
            .to_owned()
    }
}

fn git(dir: &Path, args: &[&str], call: Call<'_>) -> Result<Output, ThreadGitError> {
    git_with_input(dir, args, call, None)
}

fn git_with_input(
    dir: &Path,
    args: &[&str],
    call: Call<'_>,
    stdin: Option<Vec<u8>>,
) -> Result<Output, ThreadGitError> {
    let started = Instant::now();
    let mut command = git_command(dir);
    // A call that runs the repository's hooks (an explicit commit, push or
    // new worktree) runs the repository's own code anyway. Every other call
    // runs none of the programs the repository's settings name: they are
    // listed first, and the call runs with them overridden (`git_hardening`).
    if !call.hooks {
        let mut probe = git_command(dir);
        probe.args(git_hardening::PROBE_ARGS);
        let listed = run_bounded_command(probe, None, call.timeout, git_hardening::PROBE_MAX_BYTES);
        bounded(dir, args, call, &listed)?;
        let overrides = (git_hardening::probe_answered(listed.exit_code)
            && !listed.output_truncated)
            .then(|| git_hardening::Overrides::from_listing(&listed.stdout))
            .flatten();
        let Some(overrides) = overrides else {
            return Ok(Output {
                code: Some(listed.exit_code.filter(|code| *code > 1).unwrap_or(128)),
                stdout: Vec::new(),
                stderr: format!(
                    "{} {}",
                    git_hardening::PROBE_REFUSED,
                    String::from_utf8_lossy(&listed.stderr).trim()
                ),
                truncated: false,
            });
        };
        overrides.apply(&mut command);
        command.args(git_hardening::STATIC_ARGS);
    }
    if let Some(index) = call.index {
        command.env("GIT_INDEX_FILE", index);
    }
    if call.identity {
        command.envs(CHECKPOINT_IDENTITY);
    }
    command.args([
        "--no-pager",
        "-c",
        "core.quotepath=false",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.splitIndex=false",
        "-c",
        "gc.auto=0",
        "-c",
        "maintenance.auto=false",
    ]);
    if !call.hooks {
        command.args(["-c", "core.hooksPath=/dev/null"]);
    }
    command.args(args);
    let output = run_bounded_command(
        command,
        stdin,
        call.timeout.saturating_sub(started.elapsed()),
        call.max_output,
    );
    bounded(dir, args, call, &output)?;
    Ok(Output {
        code: output.exit_code,
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        truncated: output.output_truncated,
    })
}

/// `git` in `dir`, without the inherited `GIT_*` variables (an inherited
/// `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_CONFIG_PARAMETERS` … would point it at
/// another repository, index or configuration), never prompting.
fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    // Nor does git, or a hook it runs, get Interface's or a provider's secrets.
    ship::filter_credentials(&mut command);
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .current_dir(dir);
    command
}

/// A git child that could not start, or outlived `call`'s deadline, as the
/// error `args` answers with.
fn bounded(
    dir: &Path,
    args: &[&str],
    call: Call<'_>,
    output: &BoundedOutput,
) -> Result<(), ThreadGitError> {
    if output.unavailable {
        return Err(ThreadGitError::new(
            ThreadGitError::UNAVAILABLE_CODE,
            format!(
                "Interface could not run git in {} ({}).",
                dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    if output.timed_out {
        return Err(ThreadGitError::new(
            ThreadGitError::TIMEOUT_CODE,
            format!(
                "git did not finish `{}` in {} within {} seconds. If this project is in iCloud Drive, files git needs may still be downloading; try again once they have.",
                args.first().copied().unwrap_or("git"),
                dir.display(),
                call.timeout.as_secs()
            ),
        ));
    }
    Ok(())
}

#[path = "thread_ship.rs"]
pub mod ship;

#[path = "thread_commit.rs"]
pub mod commit;

#[path = "thread_ship_text.rs"]
pub mod ship_text;

#[cfg(test)]
#[path = "thread_git_tests.rs"]
mod tests;
