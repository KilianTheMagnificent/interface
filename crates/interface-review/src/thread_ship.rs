//! Shipping a thread's work (protocol 37): its checkout's branch and
//! changes, commit, a new branch, push, and the GitHub CLI (`gh`) calls
//! behind a pull request, its checks, its review threads and its merge.
//! Synchronous; the host calls these on its blocking pool.
//!
//! Every child is bounded (deadline, output cap, process-group teardown) and
//! credential-filtered: git runs as every `thread_git` call does (inherited
//! `GIT_*` removed, no terminal prompts), and both git and `gh` lose the
//! app's own and the providers' secrets from their environment
//! ([`credential_variable`]); what they print back is passed through
//! [`redact_credentials`] before it reaches the owner or an agent. Commit
//! and push run the repository's own hooks, as a terminal commit would.
//!
//! Nothing here retries. An effect whose outcome is unknown is re-read
//! (HEAD, the remote branch, the pull request) and answered as what the
//! re-read found, or as unknown when the re-read failed too.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use std::collections::BTreeSet;

use interface_contracts::threads::{
    CheckBucket, ChecksState, DiffFileStatus, DiffSide, LeftOutReason, MergeMethod,
    PullRequestState, ReviewComment, ShipCheck, ShipChecks, ShipFile, ShipLeftOut, ShipPullRequest,
};
use interface_workspace::is_credential_path;
use regex::Regex;
use serde_json::Value;

use super::{
    Call, Output, ThreadGitError, failed, git, git_with_input, lossy, require_repo, symbolic_ref,
    validate_branch,
};
use crate::process::run_bounded_command;

/// `gh` gets this long for a read.
const GH_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// ... and this long to create or merge a pull request.
const GH_WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const GH_OUTPUT_BYTES: usize = 1024 * 1024;
/// A failed job's log is read up to this much, of which the tail is kept.
const LOG_BYTES: usize = 4 * 1024 * 1024;
/// The tail of one failed job's log a prompt quotes.
pub const LOG_EXCERPT_LINES: usize = 80;
pub const LOG_EXCERPT_BYTES: usize = 6 * 1024;
/// Checks a status names one by one; the rest are counted.
pub const MAX_LISTED_CHECKS: usize = 20;
/// Review threads read per pull request, and the text kept of each.
pub const MAX_REVIEW_THREADS: usize = 100;
pub const MAX_REVIEW_COMMENT_BYTES: usize = 4 * 1024;
pub const MAX_REVIEW_CODE_LINES: usize = 12;
/// Changed files a status lists; past it, a commit is refused.
pub const MAX_SHIP_FILES: usize = 1_000;
/// Remote reads (`ls-remote`) answer within this.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
/// A remote read runs git's own upload-pack: `remote.<name>.uploadpack` is a
/// command, one the repository's settings may choose, that a remote on this
/// disk runs here, and git keeps its first value, so only this flag outranks it.
const UPLOAD_PACK: &str = "--upload-pack=git-upload-pack";

// ---------------------------------------------------------------------------
// Credentials

/// Whether an environment variable is a secret neither git nor `gh` needs:
/// Interface's own, a provider's, or any `*_API_KEY` / `*_SECRET` /
/// `*_PASSWORD`. GitHub's own tokens (`GH_*`, `GITHUB_*`) stay: `gh` and a
/// git credential helper may sign in with them.
#[must_use]
pub fn credential_variable(name: &str) -> bool {
    const PREFIXES: [&str; 13] = [
        "INTERFACE_",
        "IFACE_",
        "ANTHROPIC_",
        "OPENAI_",
        "XAI_",
        "GEMINI_",
        "GOOGLE_API",
        "CLAUDE",
        "CODEX_",
        "GROK_",
        "QWEN_",
        "OPENCODE",
        "AWS_SECRET",
    ];
    const SUFFIXES: [&str; 5] = ["_API_KEY", "_SECRET", "_SECRET_KEY", "_PASSWORD", "_PASSWD"];
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("GH_") || upper.starts_with("GITHUB_") {
        return false;
    }
    PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
        || SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

/// Removes [`credential_variable`]s from what `command` inherits.
pub(crate) fn filter_credentials(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(credential_variable) {
            command.env_remove(name);
        }
    }
}

static SECRET_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // Private keys, whole blocks.
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            "[redacted private key]",
        ),
        // GitHub, Slack, AWS, Google and `sk-` API tokens.
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}", "[redacted]"),
        (r"\bgithub_pat_[A-Za-z0-9_]{20,}", "[redacted]"),
        (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "[redacted]"),
        (r"\b(AKIA|ASIA)[0-9A-Z]{16}\b", "[redacted]"),
        (r"\bAIza[0-9A-Za-z_-]{35}", "[redacted]"),
        (r"\bsk-[A-Za-z0-9_-]{20,}", "[redacted]"),
        // Credentials inside a URL.
        (r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@", "${1}[redacted]@"),
        // `Authorization: Bearer …` and `token=…`-style assignments.
        (
            r"(?i)\b(authorization:\s*(?:bearer|token|basic)\s+)[^\s'\x22]+",
            "${1}[redacted]",
        ),
        (
            r"(?i)\b((?:password|passwd|secret|token|api[_-]?key|access[_-]?key|client[_-]?secret)\s*[=:]\s*)[^\s'\x22,;]+",
            "${1}[redacted]",
        ),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (Regex::new(pattern).expect("a valid pattern"), replacement))
    .collect()
});

/// `text` with credential-shaped values replaced by `[redacted]`: tokens of
/// the known shapes, private key blocks, a URL's user and password, and the
/// value of an `Authorization` header or a `password=`/`token:` assignment.
#[must_use]
pub fn redact_credentials(text: &str) -> String {
    let mut redacted = text.to_owned();
    for (pattern, replacement) in SECRET_PATTERNS.iter() {
        if pattern.is_match(&redacted) {
            redacted = pattern.replace_all(&redacted, *replacement).into_owned();
        }
    }
    redacted
}

