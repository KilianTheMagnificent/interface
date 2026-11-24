//! The ship commands (protocol 37): a thread's checkout read for shipping,
//! commit, a new branch, push and pull request, the failing checks' prompt,
//! review comments and merge. An adapter: the git and `gh` mechanics and the
//! rules for what Commit stages and when a push is refused live in
//! `interface_review::thread_git::ship`, and what a thread's work covers in
//! `interface_settings::threads` (`ship_scope`, `commit_message`).
//!
//! Fail-closed: a ship effect is answered as done (seen, or found on a
//! re-read), refused (an error: nothing changed), or unknown (`shipped`
//! with certainty unknown: the re-read failed too). Nothing here retries,
//! and every answer carries a fresh read of the checkout.

use std::{collections::BTreeSet, fmt::Write as _, path::Path, sync::Arc};

use interface_common::sync::lock;
use interface_contracts::{
    engine::EngineAuth,
    threads::{
        ChecksState, CommitChoice, DiffScope, GhState, MergeMethod, PullRequestState, ShipAction,
        ShipCertainty, ShipOutcome, ShipTextKind, ThreadCommand, ThreadControlResponse, ThreadShip,
    },
};
use interface_review::thread_git::{
    DiffRequest, ThreadGitError,
    ship::{
        self, Gh, GhAuth, GhPullRequest, ShipEffect, ShipGitState, check_run, checks_summary,
        commit_blocked, commit_selection, on_default_branch, push_blocked, suggested_branch,
    },
    ship_text,
};
use interface_settings::threads::{
    FoundPullRequest, ShipScope, THREADS_BUSY_CODE, THREADS_INVALID_CODE, commit_message,
};
use tokio::sync::OwnedMutexGuard;

use super::{
    THREADS_CHECKS_NOT_FAILING_CODE, THREADS_GH_UNAVAILABLE_CODE, THREADS_GIT_FAILED_CODE,
    THREADS_NO_PULL_REQUEST_CODE, THREADS_SHIP_BLOCKED_CODE, THREADS_SHIP_REFUSED_CODE,
    THREADS_SHIP_STALE_CODE, THREADS_SHIP_TEXT_FAILED_CODE, THREADS_SHIP_TEXT_UNAVAILABLE_CODE,
    ThreadHost, internal, refused,
};
use crate::dispatcher::DispatchError;

const GH_MISSING_FIX: &str =
    "Install the GitHub CLI (brew install gh), then run gh auth login in a terminal.";
const GH_SIGNED_OUT_FIX: &str =
    "Run gh auth login in a terminal to sign the GitHub CLI in, then check again.";
const GH_SILENT_FIX: &str = "The GitHub CLI didn't answer. Check that gh auth status works in a terminal, then check again.";
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_TITLE_CHARS: usize = 256;
/// A failing-checks prompt quotes at most this many logs, within this many bytes.
const PROMPT_LOGS: usize = 3;
const PROMPT_BYTES: usize = 16 * 1024;

/// What `gh` answered about a thread's pull request.
struct RemoteRead {
    gh: GhState,
    gh_fix: Option<String>,
    pull_request: Option<GhPullRequest>,
    checks: Option<interface_contracts::threads::ShipChecks>,
}

impl ThreadHost {
    pub(super) async fn ship_command(
        &self,
        command: ThreadCommand,
    ) -> Result<ThreadControlResponse, DispatchError> {
        match command {
            ThreadCommand::ShipStatus { key, remote } => {
                let scope = self.ship_scope(&key).await?;
                let ship = self.read_ship(&key, &scope, remote).await?;
                self.ship_response(ship, None).await
            }
            ThreadCommand::Commit {
                key,
                message,
                paths,
            } => self.commit(key, message, paths).await,
            ThreadCommand::ReviewCommit { key } => {
                let scope = self.ship_scope(&key).await?;
                let reviewed = blocking(move || {
                    interface_review::thread_git::commit::review(
                        Path::new(&scope.dir),
                        Some(&scope.attributed),
                    )
                })
                .await?
                .map_err(git_failed)?;
                let mut response = self.view_response().await?;
                response.commit_review = Some(reviewed);
                Ok(response)
            }
            ThreadCommand::CommitSelected {
                key,
                revision,
                message,
                choices,
            } => self.commit_selected(key, revision, message, choices).await,
            ThreadCommand::CreateBranch { key, branch } => self.create_branch(key, branch).await,
            ThreadCommand::OpenPullRequest { key, title, body } => {
                self.open_pull_request(key, title, body).await
            }
            ThreadCommand::FailingChecksPrompt { key } => self.failing_checks_prompt(key).await,
            ThreadCommand::ReviewComments { key } => self.review_comments(key).await,
            ThreadCommand::MergePullRequest {
                key,
                number,
                method,
            } => self.merge_pull_request(key, number, method).await,
            ThreadCommand::SuggestShipText { key, what, paths } => {
                self.suggest_ship_text(key, what, paths).await
            }
            other => Err(internal(&format!(
                "The thread command {} is not a ship command.",
                other.kind()
            ))),
        }
    }

