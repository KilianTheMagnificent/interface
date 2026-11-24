//! The seams [`super::ThreadHost`] reaches the rest of the host through, and
//! their production implementations. Each test replaces them with fakes.

use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::SessionSummary,
    ipc::{
        CHANNELS, CollaborationStatusDto, CollaborationSubmitRequest, HISTORY_LIMIT, HistoryPage,
        parse_key,
    },
    threads::{BranchList, PullRequestState, RestoreResult, ThreadDiff},
};
use interface_engine::ForkBoundary;
use interface_registry::SessionRegistry;
use interface_review::thread_git::{
    self, CaptureOutcome, DiffRequest, GitWorkspaceRequest, PreparedGitWorkspace, RemoveOutcome,
    ThreadGitError, ship::Gh,
};
use interface_runtime::{RuntimeError, RuntimeErrorKind, SessionRuntime};
use interface_settings::threads::FoundPullRequest;
use serde_json::{Value, json};

use crate::{
    bounded_process::{Capture, CapturedOutput, run_bounded},
    collaboration::CollaborationDesktopControl,
    dispatcher::{DispatchError, HostDispatcher},
    orchestration_transport::OrchestrationHostControl,
    project_away::AwayHostDispatcher,
    registry_access,
};

/// Milliseconds since the Unix epoch.
pub type ThreadClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The registry rows and runtime operations threads need.
#[async_trait]
pub trait ThreadSessions: Send + Sync + 'static {
    /// Every registry row, archived ones included.
    ///
    /// # Errors
    /// The registry could not be read.
    fn sessions(&self) -> Result<Vec<SessionSummary>, String>;

    /// # Errors
    /// The registry could not be read.
    fn session(&self, key: &str) -> Result<Option<SessionSummary>, String>;

    /// Whether the registry lists every session it knows: false while it
    /// is rebuilt from the providers' listings because its cache did not
    /// load. Reconcile then marks no thread missing.
    fn registry_complete(&self) -> bool {
        true
    }

    /// The sessions the runtime holds a live attachment for.
    ///
    /// # Errors
    /// An attachment is changing; try again.
    fn live_keys(&self) -> Result<HashSet<String>, String>;

    /// Forks `key` through the last answer to the prompt `boundary` names;
    /// answers the new key.
    async fn fork_at(&self, key: &str, boundary: ForkBoundary) -> Result<String, RuntimeError>;

    /// The existing rename operation (provider first, then the registry).
    async fn rename(&self, key: &str, title: String) -> Result<(), RuntimeError>;

    /// Closes `key`'s live provider session through the runtime's close
    /// operation, as the desktop's close does. A session with no live
    /// attachment is left as it is.
    ///
    /// # Errors
    /// Why the close was not confirmed; the session may still be open.
    async fn close(&self, key: &str) -> Result<(), String>;

    /// The newest page of `key`'s provider transcript, as the renderer reads it.
    async fn history(&self, key: &str) -> Result<HistoryPage, RuntimeError>;

    /// `key`'s whole provider transcript, up to the runtime's import bound,
    /// for the thread journal's one-time import.
    async fn import_history(&self, key: &str) -> Result<HistoryPage, RuntimeError> {
        self.history(key).await
    }

    /// Archives (or restores) `key`'s session as the desktop's archive does:
    /// an earlier segment of a switched thread follows its head (T3 section
    /// 5). A live session is closed first.
    ///
    /// # Errors
    /// Why it did not change.
    async fn set_archived(&self, key: &str, archived: bool) -> Result<(), String> {
        let _ = (key, archived);
        Err("archiving is not available here".into())
    }

    /// Deletes (`deleted`) or forgets `key`'s session as the desktop's
    /// delete and forget do, closing it first: an earlier segment of a
    /// switched thread goes with its head.
    ///
    /// # Errors
    /// Why it was not removed.
    async fn remove(&self, key: &str, deleted: bool) -> Result<(), String> {
        let _ = (key, deleted);
        Err("removing is not available here".into())
    }
}