// ---------------------------------------------------------------------------
// The checkout

/// One changed file in a checkout, relative to the repository root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShipChange {
    pub path: String,
    pub status: DiffFileStatus,
    /// Never in the index yet: committing it adds it first.
    pub untracked: bool,
    /// Credential-shaped (`.env`, keys): never committed by Interface.
    pub credential: bool,
}

/// What git says about a thread's checkout, for shipping it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShipGitState {
    pub repo: PathBuf,
    /// `None`: a detached HEAD.
    pub branch: Option<String>,
    /// `None`: no commit yet.
    pub head: Option<String>,
    /// The remote the branch pushes to: its upstream's, else `origin`, else
    /// the only remote.
    pub remote: Option<String>,
    /// `<remote>/<branch>` when the branch tracks one.
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// From `refs/remotes/<remote>/HEAD`, or asked of the remote.
    pub default_branch: Option<String>,
    pub changes: Vec<ShipChange>,
    /// More changes than [`MAX_SHIP_FILES`].
    pub changes_truncated: bool,
    /// A merge is half done: some file is unmerged.
    pub conflicted: bool,
}

/// The checkout `cwd` is in: branch, remote, upstream counts, default branch
/// and changed files. `ask_remote` lets it ask the remote for its default
/// branch when nothing local names it.
///
/// # Errors
///
/// Fails when `cwd` is not in a repository git can read.
pub fn ship_state(cwd: &Path, ask_remote: bool) -> Result<ShipGitState, ThreadGitError> {
    let mut state = branch_state_with(cwd, ask_remote)?;
    let repo = &state.repo;
    let head = super::head_commit(repo, Call::read())?;
    let upstream = if state.branch.is_some() {
        let output = git(
            repo,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
            Call::read(),
        )?;
        output
            .succeeded()
            .then(|| output.line())
            .filter(|name| !name.is_empty())
    } else {
        None
    };
    let (behind, ahead) = match &upstream {
        Some(upstream) => counts(repo, upstream)?,
        None => (None, None),
    };
    let (changes, changes_truncated, conflicted) = changes(repo)?;
    state.head = head;
    state.upstream = upstream;
    state.ahead = ahead;
    state.behind = behind;
    state.changes = changes;
    state.changes_truncated = changes_truncated;
    state.conflicted = conflicted;
    Ok(state)
}

/// The checkout's repository root, current branch, push remote and local
/// default branch, without scanning the working tree or upstream commit counts.
///
/// # Errors
///
/// Fails when `cwd` is not in a repository git can read.
pub fn branch_state(cwd: &Path) -> Result<ShipGitState, ThreadGitError> {
    branch_state_with(cwd, false)
}

fn branch_state_with(cwd: &Path, ask_remote: bool) -> Result<ShipGitState, ThreadGitError> {
    let repo = require_repo(cwd)?;
    let branch = symbolic_ref(&repo, "HEAD")?
        .and_then(|target| target.strip_prefix("refs/heads/").map(ToOwned::to_owned));
    let remotes = list_remotes(&repo)?;
    let remote = match remotes.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        _ => {
            let configured = match &branch {
                Some(branch) => config_value(&repo, &format!("branch.{branch}.remote"))?,
                None => None,
            };
            configured
                .filter(|remote| remotes.contains(remote))
                .or_else(|| remotes.iter().find(|remote| *remote == "origin").cloned())
        }
    };
    let default_branch = match &remote {
        Some(remote) => default_branch(&repo, remote, ask_remote)?,
        None => None,
    };
    Ok(ShipGitState {
        repo,
        branch,
        remote,
        default_branch,
        ..ShipGitState::default()
    })
}

pub(super) fn working_tree_commit_blocked(repo: &Path) -> Result<Option<String>, ThreadGitError> {
    let (_, changes_truncated, conflicted) = changes(repo)?;
    Ok(commit_blocked(&ShipGitState {
        changes_truncated,
        conflicted,
        ..ShipGitState::default()
    }))
}

fn list_remotes(repo: &Path) -> Result<Vec<String>, ThreadGitError> {
    let output = git(repo, &["remote"], Call::read())?;
    if !output.succeeded() {
        return Err(failed("list the remotes", &output));
    }
    Ok(lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn config_value(repo: &Path, name: &str) -> Result<Option<String>, ThreadGitError> {
    let output = git(repo, &["config", "--get", name], Call::read())?;
    Ok(output
        .succeeded()
        .then(|| output.line())
        .filter(|value| !value.is_empty()))
}

/// `(behind, ahead)` of HEAD against `upstream`.
fn counts(repo: &Path, upstream: &str) -> Result<(Option<u32>, Option<u32>), ThreadGitError> {
    let range = format!("{upstream}...HEAD");
    let output = git(
        repo,
        &["rev-list", "--left-right", "--count", &range],
        Call::read(),
    )?;
    if !output.succeeded() {
        return Ok((None, None));
    }
    let line = output.line();
    let mut numbers = line.split_whitespace().map(str::parse::<u32>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(behind)), Some(Ok(ahead))) => Ok((Some(behind), Some(ahead))),
        _ => Ok((None, None)),
    }
}