    /// Write with AI (protocol 67, T3 section 12): the changes Commit would
    /// stage (or `paths` of them) or the branch's committed work since its
    /// default branch, with the thread's prompts, bounded and
    /// credential-filtered (`thread_git::ship_text`), written up by one
    /// tool-less Claude call on Claude's default account. Changes nothing.
    async fn suggest_ship_text(
        &self,
        key: String,
        what: ShipTextKind,
        paths: Option<Vec<String>>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if !self.inner.ports.titles.available() {
            return Err(ship_text_unavailable(
                "Write with AI uses Claude, and Claude is not installed.",
            ));
        }
        let claude_signed_out = self.probes().await.iter().any(|probe| {
            probe.engine.as_str() == "claude"
                && probe.info.as_ref().and_then(|info| info.auth) == Some(EngineAuth::SignedOut)
        });
        if claude_signed_out {
            return Err(ship_text_unavailable(
                "Write with AI uses Claude, and Claude is signed out. Sign in, then try again.",
            ));
        }
        let scope = self.ship_scope(&key).await?;
        let dir = scope.dir.clone();
        let state = blocking(move || ship::ship_state(Path::new(&dir), false))
            .await?
            .map_err(git_failed)?;
        let request = match what {
            ShipTextKind::Commit => {
                let selection = commit_selection(&state.changes, scope.touched.as_ref());
                let paths = match paths {
                    // Only files that are changed now, never a credential-shaped one.
                    Some(paths) => state
                        .changes
                        .iter()
                        .filter(|change| !change.credential && paths.contains(&change.path))
                        .map(|change| change.path.clone())
                        .collect::<Vec<_>>(),
                    None => selection.paths(),
                };
                if paths.is_empty() {
                    return Err(invalid(
                        "There are no changes to write a commit message for.",
                    ));
                }
                DiffRequest {
                    scope: DiffScope::Uncommitted,
                    base: "HEAD".into(),
                    head: None,
                    paths: Some(paths),
                }
            }
            ShipTextKind::PullRequest => {
                let base_ref = match (&state.remote, &state.default_branch) {
                    (Some(remote), Some(branch)) => format!("{remote}/{branch}"),
                    (None, Some(branch)) => branch.clone(),
                    _ => {
                        return Err(invalid(
                            "Interface can't tell which branch this pull request goes into, so there is nothing to compare.",
                        ));
                    }
                };
                let dir = scope.dir.clone();
                let base = blocking(move || {
                    interface_review::thread_git::merge_base(Path::new(&dir), &base_ref)
                })
                .await?
                .map_err(git_failed)?
                .ok_or_else(|| invalid("This branch shares no history with its default branch."))?;
                DiffRequest {
                    scope: DiffScope::Branch,
                    base,
                    head: Some("HEAD".into()),
                    paths: None,
                }
            }
        };
        let dir = scope.dir.clone();
        let diff = blocking(move || interface_review::thread_git::diff(Path::new(&dir), &request))
            .await?
            .map_err(git_failed)?;
        if diff.files.is_empty() {
            return Err(invalid("There are no changes to write about."));
        }
        let material = ship_text::ship_text_material(&diff, &scope.prompts);
        let text = self
            .inner
            .ports
            .titles
            .write_ship_text(what, material)
            .await
            .map_err(|reason| {
                DispatchError::new(
                    THREADS_SHIP_TEXT_FAILED_CODE,
                    format!("Claude could not write it ({reason}). Nothing was changed."),
                    true,
                    true,
                )
            })?;
        let mut response = self.view_response().await?;
        response.ship_text = Some(text);
        Ok(response)
    }