pub struct RuntimeThreadSessions(pub Arc<SessionRuntime>);

#[async_trait]
impl ThreadSessions for RuntimeThreadSessions {
    fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        registry_access::read(&self.0, SessionRegistry::all).map_err(|error| error.to_string())
    }

    fn session(&self, key: &str) -> Result<Option<SessionSummary>, String> {
        registry_access::read(&self.0, |registry| registry.get(key).cloned())
            .map_err(|error| error.to_string())
    }

    fn registry_complete(&self) -> bool {
        registry_access::read(&self.0, SessionRegistry::restored_from_cache).unwrap_or(false)
    }

    fn live_keys(&self) -> Result<HashSet<String>, String> {
        self.0
            .live_session_keys()
            .map_err(|error| error.to_string())
    }

    async fn fork_at(&self, key: &str, boundary: ForkBoundary) -> Result<String, RuntimeError> {
        let (engine, session_id) = parse_key(key).map_err(|error| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                format!("Cannot fork {key}: {error}."),
                false,
            )
        })?;
        self.0.fork_session_at(&engine, &session_id, boundary).await
    }

    async fn rename(&self, key: &str, title: String) -> Result<(), RuntimeError> {
        self.0.rename_stored_session(key, title).await
    }

    async fn close(&self, key: &str) -> Result<(), String> {
        if !self
            .0
            .session_is_live(key)
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
        self.0
            .close_session(key)
            .await
            .outcome
            .into_result()
            .map_err(|error| error.to_string())
    }

    async fn history(&self, key: &str) -> Result<HistoryPage, RuntimeError> {
        self.0.read_provider_history(key, HISTORY_LIMIT).await
    }

    async fn import_history(&self, key: &str) -> Result<HistoryPage, RuntimeError> {
        self.0.read_provider_history_for_import(key).await
    }

    async fn set_archived(&self, key: &str, archived: bool) -> Result<(), String> {
        if archived {
            self.close(key).await?;
        }
        self.0
            .archive_session(key, archived)
            .await
            .map_err(|error| error.to_string())
    }

    async fn remove(&self, key: &str, deleted: bool) -> Result<(), String> {
        self.close(key).await?;
        if deleted {
            self.0.delete_stored_session(key).await
        } else {
            self.0.forget_session(key).await
        }
        .map_err(|error| error.to_string())
    }
}

/// The cross-model consult a thread's Second opinion goes through (review
/// B4): the runtime dispatcher's resume, with its gates (one writer per
/// session, the permission mode the thread was given), and the durable
/// collaboration service's submit.
#[async_trait]
pub trait ThreadConsults: Send + Sync + 'static {
    /// Opens `key`'s provider session through the runtime dispatcher, as a
    /// send would: a consult reads a live source.
    ///
    /// # Errors
    /// The runtime's refusal (a pane holds the session, the provider is
    /// down), with its own certainty.
    async fn resume(&self, key: &str) -> Result<(), DispatchError>;

    /// Submits `request` as the collaboration channels do.
    ///
    /// # Errors
    /// Collaboration is unavailable, or the coordinator refused it.
    async fn submit(
        &self,
        request: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, DispatchError>;
}

/// [`ThreadConsults`] over the host's runtime dispatcher (attached once it
/// exists) and its collaboration service (none when it did not open).
pub struct HostThreadConsults {
    pub runtime: Arc<dyn HostDispatcher>,
    pub collaboration: Option<Arc<dyn CollaborationDesktopControl>>,
}

#[async_trait]
impl ThreadConsults for HostThreadConsults {
    async fn resume(&self, key: &str) -> Result<(), DispatchError> {
        self.runtime
            .dispatch(CHANNELS.resume_session, json!({ "sessionKey": key }))
            .await
            .map(|_| ())
    }