fn default_branch(
    repo: &Path,
    remote: &str,
    ask_remote: bool,
) -> Result<Option<String>, ThreadGitError> {
    validate_remote(remote)?;
    let prefix = format!("refs/remotes/{remote}/");
    if let Some(name) = symbolic_ref(repo, &format!("{prefix}HEAD"))?
        .and_then(|target| target.strip_prefix(&prefix).map(ToOwned::to_owned))
        .filter(|name| !name.is_empty())
    {
        return Ok(Some(name));
    }
    if !ask_remote {
        return Ok(None);
    }
    let output = git(
        repo,
        &["ls-remote", UPLOAD_PACK, "--symref", remote, "HEAD"],
        Call::read().within(REMOTE_TIMEOUT),
    );
    let Ok(output) = output else {
        return Ok(None);
    };
    Ok(output
        .succeeded()
        .then(|| {
            lossy(&output.stdout).lines().find_map(|line| {
                line.strip_prefix("ref: refs/heads/")
                    .and_then(|rest| rest.split('\t').next())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
        .flatten())
}

/// Every changed file (untracked included, ignored excluded), whether the
/// list was cut at [`MAX_SHIP_FILES`], and whether a merge left conflicts.
fn changes(repo: &Path) -> Result<(Vec<ShipChange>, bool, bool), ThreadGitError> {
    let output = git(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
        Call::listing(),
    )?;
    if !output.succeeded() {
        return Err(failed("read the working tree's status", &output));
    }
    let mut changes = Vec::new();
    let mut conflicted = false;
    let mut truncated = false;
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.len() < 4 {
            continue;
        }
        let (x, y) = (record[0], record[1]);
        let path = lossy(&record[3..]);
        if matches!((x, y), (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D')) {
            conflicted = true;
        }
        let untracked = x == b'?';
        let status = if untracked || x == b'A' {
            DiffFileStatus::Added
        } else if x == b'D' || y == b'D' {
            DiffFileStatus::Deleted
        } else {
            DiffFileStatus::Modified
        };
        if changes.len() >= MAX_SHIP_FILES {
            truncated = true;
            break;
        }
        changes.push(ShipChange {
            credential: is_credential_path(&path),
            path,
            status,
            untracked,
        });
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((changes, truncated, conflicted))
}

/// Changed files a status lists as left out, at most.
pub const MAX_LEFT_OUT: usize = 200;

/// What Commit stages from a checkout's changes, and what it leaves out.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommitSelection {
    pub files: Vec<ShipChange>,
    pub left_out: Vec<ShipLeftOut>,
}

impl CommitSelection {
    /// The staged files as the wire lists them.
    #[must_use]
    pub fn ship_files(&self) -> Vec<ShipFile> {
        self.files
            .iter()
            .map(|change| ShipFile {
                path: change.path.clone(),
                status: change.status,
            })
            .collect()
    }

    /// The staged files' paths, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|change| change.path.clone())
            .collect()
    }

    /// The staged files git does not track yet.
    #[must_use]
    pub fn untracked(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|change| change.untracked)
            .map(|change| change.path.clone())
            .collect()
    }
}

/// The changes Commit stages: every one for a worktree thread (`touched`
/// `None`), and for a Local thread only those its turns' edits named — the
/// checkout is the owner's, and other sessions may work in it. A
/// credential-shaped file is never staged; both kinds of leaving out are
/// listed, at most [`MAX_LEFT_OUT`].
#[must_use]
pub fn commit_selection(
    changes: &[ShipChange],
    touched: Option<&BTreeSet<String>>,
) -> CommitSelection {
    let mut selection = CommitSelection::default();
    for change in changes {
        let reason = if change.credential {
            Some(LeftOutReason::Credential)
        } else if touched.is_some_and(|touched| !touched.contains(&change.path)) {
            Some(LeftOutReason::NotThisThread)
        } else {
            None
        };
        match reason {
            None => selection.files.push(change.clone()),
            Some(reason) if selection.left_out.len() < MAX_LEFT_OUT => {
                selection.left_out.push(ShipLeftOut {
                    path: change.path.clone(),
                    reason,
                });
            }
            Some(_) => {}
        }
    }
    selection
}

/// Why this checkout may not be pushed, if it may not: a detached HEAD, no
/// commit, no remote, or the repository's default branch (or, when that is
/// unknown, `main` or `master`) — Interface never pushes to it.
#[must_use]
pub fn push_blocked(state: &ShipGitState) -> Option<String> {
    let Some(branch) = state.branch.as_deref() else {
        return Some(
            "This checkout isn't on a branch (a detached HEAD). Switch to a branch first.".into(),
        );
    };
    if state.head.is_none() {
        return Some("There is no commit to push yet.".into());
    }
    if state.remote.is_none() {
        return Some("This repository has no remote to push to.".into());
    }
    on_default_branch(state).then(|| {
        format!(
            "This checkout is on {branch}, the default branch. Interface won't push to it: create a branch for this work first."
        )
    })
}

/// The checkout is on the repository's default branch (or, when that is
/// unknown, on `main` or `master`).
#[must_use]
pub fn on_default_branch(state: &ShipGitState) -> bool {
    let Some(branch) = state.branch.as_deref() else {
        return false;
    };
    match state.default_branch.as_deref() {
        Some(default) => branch == default,
        None => matches!(branch, "main" | "master"),
    }
}

/// Why Commit cannot run in this checkout now, if it cannot.
#[must_use]
pub fn commit_blocked(state: &ShipGitState) -> Option<String> {
    if state.conflicted {
        Some("A merge is in progress with conflicts. Resolve and finish it first.".into())
    } else if state.changes_truncated {
        Some(format!(
            "More than {MAX_SHIP_FILES} files changed. Commit them from a terminal."
        ))
    } else {
        None
    }
}

impl GhPullRequest {
    /// As the wire carries it.
    #[must_use]
    pub fn ship(&self) -> ShipPullRequest {
        ShipPullRequest {
            number: self.number,
            url: self.url.clone(),
            state: self.state,
            title: self.title.clone(),
            base: self.base.clone(),
            draft: self.draft,
        }
    }
}

/// The checks summed up: failing if any failed, else pending if any still
/// run, else passing; `None` without checks. Lists at most
/// [`MAX_LISTED_CHECKS`] failing ones.
#[must_use]
pub fn checks_summary(checks: &[GhCheck]) -> ShipChecks {
    let count = |bucket: CheckBucket| {
        u32::try_from(checks.iter().filter(|check| check.bucket == bucket).count())
            .unwrap_or(u32::MAX)
    };
    let (passed, failed, pending) = (
        count(CheckBucket::Pass),
        count(CheckBucket::Fail),
        count(CheckBucket::Pending),
    );
    let skipped = count(CheckBucket::Skipping).saturating_add(count(CheckBucket::Cancel));
    let state = if failed > 0 {
        ChecksState::Failing
    } else if pending > 0 {
        ChecksState::Pending
    } else if passed > 0 || skipped > 0 {
        ChecksState::Passing
    } else {
        ChecksState::None
    };
    ShipChecks {
        state,
        passed,
        failed,
        pending,
        skipped,
        failing: checks
            .iter()
            .filter(|check| check.bucket == CheckBucket::Fail)
            .take(MAX_LISTED_CHECKS)
            .map(|check| ShipCheck {
                name: check.name.clone(),
                workflow: check.workflow.clone(),
                bucket: check.bucket,
                link: check.link.clone(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Commit, branch and push

/// How an effect ended. `Unknown` only when re-reading its result failed
/// too: the caller never retries it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShipEffect<T> {
    Done(T),
    /// It did not happen: git or `gh` refused, and a re-read agrees.
    Refused(String),
    Unknown(String),
}

/// Commits exactly `paths` (repository-relative, as a status listed them)
/// with `message`, leaving every other staged or unstaged change as it was.
/// Untracked paths are added first and taken back out of the index if the
/// commit does not happen. The repository's hooks run.
///
/// # Errors
///
/// Fails before anything changes when a path is not a plain
/// repository-relative path or HEAD cannot be read.
pub fn commit_paths(
    cwd: &Path,
    paths: &[String],
    untracked: &[String],
    message: &str,
) -> Result<ShipEffect<String>, ThreadGitError> {
    let repo = require_repo(cwd)?;
    if paths.is_empty() || message.trim().is_empty() {
        return Ok(ShipEffect::Refused(
            "There is nothing to commit, or no message.".into(),
        ));
    }
    let specs = pathspecs(paths)?;
    let new_specs = pathspecs(untracked)?;
    let before = super::head_commit(&repo, Call::read())?;
    if !new_specs.is_empty() {
        let added = git_with_input(
            &repo,
            &["add", "--pathspec-from-file=-", "--pathspec-file-nul", "--"],
            Call::long(),
            Some(new_specs.clone()),
        )?;
        if !added.succeeded() {
            return Ok(ShipEffect::Refused(redact_credentials(
                &failed("add the new files", &added).message,
            )));
        }
    }
    let committed = git_with_input(
        &repo,
        &[
            "commit",
            "--only",
            "-m",
            message,
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
            "--",
        ],
        Call::checkout(),
        Some(specs),
    );
    let after = super::head_commit(&repo, Call::read());
    let moved = match (&after, &before) {
        (Ok(Some(after)), before) => before.as_ref() != Some(after),
        (Ok(None), _) => false,
        (Err(_), _) => {
            return Ok(ShipEffect::Unknown(
                "Interface couldn't read the branch after committing, so it can't tell whether the commit was made. Check again before committing.".into(),
            ));
        }
    };
    if moved {
        return Ok(ShipEffect::Done(after.ok().flatten().unwrap_or_default()));
    }
    // Nothing was committed: the new files go back out of the index.
    if !new_specs.is_empty() {
        let _ = git_with_input(
            &repo,
            &[
                "rm",
                "--cached",
                "-q",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
                "--",
            ],
            Call::long(),
            Some(new_specs),
        );
    }
    Ok(ShipEffect::Refused(match committed {
        Ok(output) => redact_credentials(&failed("commit", &output).message),
        Err(error) => redact_credentials(&error.message),
    }))
}

/// NUL-separated literal pathspecs, for `--pathspec-from-file`.
fn pathspecs(paths: &[String]) -> Result<Vec<u8>, ThreadGitError> {
    let mut bytes = Vec::new();
    for path in paths {
        let Some(spec) = super::literal_spec(path) else {
            return Err(ThreadGitError::new(
                ThreadGitError::INVALID_CODE,
                format!("{path:?} is not a path inside the repository."),
            ));
        };
        bytes.extend_from_slice(spec.as_bytes());
        bytes.push(0);
    }
    Ok(bytes)
}

/// Creates `branch` at HEAD and switches the checkout to it, carrying every
/// uncommitted change along (`git switch -c`).
///
/// # Errors
///
/// Fails before git runs on a name git would refuse.
pub fn create_branch(cwd: &Path, branch: &str) -> Result<ShipEffect<String>, ThreadGitError> {
    validate_branch(branch)?;
    let repo = require_repo(cwd)?;
    let reference = format!("refs/heads/{branch}");
    if git(
        &repo,
        &["rev-parse", "--verify", "--quiet", &reference],
        Call::read(),
    )?
    .succeeded()
    {
        return Ok(ShipEffect::Refused(format!(
            "A branch named {branch} already exists. Choose another name."
        )));
    }
    let switched = git(&repo, &["switch", "-c", branch], Call::long());
    match symbolic_ref(&repo, "HEAD") {
        Ok(Some(target)) if target == reference => Ok(ShipEffect::Done(branch.to_owned())),
        Ok(_) => Ok(ShipEffect::Refused(match switched {
            Ok(output) => redact_credentials(&failed("create the branch", &output).message),
            Err(error) => redact_credentials(&error.message),
        })),
        Err(_) => Ok(ShipEffect::Unknown(
            "Interface couldn't read the checkout's branch afterwards. Check again before trying again.".into(),
        )),
    }
}

/// The commit `remote` has for `branch`, `None` when it has no such branch.
///
/// # Errors
///
/// Fails when the remote cannot be reached.
pub fn remote_branch_head(
    cwd: &Path,
    remote: &str,
    branch: &str,
) -> Result<Option<String>, ThreadGitError> {
    validate_branch(branch)?;
    validate_remote(remote)?;
    let repo = require_repo(cwd)?;
    let reference = format!("refs/heads/{branch}");
    let output = git(
        &repo,
        &["ls-remote", UPLOAD_PACK, "--refs", remote, &reference],
        Call::read().within(REMOTE_TIMEOUT),
    )?;
    if !output.succeeded() {
        return Err(failed(&format!("read {remote}'s {branch}"), &output));
    }
    Ok(lossy(&output.stdout).lines().find_map(|line| {
        let (sha, name) = line.split_once('\t')?;
        (name == reference).then(|| sha.trim().to_owned())
    }))
}

/// Pushes HEAD to `remote`'s `branch` and tracks it there. When git fails
/// or runs out of time, the remote is read again: holding HEAD, the push is
/// done; holding anything else, it was refused; unreadable, unknown. The
/// repository's pre-push hook runs.
///
/// # Errors
///
/// Fails before git runs on a name git would refuse, or without a commit.
pub fn push(cwd: &Path, remote: &str, branch: &str) -> Result<ShipEffect<()>, ThreadGitError> {
    push_with(cwd, remote, branch, Call::checkout())
}

/// [`push`] with git bounded by `call`.
fn push_with(
    cwd: &Path,
    remote: &str,
    branch: &str,
    call: Call<'_>,
) -> Result<ShipEffect<()>, ThreadGitError> {
    validate_branch(branch)?;
    validate_remote(remote)?;
    let repo = require_repo(cwd)?;
    let Some(head) = super::head_commit(&repo, Call::read())? else {
        return Ok(ShipEffect::Refused(
            "There is no commit to push yet.".into(),
        ));
    };
    let refspec = format!("HEAD:refs/heads/{branch}");
    let pushed = git(
        &repo,
        &["push", "--porcelain", "--set-upstream", remote, &refspec],
        call,
    );
    if pushed.as_ref().is_ok_and(Output::succeeded) {
        return Ok(ShipEffect::Done(()));
    }
    let why = match &pushed {
        Ok(output) => redact_credentials(&failed(&format!("push to {remote}"), output).message),
        Err(error) => redact_credentials(&error.message),
    };
    match remote_branch_head(&repo, remote, branch) {
        Ok(Some(found)) if found == head => {
            // It arrived: only tracking may be missing.
            let _ = git(
                &repo,
                &["branch", &format!("--set-upstream-to={remote}/{branch}")],
                Call::read(),
            );
            Ok(ShipEffect::Done(()))
        }
        Ok(_) => Ok(ShipEffect::Refused(why)),
        Err(error) => Ok(ShipEffect::Unknown(format!(
            "{why} Interface then couldn't read {remote} ({}), so it can't tell whether the push arrived. Check again before pushing again.",
            redact_credentials(&error.message)
        ))),
    }
}

fn validate_remote(remote: &str) -> Result<(), ThreadGitError> {
    if remote.is_empty()
        || remote.starts_with('-')
        || remote.len() > 256
        || !remote
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(ThreadGitError::new(
            ThreadGitError::INVALID_CODE,
            format!("{remote:?} is not a usable remote name."),
        ));
    }
    Ok(())
}

/// A branch name for a thread's work: `iface/<slug>-<6 hex>`, the slug from
/// `hint` and the hex from `seed` (the thread's key), so the same thread is
/// offered the same name on every read.
#[must_use]
pub fn suggested_branch(hint: &str, seed: &str) -> String {
    // FNV-1a: stable across runs, unlike the std hasher.
    let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!(
        "{}{}-{:06x}",
        super::BRANCH_PREFIX,
        super::thread_slug(hint),
        hash & 0x00ff_ffff
    )
}

// ---------------------------------------------------------------------------
// The GitHub CLI

/// Where `gh` is, if anywhere, and how long it gets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gh {
    program: PathBuf,
    read_timeout: Duration,
    write_timeout: Duration,
}

/// Whether `gh` can act for the owner here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GhAuth {
    Ready,
    SignedOut,
    /// It could not be run, or did not answer.
    Unavailable,
}

/// A `gh` call that failed: it said why, or it could not run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GhError {
    pub message: String,
    /// The call may have taken effect (it ran out of time or was cut off).
    pub uncertain: bool,
}

impl GhError {
    fn refused(action: &str, output: &crate::process::BoundedOutput) -> Self {
        let stderr = lossy(&output.stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        Self {
            message: redact_credentials(&if detail.is_empty() {
                format!("gh could not {action}.")
            } else {
                format!("gh could not {action} ({detail}).")
            }),
            uncertain: output.timed_out || output.output_truncated,
        }
    }
}

/// A pull request as `gh` reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GhPullRequest {
    pub number: u64,
    pub url: String,
    pub state: PullRequestState,
    pub title: String,
    pub base: String,
    pub draft: bool,
}

/// One check of a pull request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GhCheck {
    pub name: String,
    pub workflow: Option<String>,
    pub bucket: CheckBucket,
    pub link: Option<String>,
}

const PR_FIELDS: &str = "number,url,state,title,baseRefName,isDraft";

impl Gh {
    /// `gh` on `PATH`, else where Homebrew or a user install puts it
    /// (`~/.local/bin`, `~/bin`): an app started from the Dock does not get
    /// the login shell's `PATH`.
    #[must_use]
    pub fn locate() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::locate_in(std::env::var_os("PATH").as_deref(), home.as_deref())
    }

    /// [`Self::locate`] over a given `PATH` and home folder.
    fn locate_in(path: Option<&std::ffi::OsStr>, home: Option<&Path>) -> Option<Self> {
        let user = home
            .filter(|home| home.is_absolute())
            .iter()
            .flat_map(|home| [home.join(".local/bin"), home.join("bin")])
            .collect::<Vec<_>>();
        std::env::split_paths(path.unwrap_or_default())
            .filter(|directory| directory.is_absolute())
            .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
            .chain(user)
            .map(|directory| directory.join("gh"))
            .find(|candidate| candidate.is_file())
            .map(Self::at)
    }

    /// The `gh` this runs.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// `gh` at `program` (tests use a fake).
    #[must_use]
    pub fn at(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            read_timeout: GH_READ_TIMEOUT,
            write_timeout: GH_WRITE_TIMEOUT,
        }
    }

    /// The same `gh` with other deadlines for reads and for writes.
    #[must_use]
    pub fn with_timeouts(self, read: Duration, write: Duration) -> Self {
        Self {
            read_timeout: read,
            write_timeout: write,
            ..self
        }
    }

    fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        max_output: usize,
    ) -> crate::process::BoundedOutput {
        let mut command = Command::new(&self.program);
        filter_credentials(&mut command);
        command
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_SPINNER_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .current_dir(cwd);
        run_bounded_command(command, stdin, timeout, max_output)
    }

    fn read_json(&self, cwd: &Path, args: &[&str], action: &str) -> Result<Value, GhError> {
        let output = self.run(cwd, args, None, self.read_timeout, GH_OUTPUT_BYTES);
        if output.unavailable || output.timed_out || output.exit_code != Some(0) {
            return Err(GhError::refused(action, &output));
        }
        serde_json::from_slice(&output.stdout).map_err(|_| GhError {
            message: format!("gh answered something unexpected when asked to {action}."),
            uncertain: false,
        })
    }

    /// `gh auth status`.
    #[must_use]
    pub fn auth(&self, cwd: &Path) -> GhAuth {
        let output = self.run(
            cwd,
            &["auth", "status"],
            None,
            self.read_timeout,
            GH_OUTPUT_BYTES,
        );
        if output.unavailable || output.timed_out {
            GhAuth::Unavailable
        } else if output.exit_code == Some(0) {
            GhAuth::Ready
        } else {
            GhAuth::SignedOut
        }
    }

    /// The open pull request whose head is `branch`.
    ///
    /// # Errors
    ///
    /// `gh` failed or answered something unreadable.
    pub fn open_pull_request_for(
        &self,
        cwd: &Path,
        branch: &str,
    ) -> Result<Option<GhPullRequest>, GhError> {
        let value = self.read_json(
            cwd,
            &[
                "pr", "list", "--head", branch, "--state", "open", "--json", PR_FIELDS, "--limit",
                "1",
            ],
            "list the branch's pull requests",
        )?;
        let Value::Array(found) = value else {
            return Err(unexpected("list the branch's pull requests"));
        };
        found
            .first()
            .map(|value| parse_pull_request(value).ok_or_else(|| unexpected("read a pull request")))
            .transpose()
    }

    /// Pull request `number`.
    ///
    /// # Errors
    ///
    /// `gh` failed or answered something unreadable.
    pub fn pull_request(&self, cwd: &Path, number: u64) -> Result<GhPullRequest, GhError> {
        let number = number.to_string();
        let value = self.read_json(
            cwd,
            &["pr", "view", &number, "--json", PR_FIELDS],
            "read the pull request",
        )?;
        parse_pull_request(&value).ok_or_else(|| unexpected("read the pull request"))
    }

    /// `gh pr create`; the caller has already looked for an open one.
    /// Answers the new pull request's number.
    ///
    /// # Errors
    ///
    /// `gh` failed; `uncertain` when it may have created one anyway.
    pub fn create_pull_request(
        &self,
        cwd: &Path,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<u64, GhError> {
        let output = self.run(
            cwd,
            &[
                "pr",
                "create",
                "--head",
                head,
                "--base",
                base,
                "--title",
                title,
                "--body-file",
                "-",
            ],
            Some(body.as_bytes().to_vec()),
            self.write_timeout,
            GH_OUTPUT_BYTES,
        );
        if output.exit_code == Some(0) && !output.timed_out {
            if let Some(number) = lossy(&output.stdout)
                .lines()
                .rev()
                .find_map(pull_request_number)
            {
                return Ok(number);
            }
            return Err(GhError {
                message: "gh created something but did not say which pull request.".into(),
                uncertain: true,
            });
        }
        let mut error = GhError::refused("open the pull request", &output);
        error.uncertain |= !output.unavailable && output.exit_code.is_none();
        Err(error)
    }

    /// The pull request's checks (`gh pr checks --json`). No checks is an
    /// empty list.
    ///
    /// # Errors
    ///
    /// `gh` failed or answered something unreadable.
    pub fn checks(&self, cwd: &Path, number: u64) -> Result<Vec<GhCheck>, GhError> {
        let number = number.to_string();
        let output = self.run(
            cwd,
            &[
                "pr",
                "checks",
                &number,
                "--json",
                "name,state,bucket,link,workflow",
            ],
            None,
            self.read_timeout,
            GH_OUTPUT_BYTES,
        );
        if output.unavailable || output.timed_out {
            return Err(GhError::refused("read the checks", &output));
        }
        // 1: some check failed; 8: some are pending. Both still print them.
        match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(Value::Array(checks)) => Ok(checks.iter().filter_map(parse_check).collect()),
            _ if lossy(&output.stderr).contains("no checks reported") => Ok(Vec::new()),
            _ => Err(GhError::refused("read the checks", &output)),
        }
    }

    /// The tail of a failed job's log (`gh run view --job <id> --log-failed`,
    /// or the whole run's when only its id is known), at most
    /// [`LOG_EXCERPT_LINES`] lines and [`LOG_EXCERPT_BYTES`], credentials
    /// redacted.
    ///
    /// # Errors
    ///
    /// `gh` failed.
    pub fn failed_log(&self, cwd: &Path, run: &CheckRun) -> Result<String, GhError> {
        let (id, flag) = match run.job {
            Some(job) => (job.to_string(), "--job"),
            None => (run.run.to_string(), ""),
        };
        let mut args = vec!["run", "view"];
        if flag.is_empty() {
            args.push(&id);
        } else {
            args.extend([flag, id.as_str()]);
        }
        args.push("--log-failed");
        let output = self.run(cwd, &args, None, self.read_timeout, LOG_BYTES);
        if output.unavailable || output.timed_out || output.exit_code != Some(0) {
            return Err(GhError::refused("read the failed log", &output));
        }
        Ok(log_excerpt(&lossy(&output.stdout), output.output_truncated))
    }

    /// The pull request's unresolved review threads, as comments for a draft
    /// the owner reads before sending: everyone's words.
    ///
    /// # Errors
    ///
    /// `gh` failed, or the URL names no repository.
    pub fn review_comments(&self, cwd: &Path, url: &str) -> Result<Vec<ReviewComment>, GhError> {
        self.read_review_comments(cwd, url, ReviewAudience::Everyone)
    }

    /// The unresolved review threads a watch may hand to an agent with nobody
    /// reading first: only what the project's own people and the apps its
    /// owner installed wrote. Anyone at all can comment on a public pull
    /// request, and a watch that passed their words on would let a stranger
    /// give the thread instructions.
    ///
    /// # Errors
    ///
    /// `gh` failed, or the URL names no repository.
    pub fn project_review_comments(
        &self,
        cwd: &Path,
        url: &str,
    ) -> Result<Vec<ReviewComment>, GhError> {
        self.read_review_comments(cwd, url, ReviewAudience::Project)
    }

    fn read_review_comments(
        &self,
        cwd: &Path,
        url: &str,
        audience: ReviewAudience,
    ) -> Result<Vec<ReviewComment>, GhError> {
        let Some((host, owner, name, number)) = pull_request_coordinates(url) else {
            return Err(GhError {
                message: format!("{url} is not a pull request address Interface can read."),
                uncertain: false,
            });
        };
        let owner = format!("owner={owner}");
        let name = format!("name={name}");
        let number = format!("number={number}");
        let query = format!("query={REVIEW_THREADS_QUERY}");
        let mut args = vec!["api", "graphql"];
        if host != "github.com" {
            args.extend(["--hostname", host.as_str()]);
        }
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        let value = self.read_json(cwd, &args, "read the review comments")?;
        let threads = value
            .pointer("/data/repository/pullRequest/reviewThreads/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| unexpected("read the review comments"))?;
        Ok(threads
            .iter()
            .take(MAX_REVIEW_THREADS)
            .filter_map(|thread| parse_review_thread(thread, audience))
            .collect())
    }

    /// `gh pr merge <number> --<method>`. The caller re-reads the state.
    ///
    /// # Errors
    ///
    /// `gh` failed; `uncertain` when it may have merged anyway.
    pub fn merge(&self, cwd: &Path, number: u64, method: MergeMethod) -> Result<(), GhError> {
        let number = number.to_string();
        let flag = match method {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        };
        let output = self.run(
            cwd,
            &["pr", "merge", &number, flag],
            None,
            self.write_timeout,
            GH_OUTPUT_BYTES,
        );
        if output.exit_code == Some(0) && !output.timed_out {
            return Ok(());
        }
        let mut error = GhError::refused("merge the pull request", &output);
        error.uncertain |= !output.unavailable && output.exit_code.is_none();
        Err(error)
    }
}