    /// What `key`'s work covers, from its record.
    async fn ship_scope(&self, key: &str) -> Result<ShipScope, DispatchError> {
        let target = key.to_owned();
        self.peek(move |document, _| document.ship_scope(&target))
            .await?
            .map_err(refused)
    }

    /// `key`'s title in the registry, for its commit message.
    fn ship_title(&self, key: &str) -> String {
        self.inner
            .ports
            .sessions
            .session(key)
            .ok()
            .flatten()
            .map(|summary| summary.title)
            .unwrap_or_default()
    }

    /// One commit, branch, push or merge at a time per thread: a second
    /// click while one runs is refused, never queued behind it.
    fn hold_ship_gate(&self, key: &str) -> Result<OwnedMutexGuard<()>, DispatchError> {
        let gate = {
            let mut gates = lock(&self.inner.ship_gates);
            gates.retain(|_, gate| Arc::strong_count(gate) > 1);
            Arc::clone(gates.entry(key.to_owned()).or_default())
        };
        gate.try_lock_owned().map_err(|_| {
            DispatchError::new(
                THREADS_BUSY_CODE,
                "Interface is already committing, pushing or merging this thread's work. Wait for it to finish.",
                false,
                true,
            )
        })
    }

    /// `gh`, signed in, or the refusal that says how to make it so.
    async fn ready_gh(&self, dir: &str) -> Result<Gh, DispatchError> {
        let tools = Arc::clone(&self.inner.ports.ship);
        let dir = dir.to_owned();
        let (gh, auth) = blocking(move || {
            let gh = tools.gh();
            let auth = gh.as_ref().map(|gh| gh.auth(Path::new(&dir)));
            (gh, auth)
        })
        .await?;
        match (gh, auth) {
            (Some(gh), Some(GhAuth::Ready)) => Ok(gh),
            (None, _) => Err(gh_unavailable(GH_MISSING_FIX)),
            (Some(_), Some(GhAuth::SignedOut)) => Err(gh_unavailable(GH_SIGNED_OUT_FIX)),
            (Some(_), _) => Err(gh_unavailable(GH_SILENT_FIX)),
        }
    }

    /// Reads `key`'s checkout (and, with `remote`, its pull request and
    /// checks through `gh`) into the wire's [`ThreadShip`]. A pull request
    /// found for the thread's branch that the record lacks is recorded, so
    /// it is tracked from then on.
    async fn read_ship(
        &self,
        key: &str,
        scope: &ShipScope,
        remote: bool,
    ) -> Result<ThreadShip, DispatchError> {
        let title = self.ship_title(key);
        let tools = Arc::clone(&self.inner.ports.ship);
        let dir = scope.dir.clone();
        let recorded = scope.pull_request.as_ref().map(|pr| pr.number);
        let (state, read) = blocking(move || {
            let state = ship::ship_state(Path::new(&dir), remote)?;
            let read = remote.then(|| read_remote(tools.gh(), Path::new(&dir), &state, recorded));
            Ok::<_, ThreadGitError>((state, read))
        })
        .await?
        .map_err(git_failed)?;
        let selection = commit_selection(&state.changes, scope.touched.as_ref());
        let suggested = (!scope.worktree && on_default_branch(&state))
            .then(|| suggested_branch(if title.is_empty() { "thread" } else { &title }, key));
        if let Some(found) = read.as_ref().and_then(|read| read.pull_request.as_ref())
            && scope
                .pull_request
                .as_ref()
                .is_none_or(|known| known.number != found.number || known.state != found.state)
        {
            self.record_found(key, found).await?;
        }
        let read = read.unwrap_or(RemoteRead {
            gh: GhState::Unavailable,
            gh_fix: None,
            pull_request: None,
            checks: None,
        });
        Ok(ThreadShip {
            key: key.to_owned(),
            branch: state.branch.clone(),
            default_branch: state.default_branch.clone(),
            remote: state.remote.clone(),
            upstream: state.upstream.clone(),
            ahead: state.ahead,
            behind: state.behind,
            files: selection.ship_files(),
            left_out: selection.left_out,
            files_unknown: scope.touched_unknown,
            commit_blocked: commit_blocked(&state),
            message: commit_message(&title, &scope.prompts),
            push_blocked: push_blocked(&state),
            suggested_branch: suggested,
            gh: read.gh,
            gh_fix: read.gh_fix,
            remote_read: remote,
            pull_request: read.pull_request.as_ref().map(GhPullRequest::ship),
            checks: read.checks,
        })
    }

