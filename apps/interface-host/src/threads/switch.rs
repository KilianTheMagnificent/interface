//! Switch provider inside a thread (T3 section 5, protocol 59). A thread
//! whose provider is switched becomes a chain of provider sessions
//! (`interface_settings::threads` store v5): the head runs the thread, the
//! earlier segments keep their sessions and journals for a switch back.
//!
//! One command does it all in Rust: refuse what cannot be switched now;
//! open the target (back to that provider's earlier session through the
//! guarded runtime's resume, else a new session in the thread's folder on
//! the guarded runtime, at the old head's permission rung or the nearest
//! lower one, never wider); make it the head in one store commit; close the
//! old head's session (rechecked idle under its send gate); then send the
//! handoff as a host prompt (shown folded, journalled as the host's). A new
//! segment hears the whole thread from the journal; a provider switched
//! back to hears only what the others did since it last worked. Fail
//! closed: a target that cannot be opened changes nothing, and a handoff
//! whose send is unknown is never retried (the owner types).

use std::{path::Path, sync::Arc};

use interface_collaboration::thread_handoff::{SwitchInput, provider_word, switch_prompt};
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, EngineId, LaunchOptions, SessionKind},
    ipc::{CHANNELS, parse_key},
    thread_journal::TurnOrigin,
    threads::{DiffScope, HandoffDelivery, ThreadControlResponse, ThreadDiff, WorkspaceMode},
};
use interface_review::thread_git::DiffRequest;
use interface_settings::{
    thread_journal::{self, ChainSegment},
    thread_team,
    threads::{
        ChainPart, ThreadRecord, ThreadRefusal, is_handoff_preview, pending_baseline_ref, turn_ref,
    },
};
use serde_json::{Value, json};

use super::{
    THREADS_GIT_FAILED_CODE, ThreadHost,
    opinion::{THREADS_SAME_PROVIDER_CODE, THREADS_SWITCH_MISSING_CODE},
    refused,
    terminal_bridge::{THREADS_IN_TERMINAL_CODE, THREADS_SESSION_BUSY_CODE},
};
use crate::dispatcher::DispatchError;

/// The target provider is not installed, signed in or answering.
pub const THREADS_SWITCH_NOT_READY_CODE: &str = "threads-provider-not-ready";
/// The target provider has no permission mode at or below the thread's.
pub const THREADS_SWITCH_MODE_CODE: &str = "threads-switch-mode-unavailable";
/// The guarded runtime is not attached (a host without one).
pub const THREADS_SWITCH_UNAVAILABLE_CODE: &str = "threads-switch-unavailable";

/// How many journal lines per segment a handoff reads.
const HANDOFF_LINES: usize = 2_000;

fn refusal(code: &'static str, message: impl Into<String>) -> DispatchError {
    refused(ThreadRefusal::new(code, message))
}

/// What the switch read about the thread before acting.
struct Plan {
    head: ThreadRecord,
    parts: Vec<ChainPart>,
    dir: String,
    branch: Option<String>,
    /// An earlier segment on the target provider: switch back to it.
    back: Option<String>,
}