fn unexpected(action: &str) -> GhError {
    GhError {
        message: format!("gh answered something unexpected when asked to {action}."),
        uncertain: false,
    }
}

fn parse_pull_request(value: &Value) -> Option<GhPullRequest> {
    let state = match value["state"].as_str()? {
        "OPEN" => PullRequestState::Open,
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => return None,
    };
    let url = value["url"].as_str().filter(|url| !url.is_empty())?;
    Some(GhPullRequest {
        number: value["number"].as_u64()?,
        url: url.to_owned(),
        state,
        title: value["title"].as_str().unwrap_or_default().to_owned(),
        base: value["baseRefName"].as_str().unwrap_or_default().to_owned(),
        draft: value["isDraft"].as_bool().unwrap_or(false),
    })
}

fn parse_check(value: &Value) -> Option<GhCheck> {
    let bucket = match value["bucket"].as_str()? {
        "pass" => CheckBucket::Pass,
        "fail" => CheckBucket::Fail,
        "pending" => CheckBucket::Pending,
        "skipping" => CheckBucket::Skipping,
        "cancel" => CheckBucket::Cancel,
        _ => return None,
    };
    let text = |field: &str| {
        value[field]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(200).collect::<String>())
    };
    Some(GhCheck {
        name: text("name")?,
        workflow: text("workflow"),
        bucket,
        link: text("link").filter(|link| link.starts_with("https://")),
    })
}