    /// Records the pull request `found` on `key`: shown, tracked, and
    /// settled through settle-on-merge once it merges.
    async fn record_found(&self, key: &str, found: &GhPullRequest) -> Result<(), DispatchError> {
        let target = key.to_owned();
        let found = FoundPullRequest {
            url: found.url.clone(),
            number: found.number,
            state: found.state,
        };
        self.edit_quiet(move |document, now| {
            document.record_pull_request(&target, Some(found), now);
            Ok(())
        })
        .await
    }

    async fn ship_response(
        &self,
        ship: ThreadShip,
        shipped: Option<ShipOutcome>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let mut response = self.view_response().await?;
        response.ship = Some(ship);
        response.shipped = shipped;
        Ok(response)
    }

    /// Answers an effect that happened (`Done`) or cannot be told
    /// (`Unknown`) with a fresh read; a refused one is an error.
    async fn ship_answer<T>(
        &self,
        key: &str,
        action: ShipAction,
        effect: ShipEffect<T>,
        done: impl FnOnce(T) -> String,
        remote: bool,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (certainty, message) = match effect {
            ShipEffect::Done(value) => (ShipCertainty::Done, done(value)),
            ShipEffect::Unknown(message) => (ShipCertainty::Unknown, message),
            ShipEffect::Refused(message) => return Err(ship_refused(message)),
        };
        // An effect already happened or may have happened. A failed status
        // refresh cannot turn that into a replay-safe error.
        let post_effect_error = |_| {
            DispatchError::new(
                THREADS_GIT_FAILED_CODE,
                "The shipping action finished, but its result could not be refreshed. Inspect the checkout before trying another action.",
                false,
                false,
            )
        };
        let scope = self.ship_scope(key).await.map_err(post_effect_error)?;
        let ship = self
            .read_ship(key, &scope, remote)
            .await
            .map_err(post_effect_error)?;
        self.ship_response(
            ship,
            Some(ShipOutcome {
                action,
                certainty,
                message,
            }),
        )
        .await
        .map_err(post_effect_error)
    }

    /// Commits exactly `paths`, which must be what Commit stages now.
    async fn commit_selected(
        &self,
        key: String,
        revision: String,
        message: String,
        choices: Vec<CommitChoice>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if self.busy(&key) {
            return Err(still_working("commit"));
        }
        let _send = self.hold_send_gate(&key)?;
        let _ship = self.hold_ship_gate(&key)?;
        let scope = self.ship_scope(&key).await?;
        let effect = blocking(move || {
            interface_review::thread_git::commit::selected(
                Path::new(&scope.dir),
                &revision,
                &message,
                &choices,
            )
        })
        .await?
        .map_err(git_failed)?;
        self.ship_answer(
            &key,
            ShipAction::Commit,
            effect,
            |sha| {
                format!(
                    "Committed the selected changes as {}.",
                    sha.get(..7).unwrap_or(&sha)
                )
            },
            false,
        )
        .await
    }

    /// Retained whole-file command for protocol compatibility.
    async fn commit(
        &self,
        key: String,
        message: String,
        mut paths: Vec<String>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let message = message.trim().to_owned();
        if message.is_empty() || message.len() > MAX_MESSAGE_BYTES {
            return Err(invalid("A commit needs a message (at most 64 KiB)."));
        }
        if self.busy(&key) {
            return Err(still_working("commit"));
        }
        let _send = self.hold_send_gate(&key)?;
        let _ship = self.hold_ship_gate(&key)?;
        let scope = self.ship_scope(&key).await?;
        paths.sort_unstable();
        paths.dedup();
        let dir = scope.dir.clone();
        let touched = scope.touched.clone();
        let effect = blocking(move || {
            let state = ship::ship_state(Path::new(&dir), false)?;
            if let Some(why) = commit_blocked(&state) {
                return Ok(Err(why));
            }
            let selection = commit_selection(&state.changes, touched.as_ref());
            if selection.paths() != paths {
                return Ok(Err(String::new()));
            }
            ship::commit_paths(Path::new(&dir), &paths, &selection.untracked(), &message)
                .map(|effect| Ok((effect, paths.len())))
        })
        .await?
        .map_err(git_failed)?;
        let (effect, count) = match effect {
            Ok(done) => done,
            Err(why) if why.is_empty() => {
                return Err(DispatchError::new(
                    THREADS_SHIP_STALE_CODE,
                    "The files to commit changed since you looked. Review them again, then commit.",
                    false,
                    true,
                ));
            }
            Err(why) => return Err(ship_refused(why)),
        };
        self.ship_answer(
            &key,
            ShipAction::Commit,
            effect,
            |sha| {
                let short = sha.get(..7).unwrap_or(&sha);
                format!(
                    "Committed {count} {} as {short}.",
                    if count == 1 { "file" } else { "files" }
                )
            },
            false,
        )
        .await
    }

