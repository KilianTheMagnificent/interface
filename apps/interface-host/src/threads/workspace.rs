//! The commands that touch folders, git or the provider: preparing,
//! discarding and binding a workspace, branch and worktree lists, diffs,
//! restore, fork and edit, release and pull request refresh. Every rule they
//! check lives in `interface_settings::threads`; this file gathers the facts
//! (registry, liveness, folders) and performs the effects.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use interface_common::paths::lexically_normalized;
use interface_contracts::{
    engine::{SessionKind, SessionStatus},
    thread_journal::TurnOrigin,
    threads::{
        DiffScope, LineageRelation, PreparedWorkspace, ThreadCommand, ThreadControlResponse,
        ThreadLineage, ThreadWorkspace, TitleSource, WorkspaceMode, WorktreeEntry,
    },
};
use interface_engine::ForkBoundary;
use interface_review::thread_git::{
    CaptureOutcome, DiffRequest, GitWorkspaceRequest, PrepareMode, ThreadGitError,
};
use interface_settings::{
    ThreadsDocument,
    threads::{
        PreparedRecord, ReleaseOutcome, THREADS_BUSY_CODE, THREADS_INVALID_CODE,
        THREADS_NO_CHECKPOINT_CODE, THREADS_SHARED_CODE, ThreadBind, ThreadFork, ThreadRecord,
        ThreadRefusal, pending_baseline_ref, turn_ref,
    },
};

use super::{
    THREADS_FIRST_TURN_CODE, THREADS_GIT_FAILED_CODE, THREADS_NO_GIT_CODE,
    THREADS_PULL_REQUESTS_UNAVAILABLE_CODE, THREADS_UNKNOWN_SESSION_CODE,
    THREADS_WORKSPACE_FAILED_CODE, THREADS_WORKTREE_DIRTY_CODE, ThreadGit, ThreadHost, internal,
    ports::PullRequestLookup, refused, store_unavailable,
};
use tokio::sync::OwnedMutexGuard;

use crate::dispatcher::{DispatchError, runtime_dispatch_error};

impl ThreadHost {
    pub(super) async fn workspace_command(
        &self,
        command: ThreadCommand,
    ) -> Result<ThreadControlResponse, DispatchError> {
        match command {
            ThreadCommand::PrepareWorkspace {
                project_root,
                mode,
                base_ref,
                worktree_path,
                hint,
            } => {
                self.prepare_workspace(project_root, &mode, base_ref, worktree_path, hint, None)
                    .await
            }
            ThreadCommand::DiscardWorkspace { token } => self.discard_workspace(token).await,
            ThreadCommand::Bind {
                key,
                token,
                prompt,
                title_source,
            } => self.bind(key, token, prompt, title_source).await,
            ThreadCommand::ListBranches { project_root } => self.list_branches(project_root).await,
            ThreadCommand::ListWorktrees { project_root } => {
                self.list_worktrees(project_root).await
            }
            ThreadCommand::TurnDiff { key, ordinal } => self.turn_diff(key, ordinal).await,
            ThreadCommand::ThreadDiff { key, scope } => self.thread_diff(key, scope).await,
            ThreadCommand::RestoreTurn { key, ordinal } => {
                // Away's own files are never changed under its loop.
                self.away_unheld(&key)?;
                // No send starts while the files change under the agent.
                let _gate = self.hold_send_gate(&key)?;
                let restore = self.restore(key, ordinal).await?;
                let mut response = self.view_response().await?;
                response.restore = Some(restore);
                Ok(response)
            }
            ThreadCommand::ForkAt { key, keep_turns } => self.fork_at(key, keep_turns).await,
            ThreadCommand::EditFrom {
                key,
                ordinal,
                restore_files,
            } => {
                self.away_unheld(&key)?;
                self.edit_from(key, ordinal, restore_files).await
            }
            ThreadCommand::ReleaseWorkspace { key } => self.release_workspace(key).await,
            ThreadCommand::RefreshPullRequest { key } => self.refresh_pull_request(key).await,
            command @ (ThreadCommand::ShipStatus { .. }
            | ThreadCommand::Commit { .. }
            | ThreadCommand::ReviewCommit { .. }
            | ThreadCommand::CommitSelected { .. }
            | ThreadCommand::CreateBranch { .. }
            | ThreadCommand::OpenPullRequest { .. }
            | ThreadCommand::FailingChecksPrompt { .. }
            | ThreadCommand::ReviewComments { .. }
            | ThreadCommand::MergePullRequest { .. }
            | ThreadCommand::SuggestShipText { .. }) => self.ship_command(command).await,
            command @ (ThreadCommand::ScheduleSet { .. }
            | ThreadCommand::ScheduleDelete { .. }
            | ThreadCommand::ScheduleEnable { .. }
            | ThreadCommand::WatchSet { .. }) => self.schedule_command(command).await,
            ThreadCommand::ResumeAtReset { key, prompt } => self.resume_at_reset(key, prompt).await,
            other => Err(internal(&format!(
                "The thread command {} has no handler.",
                other.kind()
            ))),
        }
    }

