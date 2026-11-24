//! Second opinion and Continue with (review B4, protocol 46). Continue with
//! stays on the wire and served; since protocol 59 the renderer offers
//! Switch to <provider> instead (`switch.rs`), which keeps the same thread.
//!
//! Second opinion submits an ordinary human consult through the durable
//! collaboration service: the coordinator shows the owner the exact packet
//! before anything leaves, runs the other provider read-only, and holds its
//! answer until the owner sends it on or discards it. The objective names the
//! latest turn's changes (`interface_collaboration::thread_handoff`). A
//! session a Code pane holds is refused here and again inside the
//! collaboration port, so a consult never writes to a session another
//! writer owns.
//!
//! Continue with does not use the coordinator's handoff mode: that parks the
//! source and drives its target outside the runtime dispatcher, where no
//! thread run tap, permission mode or session claim reaches it. It prepares
//! the new thread's workspace instead (the same folder, or the same worktree)
//! with a `handoff` lineage, and answers the first prompt; the renderer
//! launches the new session like any thread and binds it with the token.

use std::path::Path;

use interface_collaboration::thread_handoff::{
    HandoffInput, handoff_prompt, second_opinion_objective,
};
use interface_contracts::{
    engine::{EngineId, SessionKind},
    ipc::{CollaborationEngine, CollaborationMode, CollaborationSubmitRequest, parse_key},
    threads::{DiffScope, LineageRelation, ThreadControlResponse, ThreadLineage, WorkspaceMode},
};
use interface_settings::threads::ThreadRefusal;

use super::{
    THREADS_UNKNOWN_SESSION_CODE, ThreadHost, refused,
    terminal_bridge::{THREADS_IN_TERMINAL_CODE, THREADS_SESSION_BUSY_CODE},
};
use crate::dispatcher::DispatchError;

/// Second opinion or Continue with named the thread's own provider.
pub const THREADS_SAME_PROVIDER_CODE: &str = "threads-same-provider";
/// Continue with needs a project thread: a plain chat has no folder to carry.
pub const THREADS_CONTINUE_PLAIN_CODE: &str = "threads-continue-plain";
/// A worktree thread's worktree is gone, so there is nothing to continue in.
pub const THREADS_CONTINUE_MISSING_CODE: &str = "threads-continue-worktree-missing";
/// Switch provider (protocol 59): the worktree thread's worktree is gone.
pub const THREADS_SWITCH_MISSING_CODE: &str = "threads-switch-worktree-missing";

fn refusal(code: &'static str, message: &str) -> DispatchError {
    refused(ThreadRefusal::new(code, message))
}

const fn consult_engine_wire(engine: CollaborationEngine) -> &'static str {
    match engine {
        CollaborationEngine::Claude => "claude",
        CollaborationEngine::Codex => "codex",
        CollaborationEngine::AcpGrok => "acp:grok",
    }
}

impl ThreadHost {
    /// The thread's provider, from its key; refused when the key is not a
    /// session key or the registry has no such session (or an agent's).
    pub(super) fn thread_engine(&self, key: &str) -> Result<(EngineId, String), DispatchError> {
        let (engine, _) = parse_key(key).map_err(|_| {
            refusal(
                THREADS_UNKNOWN_SESSION_CODE,
                "That is not a thread Interface knows.",
            )
        })?;
        let session = self
            .inner
            .ports
            .sessions
            .session(key)
            .map_err(|error| super::registry_unavailable(&error))?
            .filter(|session| session.kind != Some(SessionKind::Agent))
            .ok_or_else(|| {
                refusal(
                    THREADS_UNKNOWN_SESSION_CODE,
                    "That thread is no longer in Interface. Refresh the list and try again.",
                )
            })?;
        Ok((engine, session.title))
    }