    /// A Local thread's checkout switches to a new branch at HEAD.
    async fn create_branch(
        &self,
        key: String,
        branch: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let branch = branch.trim().to_owned();
        if branch.is_empty() {
            return Err(invalid("Name the branch."));
        }
        let scope = self.ship_scope(&key).await?;
        if scope.worktree {
            return Err(DispatchError::new(
                THREADS_SHIP_BLOCKED_CODE,
                "A worktree thread already works on a branch of its own.",
                false,
                true,
            ));
        }
        if self.busy(&key) {
            return Err(still_working("switch branches"));
        }
        let _send = self.hold_send_gate(&key)?;
        let _ship = self.hold_ship_gate(&key)?;
        let dir = scope.dir.clone();
        let effect = blocking(move || ship::create_branch(Path::new(&dir), &branch))
            .await?
            .map_err(git_failed)?;
        self.ship_answer(
            &key,
            ShipAction::CreateBranch,
            effect,
            |branch| format!("Switched this checkout to the new branch {branch}."),
            false,
        )
        .await
    }

    /// Pushes the branch, then opens its pull request unless one is open
    /// already (`gh pr list --head` first, every time).
    async fn open_pull_request(
        &self,
        key: String,
        title: String,
        body: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let title = title.trim().to_owned();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
            return Err(invalid(
                "A pull request needs a title (at most 256 characters).",
            ));
        }
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(invalid("A pull request's description is at most 64 KiB."));
        }
        let _ship = self.hold_ship_gate(&key)?;
        let scope = self.ship_scope(&key).await?;
        let gh = self.ready_gh(&scope.dir).await?;
        let dir = scope.dir.clone();
        let step = blocking(move || open_pull_request_now(&gh, Path::new(&dir), &title, &body))
            .await?
            .map_err(git_failed)?;
        let (effect, found) = match step {
            OpenStep::Blocked(why) => {
                return Err(DispatchError::new(
                    THREADS_SHIP_BLOCKED_CODE,
                    why,
                    false,
                    true,
                ));
            }
            OpenStep::Effect(effect, found) => (effect, found),
        };
        if let Some(found) = &found {
            // Opened here, so tracked here: even with background tracking off.
            let target = key.clone();
            let found = FoundPullRequest {
                url: found.url.clone(),
                number: found.number,
                state: found.state,
            };
            self.edit_quiet(move |document, now| {
                document.record_opened_pull_request(&target, found, now);
                Ok(())
            })
            .await?;
        }
        self.ship_answer(
            &key,
            ShipAction::OpenPullRequest,
            effect,
            |message| message,
            true,
        )
        .await
    }

    /// The prompt asking the thread to fix its pull request's failing
    /// checks: their names and links, and the tail of each failed job's log
    /// (`gh run view --log-failed`), bounded and credential-redacted.
    async fn failing_checks_prompt(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let scope = self.ship_scope(&key).await?;
        let gh = self.ready_gh(&scope.dir).await?;
        let dir = scope.dir.clone();
        let recorded = scope.pull_request.as_ref().map(|pr| pr.number);
        let prompt = blocking(move || -> Result<String, DispatchError> {
            let dir = Path::new(&dir);
            let state = ship::branch_state(dir).map_err(git_failed)?;
            let pull_request = thread_pull_request(&gh, dir, &state, recorded)
                .map_err(|message| gh_unavailable(&message))?
                .filter(|pr| pr.state == PullRequestState::Open)
                .ok_or_else(no_pull_request)?;
            let checks = gh
                .checks(dir, pull_request.number)
                .map_err(|error| gh_unavailable(&error.message))?;
            let summary = checks_summary(&checks);
            if summary.state != ChecksState::Failing {
                return Err(DispatchError::new(
                    THREADS_CHECKS_NOT_FAILING_CODE,
                    format!(
                        "No checks are failing on #{} right now.",
                        pull_request.number
                    ),
                    false,
                    true,
                ));
            }
            Ok(checks_prompt(&gh, dir, &pull_request, &summary))
        })
        .await??;
        let mut response = self.view_response().await?;
        response.prompt = Some(prompt);
        Ok(response)
    }

    /// The thread's pull request's unresolved review threads, for its draft.
    async fn review_comments(&self, key: String) -> Result<ThreadControlResponse, DispatchError> {
        let scope = self.ship_scope(&key).await?;
        let gh = self.ready_gh(&scope.dir).await?;
        let dir = scope.dir.clone();
        let recorded = scope.pull_request.as_ref().map(|pr| pr.number);
        let comments = blocking(move || -> Result<_, DispatchError> {
            let dir = Path::new(&dir);
            let state = ship::branch_state(dir).map_err(git_failed)?;
            let pull_request = thread_pull_request(&gh, dir, &state, recorded)
                .map_err(|message| gh_unavailable(&message))?
                .ok_or_else(no_pull_request)?;
            gh.review_comments(dir, &pull_request.url)
                .map_err(|error| gh_unavailable(&error.message))
        })
        .await??;
        let mut response = self.view_response().await?;
        response.review_comments = Some(comments);
        Ok(response)
    }

    /// Merges pull request `number`, which must still be the thread's, then
    /// settles the thread through settle-on-merge.
    async fn merge_pull_request(
        &self,
        key: String,
        number: u64,
        method: MergeMethod,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let _ship = self.hold_ship_gate(&key)?;
        let scope = self.ship_scope(&key).await?;
        let gh = self.ready_gh(&scope.dir).await?;
        let dir = scope.dir.clone();
        let recorded = scope.pull_request.as_ref().map(|pr| pr.number);
        let (effect, found) = blocking(move || -> Result<_, DispatchError> {
            let dir = Path::new(&dir);
            let state = ship::branch_state(dir).map_err(git_failed)?;
            let current = thread_pull_request(&gh, dir, &state, recorded)
                .map_err(|message| gh_unavailable(&message))?
                .filter(|pr| pr.number == number)
                .ok_or_else(|| {
                    DispatchError::new(
                        THREADS_NO_PULL_REQUEST_CODE,
                        format!("#{number} is no longer this thread's pull request. Check again."),
                        false,
                        true,
                    )
                })?;
            Ok(merge_now(&gh, dir, current, method))
        })
        .await??;
        if let Some(found) = &found {
            self.record_found(&key, found).await?;
            if found.state == PullRequestState::Merged {
                // The existing settle-on-merge path: a merged pull request
                // settles an active thread that has not run since.
                self.auto_settle().await;
            }
        }
        self.ship_answer(&key, ShipAction::Merge, effect, |message| message, true)
            .await
    }
}