    async fn submit(
        &self,
        request: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, DispatchError> {
        let Some(collaboration) = &self.collaboration else {
            return Err(DispatchError::new(
                "collaboration-unavailable",
                "Cross-model collaboration is unavailable, so no second opinion was asked for. Restart Interface to try again.",
                false,
                true,
            ));
        };
        collaboration.submit(request).await.map_err(Into::into)
    }
}

/// One-shot thread titles from the conversation's first exchange.
#[async_trait]
pub trait ThreadTitles: Send + Sync + 'static {
    /// Whether a title can be asked for at all: Claude is installed.
    fn available(&self) -> bool;

    /// A short title for a thread that began with `first_prompt` and was
    /// answered with `first_answer` (may be empty).
    ///
    /// # Errors
    /// Why no title came back; nothing was changed.
    async fn generate(&self, first_prompt: String, first_answer: String) -> Result<String, String>;

    /// [`Self::generate`] for the thread `key` (protocol 60: a Claude thread
    /// on an added account asks with that account).
    ///
    /// # Errors
    /// Why no title came back; nothing was changed.
    async fn generate_for(
        &self,
        _key: &str,
        first_prompt: String,
        first_answer: String,
    ) -> Result<String, String> {
        self.generate(first_prompt, first_answer).await
    }

    /// Write with AI (protocol 67): a commit message or a pull request's
    /// text from `material` (bounded and credential-filtered by the
    /// caller), on Claude's default account.
    ///
    /// # Errors
    /// Why no text came back; nothing was changed.
    async fn write_ship_text(
        &self,
        what: interface_contracts::threads::ShipTextKind,
        material: String,
    ) -> Result<String, String> {
        let _ = (what, material);
        Err("Write with AI is not available here".into())
    }
}

/// [`ThreadTitles`] through `engine_claude::title`: one `claude -p` call
/// with no tools, settings sources or saved session.
pub struct ClaudeThreadTitles {
    /// The resolved Claude CLI; `None` when it is not installed.
    pub executable: Option<PathBuf>,
    /// Protocol 60: a Claude thread bound to an added account asks with that
    /// account's `CLAUDE_CONFIG_DIR`; every other thread with Default.
    pub accounts: Option<Arc<crate::provider_accounts::ProviderAccounts>>,
}

#[async_trait]
impl ThreadTitles for ClaudeThreadTitles {
    fn available(&self) -> bool {
        self.executable.as_deref().is_some_and(Path::is_file)
    }

    async fn generate(&self, first_prompt: String, first_answer: String) -> Result<String, String> {
        let executable = self
            .executable
            .as_deref()
            .ok_or("Claude is not installed")?;
        // A neutral folder: the call reads no project and saves no session.
        engine_claude::title::generate_title(
            executable,
            &std::env::temp_dir(),
            &first_prompt,
            &first_answer,
        )
        .await
    }

    async fn generate_for(
        &self,
        key: &str,
        first_prompt: String,
        first_answer: String,
    ) -> Result<String, String> {
        let environment = if key.starts_with("claude#") {
            self.accounts
                .as_ref()
                .map(|accounts| accounts.session_environment(key))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if environment.is_empty() {
            return self.generate(first_prompt, first_answer).await;
        }
        let executable = self
            .executable
            .as_deref()
            .ok_or("Claude is not installed")?;
        engine_claude::title::generate_title_with_environment(
            executable,
            &std::env::temp_dir(),
            &first_prompt,
            &first_answer,
            &environment,
        )
        .await
    }

    async fn write_ship_text(
        &self,
        what: interface_contracts::threads::ShipTextKind,
        material: String,
    ) -> Result<String, String> {
        let executable = self
            .executable
            .as_deref()
            .ok_or("Claude is not installed")?;
        // Claude's default account, in a neutral folder: the call reads no
        // project and saves no session.
        engine_claude::ship_text::write_ship_text(
            executable,
            &std::env::temp_dir(),
            what,
            &material,
            &[],
        )
        .await
    }
}

/// Sessions the Threads surface leaves out, as Chat does: the turns of Away
/// runs and orchestration workers.
#[async_trait]
pub trait HiddenSessions: Send + Sync + 'static {
    async fn hidden_keys(&self) -> BTreeSet<String>;
}