/// The number at the end of a pull request URL.
fn pull_request_number(line: &str) -> Option<u64> {
    let (_, rest) = line.trim().rsplit_once("/pull/")?;
    rest.trim_end_matches('/').parse().ok()
}

/// `(host, owner, name, number)` of `https://<host>/<owner>/<name>/pull/<n>`.
#[must_use]
pub fn pull_request_coordinates(url: &str) -> Option<(String, String, String, u64)> {
    let rest = url.strip_prefix("https://")?;
    let mut parts = rest.split('/');
    let host = parts.next()?;
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    let safe = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    (safe(host) && safe(owner) && safe(name))
        .then(|| (host.to_owned(), owner.to_owned(), name.to_owned(), number))
}

/// A GitHub Actions run (and job) a check links to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckRun {
    pub run: u64,
    pub job: Option<u64>,
}

/// The Actions run a check's link names: `…/actions/runs/<run>[/job/<job>]`.
#[must_use]
pub fn check_run(link: &str) -> Option<CheckRun> {
    let (_, rest) = link.split_once("/actions/runs/")?;
    let mut parts = rest.split(['/', '?', '#']);
    let run = parts.next()?.parse().ok()?;
    let job = match (parts.next(), parts.next()) {
        (Some("job"), Some(job)) => job.parse().ok(),
        _ => None,
    };
    Some(CheckRun { run, job })
}