/// `gh`'s side of a status read, never failing the read: a missing,
/// signed-out or silent `gh` says how to fix it.
fn read_remote(
    gh: Option<Gh>,
    dir: &Path,
    state: &ShipGitState,
    recorded: Option<u64>,
) -> RemoteRead {
    let unread = |gh, fix: &str| RemoteRead {
        gh,
        gh_fix: Some(fix.to_owned()),
        pull_request: None,
        checks: None,
    };
    let Some(gh) = gh else {
        return unread(GhState::Missing, GH_MISSING_FIX);
    };
    match gh.auth(dir) {
        GhAuth::Ready => {}
        GhAuth::SignedOut => return unread(GhState::SignedOut, GH_SIGNED_OUT_FIX),
        GhAuth::Unavailable => return unread(GhState::Unavailable, GH_SILENT_FIX),
    }
    let pull_request = match thread_pull_request(&gh, dir, state, recorded) {
        Ok(pull_request) => pull_request,
        Err(message) => return unread(GhState::Unavailable, &message),
    };
    let checks = pull_request
        .as_ref()
        .filter(|pr| pr.state == PullRequestState::Open)
        .and_then(|pr| gh.checks(dir, pr.number).ok())
        .map(|checks| checks_summary(&checks));
    RemoteRead {
        gh: GhState::Ready,
        gh_fix: None,
        pull_request,
        checks,
    }
}