/// Reads hidden sessions from the orchestration owner (workers: agents with
/// a parent that are not orchestrators, over the whole agent list) and every
/// Away run's turns. Either source being unavailable hides nothing from it.
pub struct WorkerSessions {
    pub orchestration: Option<OrchestrationHostControl>,
    /// Set once Away exists: it sends through the runtime threads hook.
    pub away: OnceLock<Arc<AwayHostDispatcher>>,
}

#[async_trait]
impl HiddenSessions for WorkerSessions {
    async fn hidden_keys(&self) -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        if let Some(orchestration) = &self.orchestration
            && let Ok(workers) = orchestration.worker_session_keys().await
        {
            keys.extend(workers);
        }
        if let Some(away) = self.away.get() {
            keys.extend(away.session_keys().await);
        }
        keys
    }
}

/// Whether the owner may open a folder.
#[async_trait]
pub trait FolderAuthority: Send + Sync + 'static {
    /// # Errors
    /// The folder is not one the owner may open; definitely unsent.
    async fn may_open(&self, folder: &str) -> Result<(), DispatchError>;
}

/// The check the composer makes before a launch: `listDir` on the folder,
/// through the desktop owner and its session-root authority.
pub struct ListDirAuthority(pub Arc<dyn HostDispatcher>);

#[async_trait]
impl FolderAuthority for ListDirAuthority {
    async fn may_open(&self, folder: &str) -> Result<(), DispatchError> {
        self.0
            .dispatch(CHANNELS.list_dir, json!({ "path": folder }))
            .await
            .map(|_| ())
            .map_err(|error| {
                DispatchError::new(
                    super::THREADS_FOLDER_REFUSED_CODE,
                    format!(
                        "Interface may not open {folder}: {} Choose the folder again from New thread.",
                        error.message
                    ),
                    false,
                    true,
                )
            })
    }
}

/// The git mechanics threads use, mirroring
/// `interface_review::thread_git`. Synchronous: the host calls these on its
/// blocking pool.
#[allow(clippy::missing_errors_doc)] // Each mirrors the `thread_git` function it names.
pub trait ThreadGit: Send + Sync + 'static {
    fn prepare_workspace(
        &self,
        request: &GitWorkspaceRequest,
    ) -> Result<PreparedGitWorkspace, ThreadGitError>;
    fn discard_workspace(
        &self,
        prepared: &PreparedGitWorkspace,
    ) -> Result<RemoveOutcome, ThreadGitError>;
    fn ensure_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<bool, ThreadGitError>;
    fn capture_checkpoint(
        &self,
        cwd: &Path,
        refname: &str,
    ) -> Result<CaptureOutcome, ThreadGitError>;
    fn numstat(
        &self,
        cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
    ) -> Result<(u32, u32, u32), ThreadGitError>;
    fn numstat_paths(
        &self,
        cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
        paths: &[String],
    ) -> Result<(u32, u32, u32), ThreadGitError>;
    /// The files a turn's edits named, relative to the repository root.
    fn repo_relative_paths(
        &self,
        cwd: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, ThreadGitError>;
    fn diff(&self, cwd: &Path, request: &DiffRequest) -> Result<ThreadDiff, ThreadGitError>;
    fn merge_base(&self, cwd: &Path, base_ref: &str) -> Result<Option<String>, ThreadGitError>;
    fn restore_to(
        &self,
        cwd: &Path,
        refname: &str,
        recovery_ref: &str,
    ) -> Result<RestoreResult, ThreadGitError>;
    fn remove_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
    ) -> Result<RemoveOutcome, ThreadGitError>;
    fn list_branches(&self, project_root: &Path) -> Result<BranchList, ThreadGitError>;
    fn list_worktrees(
        &self,
        project_root: &Path,
    ) -> Result<Vec<(PathBuf, Option<String>)>, ThreadGitError>;
    fn delete_refs(&self, cwd: &Path, prefix: &str) -> Result<u32, ThreadGitError>;
    /// Renames ref `from` to `to` in one transaction, used to file a launch
    /// baseline under its thread. When it fails the baseline keeps its
    /// pending name (records store full ref names, so it still serves turn 0).
    fn move_ref(&self, cwd: &Path, from: &str, to: &str) -> Result<(), ThreadGitError>;
}