    /// Reads the in-memory document without the blocking pool or a view.
    pub(super) async fn peek<T>(
        &self,
        read: impl FnOnce(&ThreadsDocument, u64) -> T,
    ) -> Result<T, DispatchError> {
        let guard = self.inner.store.lock().await;
        let store = guard.as_ref().map_err(|reason| store_unavailable(reason))?;
        Ok(read(store.document(), self.now()))
    }

    pub(super) async fn view_response(&self) -> Result<ThreadControlResponse, DispatchError> {
        Ok(self.read(|_, _| ()).await?.1.into())
    }

    /// Runs `work` on the blocking pool; a git failure becomes `code` with
    /// `context` before git's own message.
    pub(super) async fn git<T, F>(
        &self,
        work: F,
        code: &'static str,
        context: &'static str,
        replay_safe: bool,
    ) -> Result<T, DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn ThreadGit) -> Result<T, ThreadGitError> + Send + 'static,
    {
        let git = Arc::clone(&self.inner.ports.git);
        tokio::task::spawn_blocking(move || work(git.as_ref()))
            .await
            .map_err(|_| internal("The git task stopped unexpectedly."))?
            .map_err(|error| {
                DispatchError::new(
                    code,
                    format!("{context}: {}", error.message),
                    false,
                    replay_safe,
                )
            })
    }

    /// Prepares a launch's workspace. `lineage` is what the thread bound to
    /// it takes (Continue with's `handoff`); a New thread card passes none.
    pub(super) async fn prepare_workspace(
        &self,
        project_root: String,
        mode: &str,
        base_ref: Option<String>,
        worktree_path: Option<String>,
        hint: String,
        lineage: Option<ThreadLineage>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.prepare_workspace_vetted(
            project_root,
            mode,
            base_ref,
            worktree_path,
            hint,
            lineage,
            false,
        )
        .await
    }

    /// [`Self::prepare_workspace`]; `vetted` when the caller already checked
    /// the folder against the host's own roots (an Away loop's project).
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One prepare, validated then recorded in order.
    pub(super) async fn prepare_workspace_vetted(
        &self,
        project_root: String,
        mode: &str,
        base_ref: Option<String>,
        worktree_path: Option<String>,
        hint: String,
        lineage: Option<ThreadLineage>,
        vetted: bool,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let mode = match mode {
            "local" => PrepareMode::Local,
            "worktree" => PrepareMode::Worktree,
            "existing" => PrepareMode::Existing,
            _ => {
                return Err(invalid(
                    "Choose Local, New worktree or an existing worktree for the thread.",
                ));
            }
        };
        if !Path::new(&project_root).is_absolute() {
            return Err(invalid("Choose a project folder for the thread."));
        }
        if mode == PrepareMode::Existing
            && worktree_path
                .as_deref()
                .is_none_or(|path| !Path::new(path).is_absolute())
        {
            return Err(invalid("Choose one of this project's worktrees."));
        }
        self.peek(|document, _| document.ensure_prepared_room())
            .await?
            .map_err(refused)?;
        if !vetted {
            self.inner.ports.authority.may_open(&project_root).await?;
        }
        let token = format!("prep-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let baseline = pending_baseline_ref(&token);
        let request = GitWorkspaceRequest {
            project_root: PathBuf::from(&project_root),
            mode,
            base_ref,
            worktree_path: worktree_path.map(PathBuf::from),
            hint,
        };
        let prepared = self
            .git(
                move |git| git.prepare_workspace(&request),
                THREADS_WORKSPACE_FAILED_CODE,
                "The thread's workspace could not be prepared",
                // Only a new worktree may have left something behind.
                mode != PrepareMode::Worktree,
            )
            .await?;
        // Only a newly created worktree runs the owner's saved setup. Existing
        // folders are never modified merely by selecting them.
        let setup = if prepared.created {
            Some(
                self.setup_workspace(
                    project_root.clone(),
                    display(&prepared.cwd),
                    interface_settings::workflows::SetupPurpose::Created,
                )
                .await?,
            )
        } else if prepared.worktree_path.is_some() {
            let root = project_root.clone();
            let cwd = display(&prepared.cwd);
            self.with_store_quiet(move |store, _| Ok(store.workflows()?.setup_for(&root, &cwd)))
                .await?
        } else {
            None
        };
        if let Some(report) = &setup {
            if !Self::setup_ready(report) {
                // Keep the failed worktree for inspection. There is deliberately
                // no prepared token, so no caller can bind or launch through it.
                let root = project_root.clone();
                let saved = report.clone();
                let (workflow, view) = self
                    .with_store(move |store, _| {
                        // Display this attempt's report without writing an old read over
                        // a setup that may have completed concurrently.
                        let mut workflow = store.workflows()?.view(&root);
                        workflow.last_setup = Some(saved);
                        Ok(workflow)
                    })
                    .await?;
                let mut response = ThreadControlResponse::from(view);
                response.workflow = Some(workflow);
                response.setup = setup;
                return Ok(response);
            }
        }
        // Checkpoint only a launchable workspace, after setup, so its generated
        // dependencies are not attributed to the provider's first turn.
        let baseline = if prepared.repo_root.is_some() {
            let cwd = prepared.cwd.clone();
            self.git(
                move |git| Ok(capture_baseline(git, &cwd, baseline)),
                THREADS_WORKSPACE_FAILED_CODE,
                "The prepared baseline could not be captured",
                true,
            )
            .await?
        } else {
            None
        };
        let workspace = ThreadWorkspace {
            mode: if prepared.worktree_path.is_some() {
                WorkspaceMode::Worktree
            } else {
                WorkspaceMode::Local
            },
            project_root,
            worktree_path: prepared.worktree_path.as_deref().map(display),
            branch: prepared.branch.clone(),
            base_ref: prepared.base_ref.clone(),
            owned: prepared.created,
            missing: None,
            kept: None,
        };
        let record = PreparedRecord {
            token: token.clone(),
            cwd: display(&prepared.cwd),
            workspace: workspace.clone(),
            git_root: prepared.repo_root.as_deref().map(display),
            created: prepared.created,
            baseline_ref: baseline,
            created_at: 0,
            lineage,
        };
        let saved = record.clone();
        match self
            .edit(move |document, now| {
                document.add_prepared(PreparedRecord {
                    created_at: now,
                    ..saved
                })
            })
            .await
        {
            Ok(((), view)) => Ok(ThreadControlResponse {
                setup,
                prepared: Some(PreparedWorkspace {
                    token,
                    cwd: record.cwd,
                    workspace,
                }),
                ..view.into()
            }),
            Err(error) => {
                if let Err(cleanup) = self.discard_prepared(record).await {
                    tracing::warn!(code = %cleanup.code, "an unrecorded prepared workspace was kept");
                }
                Err(error)
            }
        }
    }

    /// Forgets a launch's prepared workspace and removes what it made. A
    /// worktree something already works in (the launch did start after all)
    /// stays, and so does its entry, for the hourly sweep to try again.
    async fn discard_workspace(
        &self,
        token: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.discard_unused_prepared(token).await?;
        self.view_response().await
    }

    /// Binds a launch to its thread: validates the session, files the
    /// prepared baseline under the thread when git can, and records turn 0.
    pub(super) async fn bind(
        &self,
        key: String,
        token: Option<String>,
        prompt: String,
        title_source: Option<TitleSource>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let summary = self
            .inner
            .ports
            .sessions
            .session(&key)
            .map_err(|error| super::registry_unavailable(&error))?
            .ok_or_else(|| {
                DispatchError::new(
                    THREADS_UNKNOWN_SESSION_CODE,
                    "That session is not in Interface. Refresh, then start the thread again.",
                    false,
                    true,
                )
            })?;
        if summary.kind == Some(SessionKind::Agent) {
            return Err(invalid(
                "Agent sessions belong to the Agent surface; they are never threads.",
            ));
        }
        let mut start_ref = None;
        if let Some(token) = token.clone() {
            let baseline = self
                .peek(move |document, _| {
                    let prepared = document.prepared(&token)?;
                    Some((prepared.baseline_ref.clone()?, prepared.git_root.clone()?))
                })
                .await?;
            if let Some((from, root)) = baseline {
                let to = turn_ref(&key, 0, false);
                let target = to.clone();
                let moved = self
                    .git(
                        move |git| git.move_ref(Path::new(&root), &from, &target),
                        THREADS_GIT_FAILED_CODE,
                        "The launch checkpoint could not be filed",
                        true,
                    )
                    .await;
                // A baseline that stays under its pending name still serves
                // turn 0: the record keeps whichever full ref name holds it.
                start_ref = match moved {
                    Ok(()) => Some(to),
                    Err(error) => {
                        tracing::warn!(code = %error.code, "a launch checkpoint kept its pending name");
                        None
                    }
                };
            }
        }
        let launch_prompt = prompt.clone();
        let bind = ThreadBind {
            key: key.clone(),
            token,
            prompt,
            title_source,
            start_ref,
        };
        let target = key.clone();
        let host = self.clone();
        let ((finished, title), view) = self
            .edit(move |document, now| {
                let mut finished = document.bind(bind, now)?;
                // Under the store lock, as for a send: the launch run is open
                // in the tap before any reconcile can see it in the record.
                if document.record(&target).is_some_and(ThreadRecord::running)
                    && !host.run_tracked(&target)
                {
                    // A launch run the tap already saw end (a provider that
                    // failed within milliseconds) ends turn 0 here: the new
                    // generation makes the worker drop that end as stale.
                    let ended = host.run_ended_untracked(&target);
                    let generation = host.open_run(&target);
                    if let Some((at, outcome)) = ended {
                        host.close_run_of(&target, generation);
                        finished = document.run_ended(&target, at, outcome).or(finished);
                    }
                }
                Ok((finished, document.wants_generated_title(&target)))
            })
            .await?;
        // The journal puts the launch prompt before what its run said.
        self.journal_prompt(&key, TurnOrigin::Launch, &launch_prompt);
        if title {
            // Its launch run already completed: the title it would have had
            // as its prompt went out, with the answer too.
            self.title_after_first_run(&key);
        } else {
            // The launch prompt went out with the launch: named now, while
            // its turn runs.
            self.title_at_first_prompt(&key);
        }
        if let Some(finished) = finished {
            // Its launch run ended before the bind: the tap's last ended run.
            self.capture_end(finished, self.ended_run_paths(&key));
        }
        Ok(view.into())
    }

    async fn list_branches(
        &self,
        project_root: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.inner.ports.authority.may_open(&project_root).await?;
        let branches = self
            .git(
                move |git| git.list_branches(Path::new(&project_root)),
                THREADS_GIT_FAILED_CODE,
                "The project's branches could not be listed",
                true,
            )
            .await?;
        let mut response = self.view_response().await?;
        response.branches = Some(branches);
        Ok(response)
    }

    async fn list_worktrees(
        &self,
        project_root: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.inner.ports.authority.may_open(&project_root).await?;
        let worktrees = self
            .git(
                move |git| git.list_worktrees(Path::new(&project_root)),
                THREADS_GIT_FAILED_CODE,
                "The project's worktrees could not be listed",
                true,
            )
            .await?;
        let (owners, view) = self.read(|document, _| worktree_owners(document)).await?;
        let entries = worktrees
            .into_iter()
            .map(|(path, branch)| WorktreeEntry {
                owner_key: owners.get(&lexically_normalized(&path)).cloned(),
                path: display(&path),
                branch,
            })
            .collect();
        Ok(ThreadControlResponse {
            worktrees: Some(entries),
            ..view.into()
        })
    }

    /// A turn's diff: its start checkpoint against its end (or the working
    /// tree). A turn that ended while others shared its checkout reads only
    /// the files its own edits named
    /// ([`TurnRecord::diff_paths`](interface_settings::threads::TurnRecord::diff_paths)).
    async fn turn_diff(
        &self,
        key: String,
        ordinal: u32,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (dir, base, head, paths) = self
            .peek(move |document, _| {
                let record = document.record(&key).ok_or_else(ThreadRefusal::unknown)?;
                let dir = git_dir(record)?;
                let turn = record
                    .turns
                    .iter()
                    .find(|turn| turn.ordinal == ordinal)
                    .filter(|turn| turn.start_ref.is_some())
                    .ok_or_else(|| no_checkpoint(ordinal))?;
                Ok((
                    dir,
                    turn.start_ref.clone().unwrap_or_default(),
                    turn.end_ref.clone(),
                    turn.diff_paths()?.map(<[String]>::to_vec),
                ))
            })
            .await?
            .map_err(refused)?;
        self.diff_response(dir, DiffScope::Turn, DiffBase::Ref(base), head, paths)
            .await
    }

    pub(super) async fn thread_diff(
        &self,
        key: String,
        scope: DiffScope,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (dir, base, head, paths) = self
            .peek(move |document, _| {
                let record = document.record(&key).ok_or_else(ThreadRefusal::unknown)?;
                let dir = git_dir(record)?;
                let checkpointed = || record.turns.iter().filter(|turn| turn.start_ref.is_some());
                // A switched thread's whole diff starts at its earliest
                // checkpoint (T3 section 5): by when its turn was sent, the
                // first segment's (the chain's id) first on a tie. A switch
                // back makes the first segment the head again, last in
                // activation order, so that order cannot say.
                let members = document.chain_members(&key);
                let first_checkpoint = members
                    .iter()
                    .flat_map(|member| {
                        let later = member
                            .chain
                            .as_ref()
                            .is_some_and(|link| link.id != member.key);
                        member.turns.iter().map(move |turn| (later, turn))
                    })
                    .filter(|(_, turn)| turn.start_ref.is_some())
                    .min_by_key(|(later, turn)| (turn.sent_at, *later))
                    .map(|(_, turn)| turn);
                let mut paths = None;
                let (base, head) = match scope {
                    DiffScope::Turn => {
                        let turn = checkpointed().next_back().ok_or_else(|| no_checkpoint(0))?;
                        paths = turn.diff_paths()?.map(<[String]>::to_vec);
                        (
                            DiffBase::Ref(turn.start_ref.clone().unwrap_or_default()),
                            turn.end_ref.clone(),
                        )
                    }
                    DiffScope::Thread => {
                        let turn = first_checkpoint.ok_or_else(|| no_checkpoint(0))?;
                        (
                            DiffBase::Ref(turn.start_ref.clone().unwrap_or_default()),
                            None,
                        )
                    }
                    DiffScope::Uncommitted => (DiffBase::Ref("HEAD".into()), None),
                    DiffScope::Branch => {
                        let base_ref = record
                            .workspace
                            .as_ref()
                            .filter(|workspace| workspace.mode == WorkspaceMode::Worktree)
                            .and_then(|workspace| workspace.base_ref.clone())
                            .ok_or_else(|| {
                                ThreadRefusal::new(
                                    THREADS_NO_GIT_CODE,
                                    "Only a worktree thread with a base branch has a branch diff.",
                                )
                            })?;
                        (DiffBase::MergeBase(base_ref), None)
                    }
                };
                Ok((dir, base, head, paths))
            })
            .await?
            .map_err(refused)?;
        self.diff_response(dir, scope, base, head, paths).await
    }

    async fn diff_response(
        &self,
        dir: String,
        scope: DiffScope,
        base: DiffBase,
        head: Option<String>,
        paths: Option<Vec<String>>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let diff = self
            .git(
                move |git| {
                    let dir = Path::new(&dir);
                    let base = match base {
                        DiffBase::Ref(base) => base,
                        DiffBase::MergeBase(base_ref) => {
                            git.merge_base(dir, &base_ref)?.ok_or_else(|| {
                                ThreadGitError::new(
                                    "thread-git-no-merge-base",
                                    format!("the branch shares no history with {base_ref}"),
                                )
                            })?
                        }
                    };
                    git.diff(
                        dir,
                        &DiffRequest {
                            scope,
                            base,
                            head,
                            paths,
                        },
                    )
                },
                THREADS_GIT_FAILED_CODE,
                "The thread's changes could not be read",
                true,
            )
            .await?;
        let mut response = self.view_response().await?;
        response.diff = Some(diff);
        Ok(response)
    }

    /// Restores turn `ordinal`'s start checkpoint when the store's isolation
    /// rules allow it, capturing a recovery ref first.
    async fn restore(
        &self,
        key: String,
        ordinal: u32,
    ) -> Result<interface_contracts::threads::RestoreResult, DispatchError> {
        let busy = self.busy(&key);
        let uses = self.session_uses()?;
        let plan = self
            .peek(move |document, now| {
                let present = document
                    .record(&key)
                    .and_then(|record| record.workspace.as_ref()?.worktree_path.clone())
                    .is_some_and(|path| Path::new(&path).is_dir());
                document.restore_plan(&key, ordinal, busy, present, &uses, now)
            })
            .await?
            .map_err(refused)?;
        let recovery = plan.recovery_ref.clone();
        self.git(
            move |git| git.restore_to(Path::new(&plan.dir), &plan.start_ref, &plan.recovery_ref),
            THREADS_GIT_FAILED_CODE,
            "The files could not be restored",
            false,
        )
        .await
        .map_err(|mut error| {
            error.message = format!(
                "{} The files from before the restore are kept at {recovery}.",
                error.message
            );
            error
        })
    }

    /// Whether `key`'s agent is working or waiting, by the registry or a run
    /// the tap still holds open.
    pub(super) fn busy(&self, key: &str) -> bool {
        self.inner
            .ports
            .sessions
            .session(key)
            .ok()
            .flatten()
            .is_some_and(|summary| {
                matches!(
                    summary.status,
                    SessionStatus::Working | SessionStatus::Waiting
                )
            })
            || self.run_tracked(key)
    }

    /// A fork cuts a finished transcript: Claude cannot tell a running turn
    /// from its transcript, so no provider forks a busy session here.
    fn refuse_busy_fork(&self, key: &str) -> Result<(), DispatchError> {
        if self.busy(key) {
            return Err(still_working());
        }
        Ok(())
    }

    /// `key`'s send gate, held so no send starts until the guard drops. A
    /// gate already held is a send on its way to the provider (an ACP send
    /// holds it for its whole turn), so the command refuses instead of
    /// waiting behind it.
    pub(super) fn hold_send_gate(&self, key: &str) -> Result<OwnedMutexGuard<()>, DispatchError> {
        self.send_gate(key)
            .try_lock_owned()
            .map_err(|_| still_working())
    }

    /// The provider prompt `key`'s first `keep_turns` recorded turns end on.
    pub(super) async fn fork_boundary(
        &self,
        key: &str,
        keep_turns: u32,
    ) -> Result<ForkBoundary, DispatchError> {
        let key = key.to_owned();
        let point = self
            .peek(move |document, _| document.fork_point(&key, keep_turns))
            .await?
            .map_err(refused)?;
        Ok(ForkBoundary {
            prompt_digest: point.prompt_digest,
            later_same_digest: point.later_same_digest,
        })
    }

    pub(super) async fn require_record(&self, key: &str) -> Result<(), DispatchError> {
        let key = key.to_owned();
        self.peek(move |document, _| document.record(&key).map(|_| ()))
            .await?
            .ok_or_else(|| refused(ThreadRefusal::unknown()))
    }

    async fn fork_at(
        &self,
        key: String,
        keep_turns: u32,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if keep_turns == 0 {
            return Err(first_turn(
                "A fork keeps at least the first turn. Start a new thread instead.",
            ));
        }
        self.require_record(&key).await?;
        let _gate = self.hold_send_gate(&key)?;
        self.refuse_busy_fork(&key)?;
        let boundary = self.fork_boundary(&key, keep_turns).await?;
        let title = self
            .inner
            .ports
            .sessions
            .session(&key)
            .ok()
            .flatten()
            .map(|summary| summary.title);
        let child = self
            .inner
            .ports
            .sessions
            .fork_at(&key, boundary)
            .await
            .map_err(runtime_dispatch_error)?;
        let view = self
            .record_fork(ThreadFork {
                parent: key,
                child: child.clone(),
                relation: LineageRelation::Fork,
                kept_turns: keep_turns,
            })
            .await?;
        if let Some(title) = title {
            if let Err(error) = self
                .inner
                .ports
                .sessions
                .rename(&child, format!("{title} (fork)"))
                .await
            {
                tracing::warn!(%error, "a forked thread kept its parent's title");
            }
        }
        self.schedule_registry_push();
        Ok(ThreadControlResponse {
            key: Some(child),
            ..view.into()
        })
    }

    /// Edit from turn `ordinal`: restore its files when asked (and allowed),
    /// fork keeping the turns before it, hand the worktree to the new thread
    /// and settle the original as edited. Answers the new key and the turn's
    /// prompt when the store holds all of it.
    async fn edit_from(
        &self,
        key: String,
        ordinal: u32,
        restore_files: bool,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if ordinal == 0 {
            return Err(first_turn(
                "Editing the first prompt starts over: start a new thread in the same workspace instead.",
            ));
        }
        self.require_record(&key).await?;
        // Held through the restore, the fork and the close: no send lands
        // between the files going back and the conversation being cut.
        let gate = self.hold_send_gate(&key)?;
        self.refuse_busy_fork(&key)?;
        let boundary = self.fork_boundary(&key, ordinal).await?;
        let target = key.clone();
        let prompt = self
            .peek(move |document, _| {
                document
                    .record(&target)?
                    .turns
                    .iter()
                    .find(|turn| turn.ordinal == ordinal)?
                    .prompt()
                    .map(str::to_owned)
            })
            .await?;
        let restored = if restore_files {
            Some(self.restore(key.clone(), ordinal).await?)
        } else {
            None
        };
        let child = match self.inner.ports.sessions.fork_at(&key, boundary).await {
            Ok(child) => child,
            Err(error) => {
                let mut error = runtime_dispatch_error(error);
                if let Some(restored) = &restored {
                    error.message = format!(
                        "The files were restored to before prompt {} (the previous files are kept at {}), but the fork failed: {}",
                        ordinal + 1,
                        restored.recovery_ref,
                        error.message
                    );
                    error.replay_safe = false;
                    error.retryable = false;
                }
                return Err(error);
            }
        };
        let view = self
            .record_fork(ThreadFork {
                parent: key.clone(),
                child: child.clone(),
                relation: LineageRelation::Edit,
                kept_turns: ordinal,
            })
            .await?;
        self.close_edited(&key, gate).await;
        self.schedule_registry_push();
        Ok(ThreadControlResponse {
            key: Some(child),
            prompt,
            restore: restored,
            ..view.into()
        })
    }

    /// Closes the session of a thread an edit just settled, so its open
    /// provider handle stops holding the worktree the new thread now owns
    /// (a restore there would otherwise refuse). The edit refused a busy
    /// original and still holds its send gate; this checks again, so a run
    /// the provider started on its own is never cut off. A close that fails
    /// keeps the session open and changes nothing else: the edit has
    /// happened either way.
    pub(super) async fn close_edited(&self, key: &str, _gate: OwnedMutexGuard<()>) {
        if self.busy(key) {
            tracing::info!("an edited thread's session is working again; it stays open");
            return;
        }
        if let Err(error) = self.inner.ports.sessions.close(key).await {
            tracing::warn!(%error, "an edited thread's session stayed open");
        }
    }

    /// Records a fork the provider already made; a failure here is local, so
    /// the fork exists either way.
    async fn record_fork(
        &self,
        fork: ThreadFork,
    ) -> Result<interface_contracts::threads::ThreadsView, DispatchError> {
        let child = fork.child.clone();
        self.edit(move |document, now| document.bind_fork(&fork, now))
            .await
            .map(|((), view)| view)
            .map_err(|error| {
                DispatchError::new(
                    error.code,
                    format!(
                        "The provider made the new session {child}, but its thread could not be recorded: {}",
                        error.message
                    ),
                    false,
                    false,
                )
            })
    }

    pub(super) fn schedule_registry_push(&self) {
        if let Some(pushes) = &self.inner.registry_pushes {
            pushes.0.schedule();
        }
    }

    async fn release_workspace(&self, key: String) -> Result<ThreadControlResponse, DispatchError> {
        let uses = self.session_uses()?;
        let own_live = uses
            .iter()
            .any(|session| session.key == key && session.live);
        let target = key.clone();
        let plan = self
            .peek(move |document, _| document.release_plan(&target, own_live, &uses))
            .await?
            .map_err(refused)?;
        match self
            .remove_worktree(plan.git_root, plan.worktree_path)
            .await
        {
            outcome @ (ReleaseOutcome::Removed | ReleaseOutcome::Missing) => {
                // An archive that kept it no longer stands. The worktree is
                // gone either way, so a failed note is only logged.
                match self
                    .edit(move |document, now| {
                        document.record_owner_release(&key, outcome, now);
                        Ok(())
                    })
                    .await
                {
                    Ok(((), view)) => Ok(view.into()),
                    Err(error) => {
                        tracing::warn!(code = %error.code, "a worktree release could not be recorded");
                        self.view_response().await
                    }
                }
            }
            ReleaseOutcome::KeptDirty => Err(DispatchError::new(
                THREADS_WORKTREE_DIRTY_CODE,
                "The worktree has uncommitted changes, so it was kept with its branch. Commit or discard them, then release it again.",
                false,
                true,
            )),
            ReleaseOutcome::KeptIgnored => Err(DispatchError::new(
                THREADS_WORKTREE_DIRTY_CODE,
                "The worktree holds ignored files (such as dependencies, build output or local settings), so it was kept with its branch. Delete the folder yourself if you no longer need them.",
                false,
                true,
            )),
            ReleaseOutcome::InUse => Err(DispatchError::new(
                THREADS_SHARED_CODE,
                "Something is still working in the worktree, so it was kept. Stop it, then release again.",
                false,
                true,
            )),
            ReleaseOutcome::Failed => Err(DispatchError::new(
                THREADS_GIT_FAILED_CODE,
                "The worktree could not be released. Check it in Finder, then try again.",
                false,
                false,
            )),
        }
    }

    /// Looks the thread's pull request up now. `gh` missing or signed out
    /// changes nothing, silently.
    async fn refresh_pull_request(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let check = self
            .peek(move |document, now| document.pull_requests_due(now, Some(&key)).pop())
            .await?
            .ok_or_else(|| {
                DispatchError::new(
                    THREADS_PULL_REQUESTS_UNAVAILABLE_CODE,
                    "Pull requests are tracked for unsettled worktree threads with a branch, and threads that already have a pull request, while tracking is on in Threads settings.",
                    false,
                    true,
                )
            })?;
        if self.look_up_pull_request(check).await == PullRequestLookup::Unavailable {
            tracing::info!("a pull request refresh found gh unavailable");
        }
        self.view_response().await
    }
}