/// The thread's pull request: the open one for its branch (not the default
/// branch), else the one it has recorded.
fn thread_pull_request(
    gh: &Gh,
    dir: &Path,
    state: &ShipGitState,
    recorded: Option<u64>,
) -> Result<Option<GhPullRequest>, String> {
    if let Some(branch) = state
        .branch
        .as_deref()
        .filter(|_| !on_default_branch(state))
        && let Some(open) = gh
            .open_pull_request_for(dir, branch)
            .map_err(|error| error.message)?
    {
        return Ok(Some(open));
    }
    recorded
        .map(|number| gh.pull_request(dir, number).map_err(|error| error.message))
        .transpose()
}

enum OpenStep {
    Blocked(String),
    /// What happened, and the pull request to record.
    Effect(ShipEffect<String>, Option<GhPullRequest>),
}

/// Push, look for an open pull request, then open one: each step re-read
/// when its outcome is unclear, never retried.
fn open_pull_request_now(
    gh: &Gh,
    dir: &Path,
    title: &str,
    body: &str,
) -> Result<OpenStep, ThreadGitError> {
    let state = ship::ship_state(dir, true)?;
    if let Some(why) = push_blocked(&state) {
        return Ok(OpenStep::Blocked(why));
    }
    let (Some(branch), Some(remote)) = (state.branch.clone(), state.remote.clone()) else {
        return Ok(OpenStep::Blocked(
            "This checkout has no branch or remote to push.".into(),
        ));
    };
    match ship::push(dir, &remote, &branch)? {
        ShipEffect::Done(()) => {}
        ShipEffect::Refused(why) => return Ok(OpenStep::Effect(ShipEffect::Refused(why), None)),
        ShipEffect::Unknown(why) => return Ok(OpenStep::Effect(ShipEffect::Unknown(why), None)),
    }
    match gh.open_pull_request_for(dir, &branch) {
        Ok(Some(open)) => {
            let message = format!("Pushed {branch}. #{} is already open for it.", open.number);
            return Ok(OpenStep::Effect(ShipEffect::Done(message), Some(open)));
        }
        Ok(None) => {}
        Err(error) => {
            return Ok(OpenStep::Effect(
                ShipEffect::Refused(format!(
                    "Pushed {branch}, but Interface couldn't check for an open pull request ({}), so it didn't open one. Check again.",
                    error.message
                )),
                None,
            ));
        }
    }
    let Some(base) = state.default_branch.clone() else {
        return Ok(OpenStep::Effect(
            ShipEffect::Refused(format!(
                "Pushed {branch}, but Interface couldn't tell the repository's default branch, so it didn't open a pull request."
            )),
            None,
        ));
    };
    match gh.create_pull_request(dir, &branch, &base, title, body) {
        Ok(number) => {
            let found = gh
                .pull_request(dir, number)
                .ok()
                .or_else(|| gh.open_pull_request_for(dir, &branch).ok().flatten());
            Ok(OpenStep::Effect(
                ShipEffect::Done(format!("Pushed {branch} and opened #{number}.")),
                found,
            ))
        }
        Err(error) => match gh.open_pull_request_for(dir, &branch) {
            Ok(Some(open)) => Ok(OpenStep::Effect(
                ShipEffect::Done(format!(
                    "Opened #{} (gh reported a problem, but the pull request is there).",
                    open.number
                )),
                Some(open),
            )),
            Ok(None) if !error.uncertain => {
                Ok(OpenStep::Effect(ShipEffect::Refused(error.message), None))
            }
            _ => Ok(OpenStep::Effect(
                ShipEffect::Unknown(format!(
                    "{} Interface couldn't confirm whether the pull request was opened. Check again before trying again.",
                    error.message
                )),
                None,
            )),
        },
    }
}