static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("a valid pattern"));
static LOG_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[^\t]*\t[^\t]*\t\d{4}-\d\d-\d\dT[\d:.]+Z ?").expect("a valid pattern")
});

/// The last [`LOG_EXCERPT_LINES`] lines of a `--log-failed` log within
/// [`LOG_EXCERPT_BYTES`], without colours or each line's job, step and time
/// prefix, credentials redacted. `truncated`: only the log's start was read.
#[must_use]
pub fn log_excerpt(log: &str, truncated: bool) -> String {
    let lines = log
        .lines()
        .map(|line| {
            let line = ANSI.replace_all(line, "");
            LOG_PREFIX.replace(&line, "").into_owned()
        })
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev().take(LOG_EXCERPT_LINES) {
        let line = if line.len() > 400 {
            let mut cut = 400;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}…", &line[..cut])
        } else {
            line.clone()
        };
        if bytes + line.len() + 1 > LOG_EXCERPT_BYTES {
            break;
        }
        bytes += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    let mut excerpt = kept.join("\n");
    if truncated {
        excerpt.push_str("\n[log cut short: only its start could be read]");
    }
    redact_credentials(&excerpt)
}

const REVIEW_THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100) { nodes { isResolved isOutdated path line startLine originalLine originalStartLine diffSide comments(first: 20) { nodes { author { __typename login } authorAssociation body url diffHunk } } } } } } }";

