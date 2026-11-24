use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use interface_common::git_hardening;
use interface_common::paths::lexically_normalized;
use interface_common::process::{CapturedPipe, poll_backoff, terminate_process_group};
use interface_storage::{
    JsonLimits, PrivateDirectory, TargetSnapshot, WriterLock, atomic_write,
    decode_json_value_strict, ensure_current_revision, read_bounded,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use thiserror::Error;
use uuid::Uuid;

use crate::error::{RegistryError, Result};

pub const SPAWN_FAILED: i32 = -1;
pub const WORKTREE_STORE_MAX_BYTES: usize = 4 * 1024 * 1024;
const GIT_MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const WORKTREE_STORE_VERSION: u8 = 1;
const WORKTREE_FILE: &str = "worktrees.json";
const WORKTREE_LOCK: &str = "worktrees.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait GitRunner: Send + Sync {
    fn run(&self, args: &[String], cwd: &Path) -> GitResult;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGit;

impl GitRunner for SystemGit {
    fn run(&self, args: &[String], cwd: &Path) -> GitResult {
        run_git_command(args, cwd, GIT_TIMEOUT, GIT_MAX_BUFFER_BYTES)
    }
}

/// A launch's project identity survives newly prepared linked worktrees.
/// This names a repository; it never grants access to it.
/// # Errors
/// Refuses unreadable folders and uncertain Git metadata rather than changing identity.
pub fn launch_recovery_scope(cwd: &Path) -> std::result::Result<String, String> {
    let directory = fs::canonicalize(cwd)
        .map_err(|_| "The launch folder could not be identified.".to_owned())?;
    let result = run_git_command(
        &strings(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        &directory,
        Duration::from_secs(5),
        16 * 1024,
    );
    if result.code == 0 {
        let reported = result.stdout.trim();
        if reported.is_empty() || reported.lines().count() != 1 {
            return Err("The repository identity was ambiguous.".into());
        }
        let common = fs::canonicalize(directory.join(reported))
            .map_err(|_| "The repository identity could not be verified.".to_owned())?;
        return Ok(format!("git:{}", common.display()));
    }
    if result.code != 128
        || directory
            .ancestors()
            .any(|parent| fs::symlink_metadata(parent.join(".git")).is_ok())
    {
        return Err(
            "The repository identity could not be verified; no new thread was started.".into(),
        );
    }
    Ok(format!("directory:{}", directory.display()))
}

// Deliberately not shared with `interface-review`'s runner: this one discards the
// status after a kill and reports failures as `SPAWN_FAILED` with git-specific
// prose. Pipe capture, teardown and the poll backoff are shared.

const HARDENED_GIT_PREFIX: [&str; 13] = [
    "--no-pager",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "filter.lfs.process=",
    "-c",
    "filter.lfs.smudge=cat",
    "-c",
    "filter.lfs.clean=cat",
    "-c",
    "filter.lfs.required=false",
];

fn is_credential_env_var(name: &str) -> bool {
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

/// Every registry git call runs none of the programs the repository's own
/// settings name (filter drivers, ssh, credential and signing programs): they
/// are listed first and overridden for the call (`git_hardening`). When the
/// listing fails, git is not run and the listing's failure is the answer.
fn run_git_command(args: &[String], cwd: &Path, timeout: Duration, max_buffer: usize) -> GitResult {
    let started = Instant::now();
    let probe = git_hardening::PROBE_ARGS.map(str::to_owned);
    let listed = run_bounded_bytes(
        Path::new("git"),
        &probe,
        cwd,
        timeout,
        git_hardening::PROBE_MAX_BYTES,
        None,
    );
    if listed.code == SPAWN_FAILED {
        return listed.into();
    }
    let overrides = git_hardening::probe_answered(Some(listed.code))
        .then(|| git_hardening::Overrides::from_listing(&listed.stdout))
        .flatten();
    let Some(overrides) = overrides else {
        return GitResult {
            code: if listed.code > 1 { listed.code } else { 128 },
            stdout: String::new(),
            stderr: format!(
                "{} {}",
                git_hardening::PROBE_REFUSED,
                String::from_utf8_lossy(&listed.stderr).trim()
            ),
        };
    };
    let mut full_args = Vec::with_capacity(
        git_hardening::STATIC_ARGS.len() + HARDENED_GIT_PREFIX.len() + args.len(),
    );
    full_args.extend(git_hardening::STATIC_ARGS.map(str::to_owned));
    full_args.extend(HARDENED_GIT_PREFIX.into_iter().map(str::to_owned));
    full_args.extend_from_slice(args);
    run_bounded_bytes(
        Path::new("git"),
        &full_args,
        cwd,
        timeout.saturating_sub(started.elapsed()),
        max_buffer,
        Some(&overrides),
    )
    .into()
}

/// The bounded runner with its output as text, as the runner's own tests read it.
#[cfg(test)]
fn run_bounded_command(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    max_buffer: usize,
) -> GitResult {
    run_bounded_bytes(executable, args, cwd, timeout, max_buffer, None).into()
}

/// [`GitResult`] before its output is read as text.
struct RawGitResult {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl From<RawGitResult> for GitResult {
    fn from(raw: RawGitResult) -> Self {
        Self {
            code: raw.code,
            stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
        }
    }
}

fn run_bounded_bytes(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    max_buffer: usize,
    overrides: Option<&git_hardening::Overrides>,
) -> RawGitResult {
    let mut command = Command::new(executable);
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_")
            || name.to_str().is_some_and(is_credential_env_var)
        {
            command.env_remove(name);
        }
    }
    command
        .env_remove("INTERFACE_PANE_MCP_SOCKET")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(overrides) = overrides {
        overrides.apply(&mut command);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let message = error.to_string();
            return failed_command_result(&message, &[], &[]);
        }
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return failed_command_result("git stdout could not be captured", &[], &[]);
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return failed_command_result("git stderr could not be captured", &[], &[]);
    };

    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_exceeded = Arc::clone(&exceeded);
    let stderr_exceeded = Arc::clone(&exceeded);
    let stdout_reader = CapturedPipe::spawn(stdout, max_buffer, stdout_exceeded);
    let stderr_reader = CapturedPipe::spawn(stderr, max_buffer, stderr_exceeded);
    let started = Instant::now();
    let mut status = None;
    let failure = loop {
        if exceeded.load(Ordering::Acquire) {
            break Some("git command exceeded the output limit".to_owned());
        }
        if started.elapsed() >= timeout {
            break Some("git command timed out".to_owned());
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(observed) => status = observed,
                Err(error) => break Some(error.to_string()),
            }
            if status.is_some() {
                stdout_reader.child_exited(Some(child.id()));
                stderr_reader.child_exited(Some(child.id()));
            }
        }
        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break None;
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        thread::sleep(poll_backoff(started.elapsed(), GIT_POLL_INTERVAL).min(remaining));
    };
    if failure.is_some() {
        terminate_process_group(child);
    }
    let stdout = stdout_reader.finish();
    let stderr = stderr_reader.finish();
    let read_failed = stdout.is_err() || stderr.is_err();
    let stdout = stdout.unwrap_or_default();
    let stderr = stderr.unwrap_or_default();
    if exceeded.load(Ordering::Acquire) {
        return failed_command_result("git command exceeded the output limit", &stdout, &stderr);
    }
    if let Some(failure) = failure {
        return failed_command_result(&failure, &stdout, &stderr);
    }
    if read_failed {
        return failed_command_result("git output could not be read", &stdout, &stderr);
    }
    RawGitResult {
        code: status
            .and_then(|status| status.code())
            .unwrap_or(SPAWN_FAILED),
        stdout,
        stderr,
    }
}

fn failed_command_result(message: &str, stdout: &[u8], stderr: &[u8]) -> RawGitResult {
    let stderr = if String::from_utf8_lossy(stderr).trim().is_empty() {
        message.as_bytes().to_vec()
    } else {
        stderr.to_vec()
    };
    RawGitResult {
        code: SPAWN_FAILED,
        stdout: stdout.to_vec(),
        stderr,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: PathBuf,
    pub session_cwd: PathBuf,
    pub branch: String,
    pub repo_root: PathBuf,
    pub source_cwd: PathBuf,
    pub dirty_source: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorktreeRecord {
    pub key: String,
    pub path: PathBuf,
    pub session_cwd: PathBuf,
    pub branch: String,
    pub repo_root: PathBuf,
    pub source_cwd: PathBuf,
    pub dirty_source: bool,
}

impl ManagedWorktreeRecord {
    #[must_use]
    pub fn worktree(&self) -> Worktree {
        Worktree {
            path: self.path.clone(),
            session_cwd: self.session_cwd.clone(),
            branch: self.branch.clone(),
            repo_root: self.repo_root.clone(),
            source_cwd: self.source_cwd.clone(),
            dirty_source: self.dirty_source,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedWorktreeMetadata {
    pub canonical_cwd: PathBuf,
    pub canonical_path: PathBuf,
    pub repository_root: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    pub dirty_state_digest: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeFailure {
    GitMissing,
    NotARepo,
    NoCommits,
    Collision,
    AddFailed,
}

#[derive(Debug, Error)]
#[error("{lead} {detail}")]
pub struct WorktreeError {
    pub reason: WorktreeFailure,
    pub lead: String,
    pub detail: String,
}

impl WorktreeError {
    fn new(reason: WorktreeFailure, lead: &str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            lead: lead.to_owned(),
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateWorktreeOptions {
    pub cwd: PathBuf,
    pub name: Option<String>,
    pub base: Option<PathBuf>,
    pub attempts: usize,
    pub init_if_needed: bool,
}

impl CreateWorktreeOptions {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            name: None,
            base: None,
            attempts: 5,
            init_if_needed: false,
        }
    }
}

/// Creates an isolated checkout with a system-generated suffix.
///
/// # Errors
///
/// Returns a user-facing refusal when Git, repository state, containment, or
/// checkout creation cannot be proven safe.
pub fn create_worktree(
    options: &CreateWorktreeOptions,
    git: &dyn GitRunner,
) -> std::result::Result<Worktree, WorktreeError> {
    create_worktree_with_suffix(options, git, &random_suffix)
}

/// Creates an isolated checkout using an injected suffix generator.
///
/// # Errors
///
/// Returns a user-facing refusal when Git, repository state, containment, or
/// checkout creation cannot be proven safe.
pub fn create_worktree_with_suffix(
    options: &CreateWorktreeOptions,
    git: &dyn GitRunner,
    suffix: &dyn Fn() -> String,
) -> std::result::Result<Worktree, WorktreeError> {
    let repo_root = resolve_repo_root(options, git)?;
    ensure_first_commit(options, git, &repo_root)?;

    let status = git.run(&strings(&["status", "--porcelain"]), &repo_root);
    let dirty_source = status.code == 0 && !status.stdout.trim().is_empty();
    let base = options
        .base
        .clone()
        .unwrap_or_else(|| default_base_for(&repo_root));
    let default_name = repo_root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let slug = slugify(options.name.as_deref().unwrap_or(default_name));
    fs::create_dir_all(&base).map_err(|error| {
        WorktreeError::new(
            WorktreeFailure::AddFailed,
            "Couldn't make a worktree.",
            format!("Could not create {} ({error}).", base.display()),
        )
    })?;

    let mut attempts_left = options.attempts.max(1);
    let failed = loop {
        let unique = format!("{slug}-{}", suffix());
        let target = base.join(&unique);
        let branch = format!("iface/{unique}");
        let added = git.run(
            &[
                "worktree".into(),
                "add".into(),
                "-b".into(),
                branch.clone(),
                target.to_string_lossy().into_owned(),
                "HEAD".into(),
            ],
            &repo_root,
        );
        if added.code == 0 {
            return finish_checkout(options, git, &repo_root, target, branch, dirty_source);
        }
        attempts_left -= 1;
        if attempts_left == 0 || !is_collision(&added.stderr) {
            break added;
        }
    };
    if is_collision(&failed.stderr) {
        return Err(WorktreeError::new(
            WorktreeFailure::Collision,
            "Couldn't name a worktree.",
            format!(
                "Every name Interface tried under {} was already taken. Remove the leftover worktrees there (git worktree list) and try again.",
                base.display()
            ),
        ));
    }
    let detail = first_line(&failed.stderr);
    Err(WorktreeError::new(
        WorktreeFailure::AddFailed,
        "Couldn't make a worktree.",
        format!(
            "git could not create a worktree of {}{}. The session was not started.",
            repo_root.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            }
        ),
    ))
}

/// Maps the requested cwd into the checkout git just added and makes that
/// folder, removing the unused checkout again when either step fails.
fn finish_checkout(
    options: &CreateWorktreeOptions,
    git: &dyn GitRunner,
    repo_root: &Path,
    target: PathBuf,
    branch: String,
    dirty_source: bool,
) -> std::result::Result<Worktree, WorktreeError> {
    let session_cwd = relative_under(repo_root, &options.cwd).map(|relative| target.join(relative));
    let orphan = Worktree {
        session_cwd: session_cwd.clone().unwrap_or_else(|| target.clone()),
        path: target,
        branch,
        repo_root: repo_root.to_path_buf(),
        source_cwd: options.cwd.clone(),
        dirty_source,
    };
    let Some(session_cwd) = session_cwd else {
        let _ = remove_worktree(&orphan, git, false);
        return Err(WorktreeError::new(
            WorktreeFailure::AddFailed,
            "Couldn't map that folder.",
            format!(
                "{} is not beneath {}, so Interface could not map the requested working directory into its new checkout. The session was not started.",
                options.cwd.display(),
                repo_root.display()
            ),
        ));
    };
    if let Err(error) = fs::create_dir_all(&session_cwd) {
        let _ = remove_worktree(&orphan, git, false);
        return Err(WorktreeError::new(
            WorktreeFailure::AddFailed,
            "Couldn't finish the worktree.",
            format!(
                "Created {} on {}, but could not materialize {} ({error}). The unused checkout was removed.",
                orphan.path.display(),
                orphan.branch,
                session_cwd.display()
            ),
        ));
    }
    Ok(orphan)
}

/// Finds the repository holding `options.cwd`, first running `git init` there
/// when the caller allows it.
fn resolve_repo_root(
    options: &CreateWorktreeOptions,
    git: &dyn GitRunner,
) -> std::result::Result<PathBuf, WorktreeError> {
    let top = git.run(&strings(&["rev-parse", "--show-toplevel"]), &options.cwd);
    if top.code == SPAWN_FAILED {
        return Err(WorktreeError::new(
            WorktreeFailure::GitMissing,
            "Git isn't available.",
            format!(
                "Interface could not run git in {}. Either git is not installed on this machine, or that folder no longer exists. The session was not started, so nothing ran in the wrong place.",
                options.cwd.display()
            ),
        ));
    }
    if top.code == 0 && !top.stdout.trim().is_empty() {
        return Ok(PathBuf::from(top.stdout.trim()));
    }
    if !options.init_if_needed {
        return Err(WorktreeError::new(
            WorktreeFailure::NotARepo,
            "Not a git repository.",
            format!(
                "{} is not inside a git repository, so there is nothing to make a worktree of. Turn isolation off to run the agent in that folder directly, or point it at a repository.",
                options.cwd.display()
            ),
        ));
    }
    let initialized = git.run(&strings(&["init", "--quiet"]), &options.cwd);
    if initialized.code != 0 {
        return Err(WorktreeError::new(
            WorktreeFailure::NotARepo,
            "Could not create a git repository.",
            format!(
                "Interface tried to make {} a git repository so its workers could each get their own checkout, and git refused: {}",
                options.cwd.display(),
                initialized.stderr.trim()
            ),
        ));
    }
    let rooted = git.run(&strings(&["rev-parse", "--show-toplevel"]), &options.cwd);
    if rooted.code != 0 || rooted.stdout.trim().is_empty() {
        return Err(WorktreeError::new(
            WorktreeFailure::NotARepo,
            "Could not create a git repository.",
            format!(
                "Interface ran git init in {} but git still does not report a repository there.",
                options.cwd.display()
            ),
        ));
    }
    Ok(PathBuf::from(rooted.stdout.trim()))
}

/// Makes sure `repo_root` has a commit to base a worktree on, first staging
/// everything into an initial commit when the caller allows it.
fn ensure_first_commit(
    options: &CreateWorktreeOptions,
    git: &dyn GitRunner,
    repo_root: &Path,
) -> std::result::Result<(), WorktreeError> {
    let head = git.run(
        &strings(&["rev-parse", "--verify", "--quiet", "HEAD"]),
        repo_root,
    );
    if head.code == 0 {
        return Ok(());
    }
    if !options.init_if_needed {
        return Err(WorktreeError::new(
            WorktreeFailure::NoCommits,
            "No commits yet.",
            format!(
                "{} has no commits, so git has nothing to base a worktree on. Make the first commit, or turn isolation off for this session.",
                repo_root.display()
            ),
        ));
    }
    let _ = git.run(&strings(&["add", "-A"]), repo_root);
    let committed = git.run(
        &strings(&[
            "commit",
            "--allow-empty",
            "-m",
            "Initial commit (Interface, so workers can each have their own checkout)",
        ]),
        repo_root,
    );
    if committed.code != 0 {
        return Err(WorktreeError::new(
            WorktreeFailure::NoCommits,
            "Could not make the first commit.",
            format!(
                "Interface tried to make the first commit in {} so its workers could each get their own checkout, and git refused: {}",
                repo_root.display(),
                committed.stderr.trim()
            ),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedWorktreeRegistration {
    Active,
    Absent,
    Unknown,
}

pub fn managed_worktree_registration(
    worktree: &Worktree,
    git: &dyn GitRunner,
    allow_detached: bool,
) -> ManagedWorktreeRegistration {
    let listed = git.run(
        &strings(&["worktree", "list", "--porcelain"]),
        &worktree.repo_root,
    );
    if listed.code != 0 {
        return ManagedWorktreeRegistration::Unknown;
    }
    let expected_path = normalize_absolute(&worktree.path);
    let expected_branch = format!("refs/heads/{}", worktree.branch);
    for lines in porcelain_blocks(&listed.stdout) {
        let path = lines.iter().find_map(|line| line.strip_prefix("worktree "));
        let branch = lines.iter().find_map(|line| line.strip_prefix("branch "));
        if path
            .map(PathBuf::from)
            .map(|path| normalize_absolute(&path))
            != Some(expected_path.clone())
        {
            continue;
        }
        if branch == Some(expected_branch.as_str())
            || (allow_detached && lines.contains(&"detached"))
        {
            return ManagedWorktreeRegistration::Active;
        }
    }
    ManagedWorktreeRegistration::Absent
}

pub fn verify_managed_worktree(worktree: &Worktree, git: &dyn GitRunner) -> bool {
    managed_worktree_registration(worktree, git, false) == ManagedWorktreeRegistration::Active
}

/// Re-proves registration, containment, repository identity, HEAD, and dirty
/// state immediately before a provider lease is issued.
///
/// # Errors
///
/// Returns an error on any missing, changed, escaped, or uninspectable identity.
pub fn verify_worktree_for_lease(
    worktree: &Worktree,
    git: &dyn GitRunner,
    allow_detached: bool,
) -> Result<VerifiedWorktreeMetadata> {
    if managed_worktree_registration(worktree, git, allow_detached)
        != ManagedWorktreeRegistration::Active
    {
        return Err(RegistryError::Worktree(
            "Git no longer identifies the isolated path and branch as managed.".into(),
        ));
    }
    let canonical_path = fs::canonicalize(&worktree.path)
        .map_err(|error| RegistryError::io("canonicalize worktree", &worktree.path, error))?;
    let canonical_cwd = fs::canonicalize(&worktree.session_cwd).map_err(|error| {
        RegistryError::io("canonicalize worktree cwd", &worktree.session_cwd, error)
    })?;
    let repository_root = fs::canonicalize(&worktree.repo_root).map_err(|error| {
        RegistryError::io("canonicalize source repository", &worktree.repo_root, error)
    })?;
    if relative_under(&canonical_path, &canonical_cwd).is_none()
        || canonical_path == repository_root
    {
        return Err(RegistryError::Worktree(
            "The isolated provider cwd is not contained by its managed checkout.".into(),
        ));
    }
    let top = git.run(&strings(&["rev-parse", "--show-toplevel"]), &canonical_cwd);
    let top_path = (!top.stdout.trim().is_empty())
        .then(|| fs::canonicalize(top.stdout.trim()).ok())
        .flatten();
    if top.code != 0 || top_path.as_deref() != Some(canonical_path.as_path()) {
        return Err(RegistryError::Worktree(
            "The isolated provider cwd no longer resolves to its managed checkout.".into(),
        ));
    }
    let head = git.run(&strings(&["rev-parse", "--verify", "HEAD"]), &canonical_cwd);
    let head_value = head.stdout.trim().to_ascii_lowercase();
    if head.code != 0
        || !(40..=64).contains(&head_value.len())
        || !head_value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RegistryError::Worktree(
            "The isolated checkout HEAD could not be verified.".into(),
        ));
    }
    let status = git.run(
        &strings(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        &canonical_cwd,
    );
    if status.code != 0 {
        return Err(RegistryError::Worktree(
            "The isolated checkout dirty state could not be verified.".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(head_value.as_bytes());
    digest.update([0]);
    digest.update(status.stdout.as_bytes());
    Ok(VerifiedWorktreeMetadata {
        canonical_cwd,
        canonical_path,
        repository_root,
        branch: (!allow_detached).then(|| worktree.branch.clone()),
        head: head_value,
        dirty_state_digest: format!("{:x}", digest.finalize()),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveResult {
    pub removed: bool,
    pub kept: Option<RemoveKept>,
    pub changes: Option<usize>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoveKept {
    Dirty,
    /// The checkout holds files git ignores (a copied `.env`,
    /// `node_modules`, build output), which `git worktree remove` would
    /// delete with the folder; the owner removes it.
    Ignored,
    Failed,
}

pub fn remove_worktree(worktree: &Worktree, git: &dyn GitRunner, force: bool) -> RemoveResult {
    // Once the owner deletes a checkout folder on disk (for example after
    // moving ignored files out of it), spawning git in `worktree.path` would
    // fail with ENOENT forever. Prune stale registrations in `repo_root` so
    // the missing checkout deregisters cleanly.
    if fs::symlink_metadata(&worktree.path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        let pruned = git.run(&strings(&["worktree", "prune"]), &worktree.repo_root);
        if pruned.code != 0 {
            return RemoveResult {
                removed: false,
                kept: Some(RemoveKept::Failed),
                changes: None,
                detail: Some(first_line(&pruned.stderr)),
            };
        }
        return RemoveResult {
            removed: true,
            kept: None,
            changes: None,
            detail: None,
        };
    }
    let status = git.run(&strings(&["status", "--porcelain"]), &worktree.path);
    if status.code != 0 {
        return RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Failed),
            changes: None,
            detail: Some(first_line(&status.stderr)),
        };
    }
    let changes = status
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if changes > 0 && !force {
        return RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Dirty),
            changes: Some(changes),
            detail: None,
        };
    }
    if !force {
        // `status --porcelain` never lists ignored files, and removing the
        // checkout would delete them with it.
        let ignored = git.run(
            &strings(&[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ]),
            &worktree.path,
        );
        if ignored.code != 0 {
            return RemoveResult {
                removed: false,
                kept: Some(RemoveKept::Failed),
                changes: None,
                detail: Some(first_line(&ignored.stderr)),
            };
        }
        if !ignored.stdout.is_empty() {
            return RemoveResult {
                removed: false,
                kept: Some(RemoveKept::Ignored),
                changes: None,
                detail: None,
            };
        }
    }
    let mut args = strings(&["worktree", "remove"]);
    if force {
        args.push("--force".into());
    }
    args.push(worktree.path.to_string_lossy().into_owned());
    let removed = git.run(&args, &worktree.repo_root);
    if removed.code != 0 {
        return RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Failed),
            changes: None,
            detail: Some(first_line(&removed.stderr)),
        };
    }
    // No repository-wide `git worktree prune`: it would also forget other
    // worktrees of this repository that are only away for now (a drive that
    // is unplugged, a folder being moved), and git in them then fails.
    RemoveResult {
        removed: true,
        kept: None,
        changes: None,
        detail: None,
    }
}

/// Cleanup entry point for recovered durable metadata. An absent, mismatched,
/// or uninspectable Git registration cannot reach `git worktree remove`.
pub fn remove_registered_worktree(
    worktree: &Worktree,
    git: &dyn GitRunner,
    force: bool,
) -> RemoveResult {
    match managed_worktree_registration(worktree, git, false) {
        ManagedWorktreeRegistration::Active => remove_worktree(worktree, git, force),
        ManagedWorktreeRegistration::Absent => RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Failed),
            changes: None,
            detail: Some("managed worktree registration is absent".into()),
        },
        ManagedWorktreeRegistration::Unknown => RemoveResult {
            removed: false,
            kept: Some(RemoveKept::Failed),
            changes: None,
            detail: Some("managed worktree registration could not be verified".into()),
        },
    }
}

#[derive(Debug)]
pub struct ManagedWorktreeStore {
    directory: PrivateDirectory,
    lock: WriterLock,
    file_name: String,
    target: TargetSnapshot,
    records: Vec<ManagedWorktreeRecord>,
    revision: u64,
}

impl ManagedWorktreeStore {
    /// Opens the default managed-worktree metadata store under one writer lock.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe storage, corrupt JSON, or a held writer lock.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        Self::open_named(directory, WORKTREE_FILE)
    }

    /// Opens a named managed-worktree metadata store under one writer lock.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe storage, corrupt JSON, or a held writer lock.
    pub fn open_named(directory: impl AsRef<Path>, file_name: &str) -> Result<Self> {
        let directory = PrivateDirectory::open_or_create(directory)?;
        let lock = WriterLock::acquire(&directory, WORKTREE_LOCK)?;
        // One hardened read supplies both the CAS target and the records.
        let (target, records) = match read_bounded(&directory, file_name, WORKTREE_STORE_MAX_BYTES)?
        {
            Some(file) => (
                TargetSnapshot::File(file.snapshot().clone()),
                parse_worktree_store(file.bytes(), directory.path().join(file_name))?,
            ),
            None => (TargetSnapshot::Missing, Vec::new()),
        };
        Ok(Self {
            directory,
            lock,
            file_name: file_name.into(),
            target,
            records,
            revision: 1,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn records(&self) -> &[ManagedWorktreeRecord] {
        &self.records
    }

    /// Validates and atomically replaces the full metadata set.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale revision, invalid identity relationship,
    /// external drift, or an unproven durable write.
    pub fn replace(
        &mut self,
        expected_revision: u64,
        records: Vec<ManagedWorktreeRecord>,
    ) -> Result<u64> {
        ensure_current_revision(expected_revision, self.revision)?;
        let mut keys = HashSet::new();
        let mut checkouts = HashSet::new();
        if records.iter().any(|record| {
            !valid_managed_record(record)
                || !keys.insert(record.key.as_str())
                || !checkouts.insert(normalize_absolute(&record.path))
        }) {
            return Err(RegistryError::Invalid(
                "managed worktree metadata failed its identity invariants".into(),
            ));
        }
        let disk = PersistedWorktrees {
            version: WORKTREE_STORE_VERSION,
            worktrees: &records,
        };
        let bytes = serde_json::to_vec_pretty(&disk).map_err(|error| {
            RegistryError::Invalid(format!("worktree metadata cannot be serialized: {error}"))
        })?;
        decode_json_value_strict(
            &bytes,
            worktree_limits()?,
            self.directory.path().join(&self.file_name),
        )?;
        let snapshot = atomic_write(
            &self.directory,
            &self.lock,
            &self.file_name,
            &bytes,
            &self.target,
            WORKTREE_STORE_MAX_BYTES,
        )?;
        self.target = TargetSnapshot::File(snapshot);
        self.records = records;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| RegistryError::Invalid("worktree revision is exhausted".into()))?;
        Ok(self.revision)
    }
}

/// Reads managed metadata without creating, locking, repairing, or rewriting.
///
/// # Errors
///
/// Returns an error for unsafe storage, corrupt JSON, or exceeded bounds.
pub fn load_managed_worktrees_read_only(
    directory: impl AsRef<Path>,
    file_name: &str,
) -> Result<Vec<ManagedWorktreeRecord>> {
    let directory = PrivateDirectory::open(directory)?;
    let Some(file) = read_bounded(&directory, file_name, WORKTREE_STORE_MAX_BYTES)? else {
        return Ok(Vec::new());
    };
    parse_worktree_store(file.bytes(), directory.path().join(file_name))
}

#[derive(Serialize)]
struct PersistedWorktrees<'a> {
    version: u8,
    worktrees: &'a [ManagedWorktreeRecord],
}

fn parse_worktree_store(
    bytes: &[u8],
    path: impl AsRef<Path>,
) -> Result<Vec<ManagedWorktreeRecord>> {
    let path = path.as_ref();
    let raw = decode_json_value_strict(bytes, worktree_limits()?, path)?;
    let Some(object) = raw.as_object() else {
        return Ok(Vec::new());
    };
    if object.get("version").and_then(Value::as_u64) != Some(u64::from(WORKTREE_STORE_VERSION)) {
        return Ok(Vec::new());
    }
    let Some(rows) = object.get("worktrees").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter_map(|row| ManagedWorktreeRecord::deserialize(row).ok())
        .filter(valid_managed_record)
        .collect())
}

fn valid_managed_record(record: &ManagedWorktreeRecord) -> bool {
    if record.key.is_empty()
        || record.branch.is_empty()
        || !record.path.is_absolute()
        || !record.session_cwd.is_absolute()
        || !record.repo_root.is_absolute()
        || !record.source_cwd.is_absolute()
    {
        return false;
    }
    let Some(relative_source) = relative_under(&record.repo_root, &record.source_cwd) else {
        return false;
    };
    let Some(relative_session) = relative_under(&record.path, &record.session_cwd) else {
        return false;
    };
    relative_source == relative_session
        && normalize_absolute(&record.path) != normalize_absolute(&record.repo_root)
}

#[must_use]
pub fn default_base_for(repo_root: &Path) -> PathBuf {
    repo_root
        .parent()
        .unwrap_or(repo_root)
        .join(".interface-worktrees")
}

#[must_use]
pub fn slugify(name: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in name.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() && output.len() < 32 {
                output.push('-');
            }
            separator = false;
            if output.len() < 32 {
                output.push(character);
            }
        } else {
            separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "session".into()
    } else {
        output
    }
}

/// Owned argv for [`GitRunner::run`].
pub(crate) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn is_collision(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("already exists")
        || lower.contains("is already checked out")
        || lower.contains("already used by worktree")
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned();
    if line
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("fatal:"))
    {
        line[6..].trim().to_owned()
    } else {
        line
    }
}

fn random_suffix() -> String {
    Uuid::new_v4().simple().to_string()[..6].to_owned()
}

fn relative_under(root: &Path, target: &Path) -> Option<PathBuf> {
    normalize_absolute(target)
        .strip_prefix(normalize_absolute(root))
        .ok()
        .map(Path::to_path_buf)
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from("/").join(path));
    lexically_normalized(&absolute)
}

fn porcelain_blocks(output: &str) -> Vec<Vec<&str>> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

fn worktree_limits() -> Result<JsonLimits> {
    Ok(JsonLimits::new(WORKTREE_STORE_MAX_BYTES, 32, 50_000)?)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn launch_identity_is_shared_by_new_linked_worktrees_and_refuses_broken_git_metadata() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let result = SystemGit.run(&strings(args), &repo);
            assert_eq!(result.code, 0, "{}", result.stderr);
        };
        git(&["init", "--quiet"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
            "--quiet",
        ]);
        let first = temp.path().join("first");
        let retry = temp.path().join("retry");
        git(&["worktree", "add", "--detach", first.to_str().unwrap()]);
        git(&["worktree", "add", "--detach", retry.to_str().unwrap()]);
        let identity = launch_recovery_scope(&repo).unwrap();
        assert_eq!(launch_recovery_scope(&first).unwrap(), identity);
        assert_eq!(launch_recovery_scope(&retry).unwrap(), identity);
        let plain = temp.path().join("plain");
        fs::create_dir(&plain).unwrap();
        assert_ne!(launch_recovery_scope(&plain).unwrap(), identity);
        fs::write(plain.join(".git"), "gitdir: /missing/recovery-test-git-dir").unwrap();
        assert!(launch_recovery_scope(&plain).is_err());
    }

    struct ScriptedGit {
        script: Mutex<HashMap<String, VecDeque<GitResult>>>,
        calls: Mutex<Vec<(Vec<String>, PathBuf)>>,
    }

    impl ScriptedGit {
        fn new(script: impl IntoIterator<Item = (&'static str, Vec<GitResult>)>) -> Self {
            Self {
                script: Mutex::new(
                    script
                        .into_iter()
                        .map(|(key, values)| (key.into(), values.into()))
                        .collect(),
                ),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl GitRunner for ScriptedGit {
        fn run(&self, args: &[String], cwd: &Path) -> GitResult {
            self.calls.lock().unwrap().push((args.to_vec(), cwd.into()));
            let key = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
            let mut script = self.script.lock().unwrap();
            if let Some(queue) = script.get_mut(&key) {
                queue.pop_front().unwrap_or_else(ok)
            } else {
                script
                    .get_mut(args.first().map_or("", String::as_str))
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(ok)
            }
        }
    }

    fn ok() -> GitResult {
        GitResult {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn answer(stdout: &str) -> GitResult {
        GitResult {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn fail(stderr: &str) -> GitResult {
        GitResult {
            code: 128,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    fn private_root() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("store");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        (temp, root)
    }

    #[cfg(unix)]
    #[test]
    fn system_git_process_is_time_and_output_bounded() {
        let temporary = TempDir::new().expect("temporary directory");
        let success = run_bounded_command(
            Path::new("/bin/sh"),
            &strings(&["-c", "printf out; printf err >&2; exit 7"]),
            temporary.path(),
            Duration::from_secs(1),
            1_024,
        );
        assert_eq!(success.code, 7);
        assert_eq!(success.stdout, "out");
        assert_eq!(success.stderr, "err");

        let started = Instant::now();
        let timed_out = run_bounded_command(
            Path::new("/bin/sh"),
            &strings(&["-c", "while :; do :; done"]),
            temporary.path(),
            Duration::from_millis(30),
            1_024,
        );
        assert_eq!(timed_out.code, SPAWN_FAILED);
        assert!(timed_out.stderr.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));

        let overflowed = run_bounded_command(
            Path::new("/bin/sh"),
            &strings(&[
                "-c",
                "i=0; while [ $i -lt 4096 ]; do printf x; i=$((i + 1)); done",
            ]),
            temporary.path(),
            Duration::from_secs(1),
            256,
        );
        assert_eq!(overflowed.code, SPAWN_FAILED);
        assert_eq!(overflowed.stdout.len(), 256);
        assert!(overflowed.stderr.contains("output limit"));
    }

    #[cfg(unix)]
    #[test]
    fn system_git_deadline_includes_inherited_output_pipes() {
        let started = Instant::now();
        let result = run_bounded_command(
            Path::new("/bin/sh"),
            &strings(&["-c", "printf ready; /bin/sleep 2 & exit 0"]),
            Path::new("/"),
            Duration::from_millis(100),
            128,
        );
        assert_eq!(result.code, SPAWN_FAILED);
        assert_eq!(result.stdout, "ready");
        assert!(result.stderr.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn record(root: &Path) -> ManagedWorktreeRecord {
        ManagedWorktreeRecord {
            key: "codex#1".into(),
            path: root.join("wt"),
            session_cwd: root.join("wt/src"),
            branch: "iface/x".into(),
            repo_root: root.join("repo"),
            source_cwd: root.join("repo/src"),
            dirty_source: false,
        }
    }

    #[test]
    fn metadata_store_round_trips_exact_schema_and_filters_unsafe_rows() {
        let (_temp, root) = private_root();
        let mut store = ManagedWorktreeStore::open(&root).unwrap();
        store
            .replace(store.revision(), vec![record(&root)])
            .unwrap();
        let bytes = fs::read(root.join(WORKTREE_FILE)).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("{\n  \"version\": 1,\n  \"worktrees\": ["));
        assert!(!text.ends_with('\n'));
        drop(store);
        let before = fs::metadata(root.join(WORKTREE_FILE))
            .unwrap()
            .modified()
            .unwrap();
        let loaded = load_managed_worktrees_read_only(&root, WORKTREE_FILE).unwrap();
        assert_eq!(loaded, [record(&root)]);
        assert_eq!(bytes, fs::read(root.join(WORKTREE_FILE)).unwrap());
        assert_eq!(
            before,
            fs::metadata(root.join(WORKTREE_FILE))
                .unwrap()
                .modified()
                .unwrap()
        );

        let unsafe_json = format!(
            "{{\"version\":1,\"worktrees\":[{},{{\"key\":\"bad\",\"path\":\"relative\",\"sessionCwd\":\"/x\",\"branch\":\"b\",\"repoRoot\":\"/r\",\"sourceCwd\":\"/r\",\"dirtySource\":false}}]}}",
            serde_json::to_string(&record(&root)).unwrap()
        );
        let parsed = parse_worktree_store(unsafe_json.as_bytes(), "/tmp/test").unwrap();
        assert_eq!(parsed, [record(&root)]);
    }

    #[test]
    fn registration_must_match_path_and_branch_before_cleanup() {
        let root = PathBuf::from("/w");
        let worktree = record(&root).worktree();
        let git = ScriptedGit::new([(
            "worktree list",
            vec![
                answer("worktree /w/wt\nbranch refs/heads/other\n"),
                fail("cannot inspect"),
            ],
        )]);
        assert_eq!(
            managed_worktree_registration(&worktree, &git, false),
            ManagedWorktreeRegistration::Absent
        );
        let result = remove_registered_worktree(&worktree, &git, false);
        assert_eq!(result.kept, Some(RemoveKept::Failed));
        assert!(
            !git.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(args, _)| args.get(1).is_some_and(|arg| arg == "remove"))
        );
    }

    #[test]
    fn creation_retries_only_collisions_and_removal_keeps_dirty_work() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let source = repo.join("src");
        fs::create_dir_all(&source).unwrap();
        let base = temp.path().join("worktrees");
        let git = ScriptedGit::new([
            (
                "rev-parse --show-toplevel",
                vec![answer(&format!("{}\n", repo.display()))],
            ),
            ("rev-parse --verify", vec![answer("abc\n")]),
            ("status --porcelain", vec![ok(), answer(" M src/lib.rs\n")]),
            ("worktree add", vec![fail("already exists"), ok()]),
        ]);
        let counter = AtomicUsize::new(0);
        let options = CreateWorktreeOptions {
            cwd: source,
            name: Some("Nightly refactor".into()),
            base: Some(base),
            attempts: 3,
            init_if_needed: false,
        };
        let worktree = create_worktree_with_suffix(&options, &git, &|| {
            format!("s{}", counter.fetch_add(1, Ordering::SeqCst))
        })
        .unwrap();
        assert_eq!(worktree.branch, "iface/nightly-refactor-s1");
        assert_eq!(worktree.session_cwd, worktree.path.join("src"));
        let removed = remove_worktree(&worktree, &git, false);
        assert_eq!(removed.kept, Some(RemoveKept::Dirty));
        assert_eq!(removed.changes, Some(1));
    }

    /// Removing a managed checkout keeps one holding git-ignored files (a
    /// copied `.env`, `node_modules`), which `git worktree remove` would
    /// delete, and never unregisters the repository's other worktrees whose
    /// folders are only away for now.
    #[test]
    fn removal_keeps_ignored_files_and_other_absent_worktrees() {
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("project");
        fs::create_dir(&repo).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let result = SystemGit.run(&strings(args), cwd);
            assert_eq!(result.code, 0, "{}", result.stderr);
            result.stdout
        };
        git(&repo, &["init", "--quiet"]);
        fs::write(repo.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        fs::write(repo.join("app.js"), "console.log(1)\n").unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "seed",
                "--quiet",
            ],
        );
        let mut options = CreateWorktreeOptions::new(&repo);
        options.base = Some(root.join("worktrees"));
        let worktree = create_worktree(&options, &SystemGit).unwrap();
        fs::write(worktree.path.join(".env"), "API_KEY=only-copy\n").unwrap();
        fs::create_dir_all(worktree.path.join("node_modules/x")).unwrap();
        fs::write(worktree.path.join("node_modules/x/index.js"), "").unwrap();

        // The owner's own worktree, on a drive that is unplugged for now.
        let drive = root.join("drive");
        let mine = drive.join("my-feature");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "my-feature",
                mine.to_str().unwrap(),
            ],
        );
        let unplugged = root.join("drive-unplugged");
        fs::rename(&drive, &unplugged).unwrap();

        let kept = remove_worktree(&worktree, &SystemGit, false);
        assert!(!kept.removed);
        assert_eq!(kept.kept, Some(RemoveKept::Ignored));
        assert_eq!(
            fs::read_to_string(worktree.path.join(".env")).unwrap(),
            "API_KEY=only-copy\n"
        );
        assert!(worktree.path.join("node_modules/x/index.js").exists());

        // Once the owner has taken their files out, the clean checkout goes,
        // and only it: the absent worktree stays registered.
        fs::remove_file(worktree.path.join(".env")).unwrap();
        fs::remove_dir_all(worktree.path.join("node_modules")).unwrap();
        let removed = remove_worktree(&worktree, &SystemGit, false);
        assert!(removed.removed, "{removed:?}");
        assert!(!worktree.path.exists());
        fs::rename(&unplugged, &drive).unwrap();
        let listed = git(&repo, &["worktree", "list", "--porcelain"]);
        assert!(listed.contains("branch refs/heads/my-feature"), "{listed}");
        git(&mine, &["status", "--short"]);
    }

    #[test]
    fn deleted_or_forced_ignored_checkout_removes_cleanly() {
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("project");
        fs::create_dir(&repo).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let result = SystemGit.run(&strings(args), cwd);
            assert_eq!(result.code, 0, "{}", result.stderr);
            result.stdout
        };
        git(&repo, &["init", "--quiet"]);
        fs::write(repo.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        fs::write(repo.join("app.js"), "console.log(1)\n").unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "seed",
                "--quiet",
            ],
        );

        let mut options = CreateWorktreeOptions::new(&repo);
        options.base = Some(root.join("worktrees"));
        let deleted = create_worktree(&options, &SystemGit).unwrap();
        fs::write(deleted.path.join(".env"), "API_KEY=saved\n").unwrap();
        let kept = remove_worktree(&deleted, &SystemGit, false);
        assert!(!kept.removed);
        assert_eq!(kept.kept, Some(RemoveKept::Ignored));

        // Following the toast ("Move what you need, then delete the folder
        // yourself") must let a subsequent remove prune and deregister cleanly.
        fs::remove_dir_all(&deleted.path).unwrap();
        let pruned = remove_worktree(&deleted, &SystemGit, false);
        assert!(pruned.removed, "{pruned:?}");
        assert_eq!(pruned.kept, None);
        let listed = git(&repo, &["worktree", "list", "--porcelain"]);
        assert!(!listed.contains(&deleted.branch), "{listed}");

        let forced = create_worktree(&options, &SystemGit).unwrap();
        fs::write(forced.path.join(".env"), "API_KEY=discard\n").unwrap();
        fs::create_dir_all(forced.path.join("node_modules/pkg")).unwrap();
        fs::write(forced.path.join("node_modules/pkg/index.js"), "").unwrap();
        let force_removed = remove_worktree(&forced, &SystemGit, true);
        assert!(force_removed.removed, "{force_removed:?}");
        assert!(!forced.path.exists());
    }

    type Calls = Vec<(Vec<String>, PathBuf)>;

    fn call(args: &[&str], cwd: &Path) -> (Vec<String>, PathBuf) {
        (strings(args), cwd.to_path_buf())
    }

    fn add_call(base: &Path, unique: &str, repo: &Path) -> (Vec<String>, PathBuf) {
        let target = base.join(unique).to_string_lossy().into_owned();
        let branch = format!("iface/{unique}");
        call(&["worktree", "add", "-b", &branch, &target, "HEAD"], repo)
    }

    fn cleanup_calls(target: &Path, repo: &Path) -> [(Vec<String>, PathBuf); 3] {
        let path = target.to_string_lossy().into_owned();
        [
            call(&["status", "--porcelain"], target),
            call(
                &[
                    "ls-files",
                    "-z",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                ],
                target,
            ),
            call(&["worktree", "remove", &path], repo),
        ]
    }

    fn attempt(
        options: &CreateWorktreeOptions,
        script: impl IntoIterator<Item = (&'static str, Vec<GitResult>)>,
    ) -> (std::result::Result<Worktree, WorktreeError>, Calls) {
        let git = ScriptedGit::new(script);
        let counter = AtomicUsize::new(0);
        let result = create_worktree_with_suffix(options, &git, &|| {
            format!("s{}", counter.fetch_add(1, Ordering::SeqCst))
        });
        (result, git.calls.into_inner().unwrap())
    }

    fn refusal(
        result: std::result::Result<Worktree, WorktreeError>,
    ) -> (WorktreeFailure, String, String) {
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("{} {}", error.lead, error.detail)
        );
        (error.reason, error.lead, error.detail)
    }

    fn refused(
        reason: WorktreeFailure,
        lead: &str,
        detail: &str,
    ) -> (WorktreeFailure, String, String) {
        (reason, lead.into(), detail.into())
    }

    const TOP: &[&str] = &["rev-parse", "--show-toplevel"];
    const HEAD: &[&str] = &["rev-parse", "--verify", "--quiet", "HEAD"];
    const STATUS: &[&str] = &["status", "--porcelain"];
    const FIRST_COMMIT: &[&str] = &[
        "commit",
        "--allow-empty",
        "-m",
        "Initial commit (Interface, so workers can each have their own checkout)",
    ];

    #[test]
    #[allow(clippy::too_many_lines)]
    fn repository_refusals_keep_their_wording_and_git_sequence() {
        let cwd = PathBuf::from("/nowhere/src");
        let repo = PathBuf::from("/nowhere");
        let plain = CreateWorktreeOptions::new(&cwd);
        let initializing = CreateWorktreeOptions {
            init_if_needed: true,
            ..CreateWorktreeOptions::new(&cwd)
        };

        let missing = GitResult {
            code: SPAWN_FAILED,
            stdout: String::new(),
            stderr: "spawn failed".into(),
        };
        let (result, calls) = attempt(&plain, [("rev-parse --show-toplevel", vec![missing])]);
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::GitMissing,
                "Git isn't available.",
                "Interface could not run git in /nowhere/src. Either git is not installed on this machine, or that folder no longer exists. The session was not started, so nothing ran in the wrong place.",
            )
        );
        assert_eq!(calls, [call(TOP, &cwd)]);

        let not_a_repo = refused(
            WorktreeFailure::NotARepo,
            "Not a git repository.",
            "/nowhere/src is not inside a git repository, so there is nothing to make a worktree of. Turn isolation off to run the agent in that folder directly, or point it at a repository.",
        );
        for top in [fail("fatal: not a git repository"), answer("  \n")] {
            let (result, calls) = attempt(&plain, [("rev-parse --show-toplevel", vec![top])]);
            assert_eq!(refusal(result), not_a_repo);
            assert_eq!(calls, [call(TOP, &cwd)]);
        }

        let (result, calls) = attempt(
            &initializing,
            [
                ("rev-parse --show-toplevel", vec![fail("not a repo")]),
                ("init --quiet", vec![fail("  permission denied \n")]),
            ],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::NotARepo,
                "Could not create a git repository.",
                "Interface tried to make /nowhere/src a git repository so its workers could each get their own checkout, and git refused: permission denied",
            )
        );
        assert_eq!(calls, [call(TOP, &cwd), call(&["init", "--quiet"], &cwd)]);

        let (result, calls) = attempt(
            &initializing,
            [(
                "rev-parse --show-toplevel",
                vec![fail("not a repo"), answer("\n")],
            )],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::NotARepo,
                "Could not create a git repository.",
                "Interface ran git init in /nowhere/src but git still does not report a repository there.",
            )
        );
        assert_eq!(
            calls,
            [
                call(TOP, &cwd),
                call(&["init", "--quiet"], &cwd),
                call(TOP, &cwd)
            ]
        );

        let (result, calls) = attempt(
            &plain,
            [
                ("rev-parse --show-toplevel", vec![answer("/nowhere\n")]),
                ("rev-parse --verify", vec![fail("")]),
            ],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::NoCommits,
                "No commits yet.",
                "/nowhere has no commits, so git has nothing to base a worktree on. Make the first commit, or turn isolation off for this session.",
            )
        );
        assert_eq!(calls, [call(TOP, &cwd), call(HEAD, &repo)]);

        let (result, calls) = attempt(
            &initializing,
            [
                (
                    "rev-parse --show-toplevel",
                    vec![fail("not a repo"), answer("/nowhere\n")],
                ),
                ("rev-parse --verify", vec![fail("")]),
                ("add -A", vec![fail("ignored")]),
                ("commit --allow-empty", vec![fail(" nothing to commit \n")]),
            ],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::NoCommits,
                "Could not make the first commit.",
                "Interface tried to make the first commit in /nowhere so its workers could each get their own checkout, and git refused: nothing to commit",
            )
        );
        assert_eq!(
            calls,
            [
                call(TOP, &cwd),
                call(&["init", "--quiet"], &cwd),
                call(TOP, &cwd),
                call(HEAD, &repo),
                call(&["add", "-A"], &repo),
                call(FIRST_COMMIT, &repo),
            ]
        );
    }

    #[test]
    fn initializing_creation_runs_every_phase_in_order() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let cwd = repo.join("src");
        let base = temp.path().join("worktrees");
        let options = CreateWorktreeOptions {
            base: Some(base.clone()),
            init_if_needed: true,
            ..CreateWorktreeOptions::new(&cwd)
        };
        let (result, calls) = attempt(
            &options,
            [
                (
                    "rev-parse --show-toplevel",
                    vec![fail("not a repo"), answer(&format!("{}\n", repo.display()))],
                ),
                ("rev-parse --verify", vec![fail("")]),
                ("status --porcelain", vec![fail("unreadable")]),
            ],
        );
        let target = base.join("repo-s0");
        assert_eq!(
            result.unwrap(),
            Worktree {
                path: target.clone(),
                session_cwd: target.join("src"),
                branch: "iface/repo-s0".into(),
                repo_root: repo.clone(),
                source_cwd: cwd.clone(),
                dirty_source: false,
            }
        );
        assert!(target.join("src").is_dir());
        assert_eq!(
            calls,
            [
                call(TOP, &cwd),
                call(&["init", "--quiet"], &cwd),
                call(TOP, &cwd),
                call(HEAD, &repo),
                call(&["add", "-A"], &repo),
                call(FIRST_COMMIT, &repo),
                call(STATUS, &repo),
                add_call(&base, "repo-s0", &repo),
            ]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn checkout_refusals_keep_their_wording_git_sequence_and_cleanup() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let cwd = repo.join("src");
        let base = temp.path().join("worktrees");
        let top = || {
            (
                "rev-parse --show-toplevel",
                vec![answer(&format!("{}\n", repo.display()))],
            )
        };
        let options = |attempts: usize| CreateWorktreeOptions {
            name: Some("Nightly".into()),
            base: Some(base.clone()),
            attempts,
            ..CreateWorktreeOptions::new(&cwd)
        };
        let prelude = [call(TOP, &cwd), call(HEAD, &repo), call(STATUS, &repo)];

        let (result, calls) = attempt(
            &options(2),
            [
                top(),
                (
                    "worktree add",
                    vec![
                        fail("fatal: 'x' already exists"),
                        fail("fatal: 'y' is already used by worktree at 'z'"),
                    ],
                ),
            ],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::Collision,
                "Couldn't name a worktree.",
                &format!(
                    "Every name Interface tried under {} was already taken. Remove the leftover worktrees there (git worktree list) and try again.",
                    base.display()
                ),
            )
        );
        let mut expected = prelude.to_vec();
        expected.push(add_call(&base, "nightly-s0", &repo));
        expected.push(add_call(&base, "nightly-s1", &repo));
        assert_eq!(calls, expected);
        assert!(base.is_dir());

        let (result, calls) = attempt(
            &options(3),
            [
                top(),
                (
                    "worktree add",
                    vec![
                        fail("already exists"),
                        fail("\n fatal: invalid reference: HEAD \nmore"),
                    ],
                ),
            ],
        );
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::AddFailed,
                "Couldn't make a worktree.",
                &format!(
                    "git could not create a worktree of {} (invalid reference: HEAD). The session was not started.",
                    repo.display()
                ),
            )
        );
        let mut expected = prelude.to_vec();
        expected.push(add_call(&base, "nightly-s0", &repo));
        expected.push(add_call(&base, "nightly-s1", &repo));
        assert_eq!(calls, expected);

        let (result, calls) = attempt(&options(0), [top(), ("worktree add", vec![fail("")])]);
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::AddFailed,
                "Couldn't make a worktree.",
                &format!(
                    "git could not create a worktree of {}. The session was not started.",
                    repo.display()
                ),
            )
        );
        let mut expected = prelude.to_vec();
        expected.push(add_call(&base, "nightly-s0", &repo));
        assert_eq!(calls, expected);

        let elsewhere = temp.path().join("elsewhere");
        let unmappable = CreateWorktreeOptions {
            cwd: elsewhere.clone(),
            ..options(1)
        };
        let (result, calls) = attempt(&unmappable, [top()]);
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::AddFailed,
                "Couldn't map that folder.",
                &format!(
                    "{} is not beneath {}, so Interface could not map the requested working directory into its new checkout. The session was not started.",
                    elsewhere.display(),
                    repo.display()
                ),
            )
        );
        let target = base.join("nightly-s0");
        let expected = vec![
            call(TOP, &elsewhere),
            call(HEAD, &repo),
            call(STATUS, &repo),
            add_call(&base, "nightly-s0", &repo),
            call(&["worktree", "prune"], &repo),
        ];
        assert_eq!(calls, expected);

        // A file where the checkout should be keeps the session folder from
        // being made; the unused checkout is removed and the refusal says so.
        fs::write(&target, "occupied").unwrap();
        let session_cwd = target.join("src");
        let expected_error = fs::create_dir_all(&session_cwd).unwrap_err();
        let (result, calls) = attempt(&options(1), [top()]);
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::AddFailed,
                "Couldn't finish the worktree.",
                &format!(
                    "Created {} on iface/nightly-s0, but could not materialize {} ({expected_error}). The unused checkout was removed.",
                    target.display(),
                    session_cwd.display()
                ),
            )
        );
        let mut expected = prelude.to_vec();
        expected.push(add_call(&base, "nightly-s0", &repo));
        expected.extend(cleanup_calls(&target, &repo));
        assert_eq!(calls, expected);

        let blocked = temp.path().join("blocked");
        fs::write(&blocked, "a file, not a directory").unwrap();
        let expected_error = fs::create_dir_all(&blocked).unwrap_err();
        let unmakeable = CreateWorktreeOptions {
            base: Some(blocked.clone()),
            ..options(1)
        };
        let (result, calls) = attempt(&unmakeable, [top()]);
        assert_eq!(
            refusal(result),
            refused(
                WorktreeFailure::AddFailed,
                "Couldn't make a worktree.",
                &format!("Could not create {} ({expected_error}).", blocked.display()),
            )
        );
        assert_eq!(calls, prelude);
    }

    #[test]
    fn slug_and_default_base_match_legacy_contract() {
        assert_eq!(slugify("fix: the ~thing~ [again]?"), "fix-the-thing-again");
        assert_eq!(slugify("a..b"), "a-b");
        assert_eq!(slugify("  "), "session");
        assert!(slugify(&"x".repeat(80)).len() <= 32);
        assert_eq!(
            default_base_for(Path::new("/Users/me/project")),
            Path::new("/Users/me/.interface-worktrees")
        );
    }
}