/// Merges `current`, judging an unclear end by reading the pull request again.
fn merge_now(
    gh: &Gh,
    dir: &Path,
    current: GhPullRequest,
    method: MergeMethod,
) -> (ShipEffect<String>, Option<GhPullRequest>) {
    let number = current.number;
    match current.state {
        PullRequestState::Merged => {
            return (
                ShipEffect::Done(format!("#{number} is already merged.")),
                Some(current),
            );
        }
        PullRequestState::Closed => {
            return (
                ShipEffect::Refused(format!("#{number} is closed, so it can't be merged.")),
                Some(current),
            );
        }
        PullRequestState::Open => {}
    }
    let merged = gh.merge(dir, number, method);
    match (merged, gh.pull_request(dir, number)) {
        (_, Ok(read)) if read.state == PullRequestState::Merged => {
            (ShipEffect::Done(format!("Merged #{number}.")), Some(read))
        }
        (Ok(()), Ok(read)) => (
            ShipEffect::Done(format!(
                "GitHub accepted the merge of #{number}, but it isn't merged yet: a merge queue or required checks may be holding it."
            )),
            Some(read),
        ),
        (Err(error), Ok(read)) if !error.uncertain => {
            (ShipEffect::Refused(error.message), Some(read))
        }
        (Err(error), _) => (
            ShipEffect::Unknown(format!(
                "{} Interface couldn't confirm whether #{number} merged. Check again before trying again.",
                error.message
            )),
            None,
        ),
        (Ok(()), Err(error)) => (
            ShipEffect::Unknown(format!(
                "gh merged #{number}, but Interface couldn't read it back ({}). Check again.",
                error.message
            )),
            None,
        ),
    }
}

/// The failing-checks prompt, at most [`PROMPT_BYTES`] (the pull-request
/// watch sends it too, T3 section 7).
pub(super) fn checks_prompt(
    gh: &Gh,
    dir: &Path,
    pull_request: &GhPullRequest,
    summary: &interface_contracts::threads::ShipChecks,
) -> String {
    let named = |check: &interface_contracts::threads::ShipCheck| match &check.workflow {
        Some(workflow) => format!("{workflow} / {}", check.name),
        None => check.name.clone(),
    };
    let mut prompt = format!(
        "The checks on pull request #{} ({}) are failing:\n",
        pull_request.number, pull_request.url
    );
    for check in &summary.failing {
        let _ = match &check.link {
            Some(link) => writeln!(prompt, "- {} — {link}", named(check)),
            None => writeln!(prompt, "- {}", named(check)),
        };
    }
    let mut seen = BTreeSet::new();
    for check in summary.failing.iter().take(PROMPT_LOGS) {
        let Some(run) = check.link.as_deref().and_then(check_run) else {
            continue;
        };
        if !seen.insert((run.run, run.job)) {
            continue;
        }
        let Ok(excerpt) = gh.failed_log(dir, &run) else {
            continue;
        };
        if excerpt.trim().is_empty() {
            continue;
        }
        let fence = "`".repeat(
            excerpt
                .split(|character| character != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1,
        );
        let block = format!(
            "\nThe end of `gh run view --log-failed` for {}:\n{fence}text\n{excerpt}\n{fence}\n",
            named(check)
        );
        if prompt.len() + block.len() > PROMPT_BYTES {
            break;
        }
        prompt.push_str(&block);
    }
    prompt.push_str(
        "\nFind out why these checks fail and fix it, then run what you can locally to confirm.",
    );
    prompt
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, DispatchError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| internal("The ship task stopped unexpectedly."))
}

fn git_failed(error: ThreadGitError) -> DispatchError {
    let ThreadGitError { message, .. } = error;
    DispatchError::new(
        THREADS_GIT_FAILED_CODE,
        format!(
            "git could not read this thread's checkout: {}",
            ship::redact_credentials(&message)
        ),
        false,
        true,
    )
}

fn ship_text_unavailable(message: &str) -> DispatchError {
    DispatchError::new(THREADS_SHIP_TEXT_UNAVAILABLE_CODE, message, false, true)
}

fn gh_unavailable(message: &str) -> DispatchError {
    DispatchError::new(THREADS_GH_UNAVAILABLE_CODE, message, false, true)
}

fn ship_refused(message: String) -> DispatchError {
    DispatchError::new(THREADS_SHIP_REFUSED_CODE, message, false, true)
}

fn no_pull_request() -> DispatchError {
    DispatchError::new(
        THREADS_NO_PULL_REQUEST_CODE,
        "This thread has no open pull request. Push & open one first.",
        false,
        true,
    )
}

fn still_working(what: &str) -> DispatchError {
    DispatchError::new(
        THREADS_BUSY_CODE,
        format!("The agent is still working in this checkout. Wait for it to finish, then {what}."),
        false,
        true,
    )
}

fn invalid(message: &str) -> DispatchError {
    DispatchError::new(THREADS_INVALID_CODE, message, false, true)
}