/// Whose review comments are read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReviewAudience {
    Everyone,
    /// The repository's owner, its organisation's members, its collaborators
    /// and installed apps: people and tools that could already change it.
    Project,
}

impl ReviewAudience {
    fn hears(self, comment: &Value) -> bool {
        self == Self::Everyone
            || matches!(
                comment["authorAssociation"].as_str(),
                Some("OWNER" | "MEMBER" | "COLLABORATOR")
            )
            || comment
                .pointer("/author/__typename")
                .and_then(Value::as_str)
                == Some("Bot")
    }
}

fn parse_review_thread(value: &Value, audience: ReviewAudience) -> Option<ReviewComment> {
    if value["isResolved"].as_bool() != Some(false) {
        return None;
    }
    let path = value["path"].as_str().filter(|path| !path.is_empty())?;
    let number = |field: &str| value[field].as_u64().and_then(|n| u32::try_from(n).ok());
    let outdated = value["isOutdated"].as_bool().unwrap_or(false);
    let end = number("line").or_else(|| number("originalLine"));
    let start = number("startLine")
        .or_else(|| number("originalStartLine"))
        .or(end);
    let side = if value["diffSide"].as_str() == Some("LEFT") {
        DiffSide::Old
    } else {
        DiffSide::New
    };
    let comments = value.pointer("/comments/nodes").and_then(Value::as_array)?;
    let first = comments.first()?;
    let mut text = String::new();
    for comment in comments.iter().filter(|comment| audience.hears(comment)) {
        let author = comment
            .pointer("/author/login")
            .and_then(Value::as_str)
            .unwrap_or("someone");
        let body = comment["body"].as_str().unwrap_or_default().trim();
        if body.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let _ = write!(text, "@{author}: {body}");
    }
    if text.is_empty() && audience == ReviewAudience::Project {
        // Nothing in this thread is the project's own to pass on.
        return None;
    }
    let comment = clip(&redact_credentials(&text), MAX_REVIEW_COMMENT_BYTES);
    let code = first["diffHunk"]
        .as_str()
        .map(|hunk| {
            let lines = hunk
                .lines()
                .filter(|line| !line.starts_with("@@"))
                .collect::<Vec<_>>();
            let wanted = match (start, end) {
                (Some(start), Some(end)) if end >= start => {
                    usize::try_from(end - start + 1).unwrap_or(1)
                }
                _ => 1,
            }
            .min(MAX_REVIEW_CODE_LINES);
            lines[lines.len().saturating_sub(wanted)..].join("\n")
        })
        .unwrap_or_default();
    Some(ReviewComment {
        path: path.to_owned(),
        start_line: start.filter(|_| end.is_some()),
        end_line: end,
        side,
        code: clip(&redact_credentials(&code), MAX_REVIEW_COMMENT_BYTES),
        comment,
        url: first["url"].as_str().unwrap_or_default().to_owned(),
        outdated,
    })
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

#[cfg(test)]
#[path = "thread_ship_tests.rs"]
mod tests;