/// [`ThreadGit`] over `interface_review::thread_git`.
pub struct SystemThreadGit;

impl ThreadGit for SystemThreadGit {
    fn prepare_workspace(
        &self,
        request: &GitWorkspaceRequest,
    ) -> Result<PreparedGitWorkspace, ThreadGitError> {
        thread_git::prepare_workspace(request)
    }

    fn discard_workspace(
        &self,
        prepared: &PreparedGitWorkspace,
    ) -> Result<RemoveOutcome, ThreadGitError> {
        thread_git::discard_workspace(prepared)
    }

    fn ensure_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<bool, ThreadGitError> {
        thread_git::ensure_worktree(repo_root, worktree_path, branch)
    }

    fn capture_checkpoint(
        &self,
        cwd: &Path,
        refname: &str,
    ) -> Result<CaptureOutcome, ThreadGitError> {
        thread_git::capture_checkpoint(cwd, refname)
    }

    fn numstat(
        &self,
        cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
    ) -> Result<(u32, u32, u32), ThreadGitError> {
        thread_git::numstat(cwd, from_ref, to_ref)
    }

    fn numstat_paths(
        &self,
        cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
        paths: &[String],
    ) -> Result<(u32, u32, u32), ThreadGitError> {
        thread_git::numstat_paths(cwd, from_ref, to_ref, paths)
    }

    fn repo_relative_paths(
        &self,
        cwd: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, ThreadGitError> {
        thread_git::repo_relative_paths(cwd, paths)
    }

    fn diff(&self, cwd: &Path, request: &DiffRequest) -> Result<ThreadDiff, ThreadGitError> {
        thread_git::diff(cwd, request)
    }

    fn merge_base(&self, cwd: &Path, base_ref: &str) -> Result<Option<String>, ThreadGitError> {
        thread_git::merge_base(cwd, base_ref)
    }

    fn restore_to(
        &self,
        cwd: &Path,
        refname: &str,
        recovery_ref: &str,
    ) -> Result<RestoreResult, ThreadGitError> {
        thread_git::restore_to(cwd, refname, recovery_ref)
    }

    fn remove_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
    ) -> Result<RemoveOutcome, ThreadGitError> {
        thread_git::remove_worktree(repo_root, worktree_path)
    }

    fn list_branches(&self, project_root: &Path) -> Result<BranchList, ThreadGitError> {
        thread_git::list_branches(project_root)
    }

    fn list_worktrees(
        &self,
        project_root: &Path,
    ) -> Result<Vec<(PathBuf, Option<String>)>, ThreadGitError> {
        thread_git::list_worktrees(project_root)
    }

    fn delete_refs(&self, cwd: &Path, prefix: &str) -> Result<u32, ThreadGitError> {
        thread_git::delete_refs(cwd, prefix)
    }

    fn move_ref(&self, cwd: &Path, from: &str, to: &str) -> Result<(), ThreadGitError> {
        thread_git::move_ref(cwd, from, to)
    }
}

/// What a pull request lookup found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PullRequestLookup {
    Found(FoundPullRequest),
    None,
    /// `gh` is missing, signed out, timed out or failed: record nothing.
    Unavailable,
}