impl ThreadHost {
    /// `switchProvider` (see the module docs). Answers the new head's key.
    #[allow(clippy::too_many_lines)] // One ordered effect: open, commit, close, hand off.
    pub(super) async fn switch_provider(
        &self,
        key: String,
        engine: EngineId,
        model: Option<String>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (source, title) = self.thread_engine(&key)?;
        if source == engine {
            return Err(refusal(
                THREADS_SAME_PROVIDER_CODE,
                "The thread already runs on that provider.",
            ));
        }
        let plan = self.switch_plan(&key, engine.as_str()).await?;
        // A loop or a Code pane drives the session; a turn is running.
        self.away_unheld(&key)?;
        if let Some(claim) = self.inner.ports.claims.claim(&key) {
            return Err(refusal(THREADS_IN_TERMINAL_CODE, claim.message()));
        }
        let busy = || {
            refusal(
                THREADS_SESSION_BUSY_CODE,
                "A turn is running. Let it finish or stop it, then switch provider.",
            )
        };
        if self.busy(&key) {
            return Err(busy());
        }
        let runtime = self.guarded_runtime().ok_or_else(|| {
            refusal(
                THREADS_SWITCH_UNAVAILABLE_CODE,
                "Switching provider is not available in this Interface.",
            )
        })?;
        if plan.back.is_none() {
            self.require_ready(engine.as_str()).await?;
        }
        // No send starts on the old head until the switch is done.
        let gate = self.hold_send_gate(&key).map_err(|_| busy())?;
        if self.busy(&key) {
            return Err(busy());
        }
        let source_word = provider_word(source.as_str());
        // Turn 0's start for an ACP launch (protocol 69).
        let mut launch_start = None;
        let (target, launch_prompt, prompt) = if let Some(back) = plan.back.clone() {
            self.team_follow(&key, &back).await;
            if let Err(error) = runtime
                .dispatch(CHANNELS.resume_session, json!({"sessionKey": back}))
                .await
            {
                self.team_follow(&back, &key).await;
                return Err(error);
            }
            let prompt = self.switch_back_prompt(&plan, &back, &title).await;
            (back, None, prompt)
        } else {
            let rung = thread_team::caller_rung(source.as_str(), self.access_mode(&key).as_deref());
            let (_, mode) = thread_team::child_mode(engine.as_str(), rung).ok_or_else(|| {
                refusal(
                    THREADS_SWITCH_MODE_CODE,
                    format!(
                        "{} has no permission mode at or below this thread's ({}), so the thread stayed on {source_word}.",
                        provider_word(engine.as_str()),
                        rung.as_str()
                    ),
                )
            })?;
            let prompt = self
                .switch_new_prompt(&plan, source.as_str(), engine.as_str(), &title)
                .await;
            // An ACP agent refuses an empty launch: its handoff rides it.
            // Protocol 60: no account is named, so the new head runs on the
            // target provider's default account; a switch back (above)
            // resumes the earlier segment on the account it is bound to.
            let launch = engine.as_str().starts_with("acp:");
            // Protocol 69: the agent works on its launch handoff at once, so
            // that turn's checkpoint is taken now, before the session starts,
            // under a pending name until the session has its key.
            let pending = if launch && plan.head.git_root.is_some() {
                let token = format!(
                    "switch-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..16]
                );
                self.capture_start(plan.dir.clone(), pending_baseline_ref(&token))
                    .await
            } else {
                None
            };
            let options = LaunchOptions {
                cwd: plan.dir.clone(),
                prompt: if launch {
                    prompt.clone()
                } else {
                    String::new()
                },
                model,
                permission_mode: Some(mode.to_owned()),
                name: Some(title.clone()),
                kind: Some(SessionKind::Code),
                // A thread team tools started stays one hop down.
                collaboration_depth: self.is_team_child(&key).then_some(1),
                ..LaunchOptions::default()
            };
            let created = runtime
                .dispatch(
                    CHANNELS.create_session,
                    json!({"engine": engine, "options": options}),
                )
                .await
                .and_then(|created| {
                    created
                        .get("key")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            DispatchError::new(
                                THREADS_SWITCH_UNAVAILABLE_CODE,
                                "The provider started without naming its session; the thread stayed where it was. Check the inbox before switching again.",
                                false,
                                false,
                            )
                        })
                });
            let target = match created {
                Ok(target) => target,
                Err(error) => {
                    self.drop_launch_checkpoint(plan.head.git_root.clone(), pending)
                        .await;
                    return Err(error);
                }
            };
            self.team_follow(&key, &target).await;
            launch_start = self
                .file_launch_checkpoint(plan.head.git_root.clone(), pending, &target)
                .await;
            (target, launch.then(|| prompt.clone()), prompt)
        };
        // The new head, in one commit.
        let committed = {
            let (head, next, launch) = (key.clone(), target.clone(), launch_prompt.clone());
            let start = launch_start.clone();
            let host = self.clone();
            self.edit(move |document, now| {
                document.switch_provider(&head, &next, launch.as_deref(), now)?;
                if let Some(start) = start {
                    document.file_launch_start(&next, start);
                }
                if launch.is_some() && !host.run_tracked(&next) {
                    // The launch's run is open in the tap before any
                    // reconcile can see it in the record, as for a bind.
                    host.open_run(&next);
                }
                Ok(())
            })
            .await
        };
        let view = match committed {
            Ok(((), view)) => view,
            Err(error) => {
                self.team_follow(&target, &key).await;
                if plan.back.is_none()
                    && let Err(closed) = self.inner.ports.sessions.close(&target).await
                {
                    tracing::warn!(%closed, "a switch's new session stayed open after its record failed");
                }
                return Err(DispatchError::new(
                    error.code,
                    format!(
                        "{} The provider session {target} was opened, but the thread stayed on {source_word}.",
                        error.message
                    ),
                    false,
                    false,
                ));
            }
        };
        // Its scheduled prompts and watch follow the head (T3 section 7).
        self.schedules_follow(&key, &target).await;
        if let Some(prompt) = &launch_prompt {
            self.journal_prompt(&target, TurnOrigin::Launch, prompt);
        }
        // T3 section 9: the thread's stashed drafts follow its head.
        self.stash_follow(&key, &target).await;
        if plan.back.is_some() {
            let summary = self.inner.ports.sessions.session(&target).ok().flatten();
            if summary.is_some_and(|summary| summary.title != title)
                && let Err(error) = self.inner.ports.sessions.rename(&target, title).await
            {
                tracing::warn!(%error, "a switched-back thread kept its earlier session's title");
            }
        }
        self.close_edited(&key, gate).await;
        self.schedule_registry_push();
        let handoff = if launch_prompt.is_some() {
            HandoffDelivery::Sent
        } else {
            self.permit_host_prompt(&target, &prompt);
            match runtime
                .dispatch(CHANNELS.send, json!({"sessionKey": target, "text": prompt}))
                .await
            {
                Ok(_) => HandoffDelivery::Sent,
                Err(error) => {
                    tracing::warn!(code = %error.code, unsent = error.replay_safe, "a switch's handoff did not go out cleanly");
                    if error.replay_safe {
                        self.take_host_prompt(&target, &prompt);
                        HandoffDelivery::Unsent
                    } else {
                        HandoffDelivery::Unknown
                    }
                }
            }
        };
        let view = self.read(|_, _| ()).await.map_or(view, |((), view)| view);
        Ok(ThreadControlResponse {
            key: Some(target),
            handoff: Some(handoff),
            // An ACP session started with its handoff: no send echoed it, so
            // the pane shows it as a launch prompt.
            prompt: launch_prompt,
            ..view.into()
        })
    }

    /// The thread as the switch reads it, and its refusals that need only
    /// the record: a segment, a plain chat, a missing worktree.
    async fn switch_plan(&self, key: &str, engine: &str) -> Result<Plan, DispatchError> {
        let owned = key.to_owned();
        let engine = engine.to_owned();
        let sessions = Arc::clone(&self.inner.ports.sessions);
        let (head, parts, back) = self
            .peek(move |document, _| {
                let head = document
                    .record(&owned)
                    .cloned()
                    .ok_or_else(ThreadRefusal::unknown)?;
                let back = document.switch_target(&owned, &engine, |segment| {
                    sessions
                        .session(segment)
                        .ok()
                        .flatten()
                        .is_some_and(|summary| !summary.archived)
                });
                Ok::<_, ThreadRefusal>((head, document.chain_parts(&owned), back))
            })
            .await?
            .map_err(refused)?;
        if !head.listed() {
            return Err(refused(ThreadRefusal::invalid(
                "That is an earlier part of a thread. Switch from the thread itself.",
            )));
        }
        let workspace = head
            .workspace
            .clone()
            .filter(|workspace| workspace.mode != WorkspaceMode::None)
            .ok_or_else(|| {
                refused(ThreadRefusal::new(
                    interface_settings::threads::THREADS_SWITCH_PLAIN_CODE,
                    "Switching provider is for project threads. Start a project thread to work with another provider.",
                ))
            })?;
        let dir = match workspace.mode {
            WorkspaceMode::Worktree => workspace
                .worktree_path
                .clone()
                .filter(|path| Path::new(path).is_dir())
                .ok_or_else(|| {
                    refusal(
                        THREADS_SWITCH_MISSING_CODE,
                        "This thread's worktree is gone. Send the thread a message to bring it back, then switch provider.",
                    )
                })?,
            _ => workspace.project_root.clone(),
        };
        Ok(Plan {
            head,
            parts,
            dir,
            branch: workspace.branch,
            back,
        })
    }

    /// Files an ACP launch's pending checkpoint under `target`'s turn 0
    /// (protocol 69), as a bind files its prepared baseline; `None` when
    /// there was none.
    async fn file_launch_checkpoint(
        &self,
        git_root: Option<String>,
        pending: Option<String>,
        target: &str,
    ) -> Option<String> {
        let (from, root) = (pending?, git_root?);
        let to = turn_ref(target, 0, false);
        let (pending, filed) = (from.clone(), to.clone());
        match self
            .git(
                move |git| git.move_ref(Path::new(&root), &from, &filed),
                THREADS_GIT_FAILED_CODE,
                "The launch checkpoint could not be filed",
                true,
            )
            .await
        {
            Ok(()) => Some(to),
            // Records keep full ref names, so it still serves turn 0.
            Err(error) => {
                tracing::warn!(code = %error.code, "a switch's launch checkpoint kept its pending name");
                Some(pending)
            }
        }
    }

    /// Deletes an ACP launch's pending checkpoint when the launch failed.
    async fn drop_launch_checkpoint(&self, git_root: Option<String>, pending: Option<String>) {
        let (Some(root), Some(pending)) = (git_root, pending) else {
            return;
        };
        if let Err(error) = self
            .git(
                move |git| git.delete_refs(Path::new(&root), &pending),
                THREADS_GIT_FAILED_CODE,
                "The launch checkpoint could not be removed",
                true,
            )
            .await
        {
            tracing::warn!(code = %error.code, "a failed switch's launch checkpoint was kept");
        }
    }

    /// Refuses a provider the guarded runtime's probe does not find ready.
    async fn require_ready(&self, engine: &str) -> Result<(), DispatchError> {
        let probes = self.probes().await;
        let probe = probes.iter().find(|probe| probe.engine.as_str() == engine);
        let signed_out = probe
            .and_then(|probe| probe.info.as_ref()?.auth)
            .is_some_and(|auth| auth == interface_contracts::engine::EngineAuth::SignedOut);
        if probe.is_some_and(|probe| probe.ok && probe.info.is_some()) && !signed_out {
            return Ok(());
        }
        let word = provider_word(engine);
        Err(refusal(
            THREADS_SWITCH_NOT_READY_CODE,
            if signed_out {
                format!("{word} is signed out. Sign in, then switch.")
            } else {
                format!("{word} is not ready. Check it in Settings, then switch.")
            },
        ))
    }

    /// The journal's segments of `parts` as [`thread_journal::replay_chain`]
    /// reads them, with their labels.
    fn chain_events(&self, parts: &[&ChainPart], since: Option<u64>) -> Vec<EngineEvent> {
        let Some(root) = self.journal_root() else {
            return Vec::new();
        };
        let labels = parts
            .iter()
            .map(|part| {
                parse_key(&part.key)
                    .map(|(engine, _)| provider_word(engine.as_str()))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let segments = parts
            .iter()
            .zip(&labels)
            .map(|(part, label)| ChainSegment {
                key: &part.key,
                label,
            })
            .collect::<Vec<_>>();
        thread_journal::replay_chain(&root, &segments, HANDOFF_LINES, since)
            .into_iter()
            // An earlier handoff is a packet of its own: never nested.
            .filter(|event| {
                !matches!(&event.event, EngineEventKind::UserMessage { text, .. } if is_handoff_preview(text))
            })
            .collect()
    }

    /// A new segment's handoff: the whole thread from the journal and its
    /// whole diff.
    async fn switch_new_prompt(
        &self,
        plan: &Plan,
        source: &str,
        target: &str,
        title: &str,
    ) -> String {
        let diff = self
            .thread_diff(plan.head.key.clone(), DiffScope::Thread)
            .await
            .ok()
            .and_then(|response| response.diff);
        let host = self.clone();
        let parts = plan.parts.clone();
        let events = tokio::task::spawn_blocking(move || {
            host.chain_events(&parts.iter().collect::<Vec<_>>(), None)
        })
        .await
        .unwrap_or_default();
        switch_prompt(&SwitchInput {
            source_engine: source,
            target_engine: target,
            title,
            cwd: &plan.dir,
            branch: plan.branch.as_deref(),
            diff: diff.as_ref(),
            events: &events,
            meanwhile: &[],
        })
    }

    /// What `back` missed: the other segments that ran after it last did,
    /// their journal since then, and the changes since its last checkpoint.
    async fn switch_back_prompt(&self, plan: &Plan, back: &str, title: &str) -> String {
        let (order, since) = plan
            .parts
            .iter()
            .find(|part| part.key == back)
            .map_or((0, 0), |part| (part.order, part.last_at));
        // The segments activated after `back` last was ran after it; of
        // their journals, only what came after it last ran is news.
        let others = plan
            .parts
            .iter()
            .filter(|part| part.key != back && part.order > order)
            .cloned()
            .collect::<Vec<_>>();
        let mut meanwhile: Vec<String> = Vec::new();
        for part in &others {
            if let Ok((engine, _)) = parse_key(&part.key)
                && meanwhile.last().is_none_or(|last| last != engine.as_str())
            {
                meanwhile.push(engine.as_str().to_owned());
            }
        }
        if meanwhile.is_empty() {
            meanwhile.push(
                parse_key(&plan.head.key)
                    .map(|(engine, _)| engine.as_str().to_owned())
                    .unwrap_or_default(),
            );
        }
        let host = self.clone();
        let events = tokio::task::spawn_blocking(move || {
            host.chain_events(&others.iter().collect::<Vec<_>>(), Some(since))
        })
        .await
        .unwrap_or_default();
        let diff = self.diff_since(back, &plan.dir).await;
        let target = parse_key(back)
            .map(|(engine, _)| engine.as_str().to_owned())
            .unwrap_or_default();
        switch_prompt(&SwitchInput {
            source_engine: meanwhile.last().map_or("", String::as_str),
            target_engine: &target,
            title,
            cwd: &plan.dir,
            branch: plan.branch.as_deref(),
            diff: diff.as_ref(),
            events: &events,
            meanwhile: &meanwhile,
        })
    }

    /// The thread's changes since `segment`'s last checkpoint (its last
    /// turn's end, else its start) to the working tree; `None` when it has
    /// none or git cannot say.
    async fn diff_since(&self, segment: &str, dir: &str) -> Option<ThreadDiff> {
        let owned = segment.to_owned();
        let base = self
            .peek(move |document, _| {
                let turn = document
                    .record(&owned)?
                    .turns
                    .iter()
                    .rev()
                    .find(|turn| turn.end_ref.is_some() || turn.start_ref.is_some())?;
                turn.end_ref.clone().or_else(|| turn.start_ref.clone())
            })
            .await
            .ok()
            .flatten()?;
        let dir = dir.to_owned();
        self.git(
            move |git| {
                git.diff(
                    Path::new(&dir),
                    &DiffRequest {
                        scope: DiffScope::Thread,
                        base,
                        head: None,
                        paths: None,
                    },
                )
            },
            THREADS_GIT_FAILED_CODE,
            "The changes since this provider last worked could not be read",
            true,
        )
        .await
        .ok()
    }

    /// The chain a history read answers for: `key`'s segments when it heads
    /// a switched thread, else none.
    pub(super) async fn chain_history_parts(&self, key: &str) -> Option<Vec<ChainPart>> {
        let owned = key.to_owned();
        let parts = self
            .peek(move |document, _| {
                document
                    .record(&owned)
                    .filter(|record| record.listed() && record.chain.is_some())
                    .map(|_| document.chain_parts(&owned))
            })
            .await
            .ok()
            .flatten()?;
        (parts.len() > 1).then_some(parts)
    }

    /// The head of `key`'s thread (`key` itself when it is not a segment,
    /// or has no record).
    pub(crate) async fn head_key(&self, key: &str) -> String {
        let owned = key.to_owned();
        self.peek(move |document, _| document.head_of(&owned).map(str::to_owned))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| key.to_owned())
    }

    /// The earlier segments of `key`'s thread when `key` is its head.
    pub(crate) async fn segments_of(&self, key: &str) -> Vec<String> {
        let owned = key.to_owned();
        self.peek(move |document, _| {
            document
                .record(&owned)
                .filter(|record| record.listed())
                .map(|_| {
                    document
                        .chain_members(&owned)
                        .into_iter()
                        .filter(|member| !member.listed())
                        .map(|member| member.key.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
}