/// Where a diff starts.
enum DiffBase {
    Ref(String),
    /// The merge base of this ref and HEAD.
    MergeBase(String),
}

fn capture_baseline(git: &dyn ThreadGit, cwd: &Path, refname: String) -> Option<String> {
    match git.capture_checkpoint(cwd, &refname) {
        Ok(CaptureOutcome::Captured { .. }) => Some(refname),
        Ok(CaptureOutcome::Skipped { reason }) => {
            tracing::info!(%reason, "a thread starts without a launch checkpoint");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "a thread's launch checkpoint failed");
            None
        }
    }
}

/// Owned worktrees by their normalized path.
fn worktree_owners(document: &ThreadsDocument) -> HashMap<PathBuf, String> {
    document
        .records
        .iter()
        .filter_map(|record| {
            let workspace = record.workspace.as_ref()?;
            (workspace.mode == WorkspaceMode::Worktree && workspace.owned).then_some(())?;
            let path = workspace.worktree_path.as_deref()?;
            Some((lexically_normalized(Path::new(path)), record.key.clone()))
        })
        .collect()
}

/// The folder git commands run in for `record`, refusing a thread outside git.
fn git_dir(record: &ThreadRecord) -> Result<String, ThreadRefusal> {
    record
        .git_root
        .as_ref()
        .and(record.workspace_dir())
        .map(str::to_owned)
        .ok_or_else(|| {
            ThreadRefusal::new(
                THREADS_NO_GIT_CODE,
                "This thread is not in a git repository, so it has no checkpoints or diffs.",
            )
        })
}

fn no_checkpoint(ordinal: u32) -> ThreadRefusal {
    ThreadRefusal::new(
        THREADS_NO_CHECKPOINT_CODE,
        format!("Turn {} has no checkpoint.", ordinal + 1),
    )
}

fn still_working() -> DispatchError {
    DispatchError::new(
        THREADS_BUSY_CODE,
        "The agent is still working. Wait for it to finish, or stop it, then try again.",
        false,
        true,
    )
}

fn invalid(message: &str) -> DispatchError {
    DispatchError::new(THREADS_INVALID_CODE, message, false, true)
}

fn first_turn(message: &str) -> DispatchError {
    DispatchError::new(THREADS_FIRST_TURN_CODE, message, false, true)
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