pub trait PullRequests: Send + Sync + 'static {
    /// The newest pull request whose head is `branch`, asked in `project_root`.
    fn lookup(&self, project_root: &Path, branch: &str) -> PullRequestLookup;
    /// Pull request `number`, asked in `project_root` (a thread that already
    /// has one, such as one opened from a Local thread).
    fn view(&self, project_root: &Path, number: u64) -> PullRequestLookup;
}

const GH_TIMEOUT: Duration = Duration::from_secs(20);
const GH_POLL_CAP: Duration = Duration::from_millis(100);
const GH_OUTPUT_BYTES: usize = 64 * 1024;

/// `gh pr list --head <branch> --state all --json number,url,state --limit 1`.
pub struct GhPullRequests;

impl PullRequests for GhPullRequests {
    fn lookup(&self, project_root: &Path, branch: &str) -> PullRequestLookup {
        let Some(gh) = find_gh() else {
            return PullRequestLookup::Unavailable;
        };
        let mut command = Command::new(gh);
        command
            .args(["pr", "list", "--head", branch, "--state", "all"])
            .args(["--json", "number,url,state", "--limit", "1"])
            .current_dir(project_root)
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let capture = Capture {
            output: CapturedOutput::Stdout,
            max_bytes: GH_OUTPUT_BYTES,
            overflow_fails: true,
        };
        match run_bounded(&mut command, GH_TIMEOUT, GH_POLL_CAP, capture) {
            Ok((status, Ok(stdout))) if status.success() => parse_gh_pr_list(&stdout),
            _ => PullRequestLookup::Unavailable,
        }
    }

    fn view(&self, project_root: &Path, number: u64) -> PullRequestLookup {
        view_with(Gh::locate(), project_root, number)
    }
}

/// `gh pr view <number>`, through the ship module's bounded, credential-filtered `gh`.
pub(crate) fn view_with(gh: Option<Gh>, project_root: &Path, number: u64) -> PullRequestLookup {
    let Some(gh) = gh else {
        return PullRequestLookup::Unavailable;
    };
    match gh.pull_request(project_root, number) {
        Ok(found) => PullRequestLookup::Found(FoundPullRequest {
            url: found.url,
            number: found.number,
            state: found.state,
        }),
        Err(_) => PullRequestLookup::Unavailable,
    }
}

/// The GitHub CLI the ship commands use (protocol 37).
pub trait ShipTools: Send + Sync + 'static {
    /// `gh`, when it is installed.
    fn gh(&self) -> Option<Gh>;
}

/// [`ShipTools`] over the installed `gh`, looked for at each use, so one
/// installed while Interface runs is found.
pub struct SystemShipTools;

impl ShipTools for SystemShipTools {
    fn gh(&self) -> Option<Gh> {
        Gh::locate()
    }
}

/// `gh`'s JSON: an array with at most one `{number, url, state}`.
pub(crate) fn parse_gh_pr_list(stdout: &[u8]) -> PullRequestLookup {
    let Ok(Value::Array(found)) = serde_json::from_slice::<Value>(stdout) else {
        return PullRequestLookup::Unavailable;
    };
    let Some(first) = found.first() else {
        return PullRequestLookup::None;
    };
    let state = match first["state"].as_str() {
        Some("OPEN") => PullRequestState::Open,
        Some("MERGED") => PullRequestState::Merged,
        Some("CLOSED") => PullRequestState::Closed,
        _ => return PullRequestLookup::Unavailable,
    };
    match (first["number"].as_u64(), first["url"].as_str()) {
        (Some(number), Some(url)) if !url.is_empty() => {
            PullRequestLookup::Found(FoundPullRequest {
                url: url.to_owned(),
                number,
                state,
            })
        }
        _ => PullRequestLookup::Unavailable,
    }
}

/// The same `gh` the ship commands find.
fn find_gh() -> Option<PathBuf> {
    Gh::locate().map(|gh| gh.program().to_path_buf())
}