    /// Second opinion: a consult with `engine` on the thread's latest answer
    /// and its turn's changes, which waits for the owner's approval.
    pub(super) async fn second_opinion(
        &self,
        key: String,
        engine: CollaborationEngine,
        request_id: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (source, _) = self.thread_engine(&key)?;
        self.require_record(&key).await?;
        if source.as_str() == consult_engine_wire(engine) {
            return Err(refusal(
                THREADS_SAME_PROVIDER_CODE,
                "Ask a different provider for a second opinion.",
            ));
        }
        if let Some(claim) = self.inner.ports.claims.claim(&key) {
            return Err(refusal(THREADS_IN_TERMINAL_CODE, claim.message()));
        }
        if self.busy(&key) {
            return Err(refusal(
                THREADS_SESSION_BUSY_CODE,
                "A turn is running. Let it finish or stop it, then ask for a second opinion.",
            ));
        }
        // A consult reads a live source: open it as a send would.
        let live = self
            .inner
            .ports
            .sessions
            .live_keys()
            .is_ok_and(|keys| keys.contains(&key));
        if !live {
            self.inner.ports.consults.resume(&key).await?;
        }
        // A thread with no checkpoints (a plain chat, a folder outside git)
        // has no turn diff; the objective says so.
        let diff = self
            .thread_diff(key.clone(), DiffScope::Turn)
            .await
            .ok()
            .and_then(|response| response.diff);
        let objective = second_opinion_objective(source.as_str(), diff.as_ref());
        let status = self
            .inner
            .ports
            .consults
            .submit(CollaborationSubmitRequest {
                source_session_key: key,
                mode: CollaborationMode::Consult,
                target_engine: engine,
                objective,
                client_request_id: request_id,
            })
            .await?;
        let mut response = self.view_response().await?;
        response.consult = Some(status);
        Ok(response)
    }

    /// Protocol 60: whether `account` is another account than the one the
    /// thread `key` runs on (Default when unbound), which makes the thread's
    /// own provider a different place to continue.
    fn other_account(&self, key: &str, account: Option<&str>) -> Result<bool, DispatchError> {
        let Some(account) = account else {
            return Ok(false);
        };
        if !interface_contracts::provider_accounts::valid_account_id(account) {
            return Err(refusal(
                THREADS_SAME_PROVIDER_CODE,
                "That provider account is not valid.",
            ));
        }
        let current = self
            .accounts()
            .and_then(|accounts| accounts.account_of_key(key))
            .unwrap_or_else(|| {
                interface_contracts::provider_accounts::DEFAULT_ACCOUNT_ID.to_owned()
            });
        Ok(account != current)
    }

    /// Continue with: the new thread's workspace (this thread's folder or
    /// worktree, with a `handoff` lineage) and its first prompt.
    pub(super) async fn continue_with(
        &self,
        key: String,
        engine: EngineId,
        account: Option<String>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (source, title) = self.thread_engine(&key)?;
        if source == engine && !self.other_account(&key, account.as_deref())? {
            return Err(refusal(
                THREADS_SAME_PROVIDER_CODE,
                "Continue with a different provider.",
            ));
        }
        let lookup = key.clone();
        let workspace = self
            .peek(move |document, _| {
                document
                    .record(&lookup)
                    .map(|record| record.workspace.clone())
            })
            .await?
            .ok_or_else(|| refused(ThreadRefusal::unknown()))?
            .filter(|workspace| workspace.mode != WorkspaceMode::None)
            .ok_or_else(|| {
                refusal(
                    THREADS_CONTINUE_PLAIN_CODE,
                    "Continue with is for project threads. Start a project thread to work with another provider.",
                )
            })?;
        if self.busy(&key) {
            return Err(refusal(
                THREADS_SESSION_BUSY_CODE,
                "A turn is running. Let it finish or stop it, then continue with another provider.",
            ));
        }
        let (mode, worktree_path, cwd) = match workspace.mode {
            WorkspaceMode::Worktree => {
                let path = workspace
                    .worktree_path
                    .clone()
                    .filter(|path| Path::new(path).is_dir())
                    .ok_or_else(|| {
                        refusal(
                            THREADS_CONTINUE_MISSING_CODE,
                            "This thread's worktree is gone. Send the thread a message to bring it back, then continue with another provider.",
                        )
                    })?;
                ("existing", Some(path.clone()), path)
            }
            _ => ("local", None, workspace.project_root.clone()),
        };
        let diff = self
            .thread_diff(key.clone(), DiffScope::Thread)
            .await
            .ok()
            .and_then(|response| response.diff);
        let events = self
            .inner
            .ports
            .sessions
            .history(&key)
            .await
            .map(|page| page.events)
            .unwrap_or_default();
        let prompt = handoff_prompt(&HandoffInput {
            source_engine: source.as_str(),
            target_engine: engine.as_str(),
            title: &title,
            cwd: &cwd,
            branch: workspace.branch.as_deref(),
            diff: diff.as_ref(),
            events: &events,
        });
        let lineage = ThreadLineage {
            parent: key,
            relation: LineageRelation::Handoff,
            kept_turns: 0,
            at: 0,
        };
        let mut response = self
            .prepare_workspace(
                workspace.project_root,
                mode,
                None,
                worktree_path,
                title,
                Some(lineage),
            )
            .await?;
        response.prompt = Some(prompt);
        Ok(response)
    }
}
