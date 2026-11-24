//! An Away loop's thread (review B3, protocol 45). The loop's own worktree is
//! prepared like a new-worktree thread's, its empty work session is bound to
//! it with no turn, and the loop holds the thread while it runs: only the
//! prompt the loop permitted reaches the session, and the turn the send hook
//! records for it is handed back. The view joins each loop's projection from
//! the Away store, which pushes a new revision whenever it changes.

use std::{collections::HashMap, path::Path};

use interface_common::sync::lock;
use interface_contracts::{
    away::ThreadAway,
    threads::{PreparedWorkspace, ThreadsView, TitleSource},
};
use interface_settings::threads::prompt_digest;

use super::{THREADS_GIT_FAILED_CODE, ThreadHost};
use crate::dispatcher::DispatchError;

/// While a loop holds its thread, the owner's sends and the commands that
/// would act on its session or files refuse with this. Definitely unsent.
pub const THREADS_AWAY_RUNNING_CODE: &str = "threads-away-running";

/// A thread an Away loop holds.
#[derive(Debug, Default)]
pub(crate) struct AwayHold {
    /// The digest of the one prompt the loop may send next.
    permit: Option<String>,
    /// The turn the permitted send began, until the loop takes it.
    turn: Option<u32>,
}

impl ThreadHost {
    /// Prepares the loop's own git worktree of `project_root`, as New
    /// worktree does for a thread (`hint` names its folder and branch).
    ///
    /// # Errors
    /// The folder is not one the owner may open, is not a git repository, or
    /// the worktree could not be made (nothing is left behind).
    pub async fn away_prepare(
        &self,
        project_root: &str,
        hint: &str,
    ) -> Result<PreparedWorkspace, DispatchError> {
        // Away admitted `project_root` against the host's own project roots
        // already, so the thread's folder grant is not asked again.
        let response = self
            .prepare_workspace_vetted(
                project_root.to_owned(),
                "worktree",
                None,
                None,
                hint.to_owned(),
                None,
                true,
            )
            .await?;
        response
            .prepared
            .ok_or_else(|| super::internal("The prepared worktree went missing."))
    }

    /// Gives back a prepared worktree no thread took. One a session works in
    /// stays, for the hourly sweep.
    pub async fn away_discard(&self, token: &str) {
        if let Err(error) = self.discard_unused_prepared(token.to_owned()).await {
            tracing::warn!(code = %error.code, "an Away worktree could not be given back");
        }
    }

    /// Files the loop's empty work session `key` as its thread in the
    /// prepared worktree, with no turn (its first send records turn 0), and
    /// holds it for the loop. The unused launch baseline is deleted.
    ///
    /// # Errors
    /// The token was discarded or the store refused; nothing is held.
    pub async fn away_bind(&self, key: &str, token: &str) -> Result<(), DispatchError> {
        lock(&self.inner.away_holds).insert(key.to_owned(), AwayHold::default());
        let bound = self.bind_empty_thread(key, token, TitleSource::User).await;
        if bound.is_err() {
            lock(&self.inner.away_holds).remove(key);
        }
        bound
    }

    /// Files the empty session `key` as a thread in the prepared workspace
    /// `token`, with its prepared lineage and no turn (its first send records
    /// turn 0); the unused launch baseline is deleted. An Away loop's thread
    /// and a thread team tools started are bound this way.
    ///
    /// # Errors
    /// The token was discarded or the store refused.
    pub(super) async fn bind_empty_thread(
        &self,
        key: &str,
        token: &str,
        title_source: TitleSource,
    ) -> Result<(), DispatchError> {
        let (target, token) = (key.to_owned(), token.to_owned());
        let baseline = self
            .edit_quiet(move |document, now| {
                document.bind_empty(&target, &token, title_source, now)
            })
            .await?;
        if let Some((root, baseline)) = baseline
            && let Err(error) = self
                .git(
                    move |git| git.delete_refs(Path::new(&root), &baseline),
                    THREADS_GIT_FAILED_CODE,
                    "The unused launch checkpoint could not be deleted",
                    true,
                )
                .await
        {
            tracing::info!(code = %error.code, "an Away thread kept its unused launch checkpoint");
        }
        Ok(())
    }

    /// Lets the loop's next prompt through to its held thread.
    pub fn away_permit(&self, key: &str, prompt: &str) {
        if let Some(hold) = lock(&self.inner.away_holds).get_mut(key) {
            hold.permit = Some(prompt_digest(prompt));
            hold.turn = None;
        }
    }

    /// The thread turn the loop's last permitted send began, once.
    #[must_use]
    pub fn away_sent_turn(&self, key: &str) -> Option<u32> {
        lock(&self.inner.away_holds).get_mut(key)?.turn.take()
    }

    /// The loop ended: the thread takes the owner's sends again.
    pub fn away_release(&self, key: &str) {
        lock(&self.inner.away_holds).remove(key);
    }

    /// Replaces the loops' projections and pushes a new revision when one
    /// changed, so the open thread's cards follow the loop.
    pub async fn set_away_threads(&self, threads: Vec<(String, ThreadAway)>) {
        let next = threads.into_iter().collect::<HashMap<_, _>>();
        let changed = {
            let mut current = lock(&self.inner.away_threads);
            let changed = *current != next;
            *current = next;
            changed
        };
        if changed
            && let Err(error) = self
                .with_store_quiet(interface_settings::ThreadStore::bump)
                .await
        {
            tracing::warn!(code = %error.code, "an Away thread's change could not be pushed");
        }
    }

    /// For a send to `key`: whether the loop holding it permitted this one
    /// (`Ok(true)`, the permit spent), or that no loop holds it
    /// (`Ok(false)`). A held thread refuses every other send.
    pub(super) fn away_send(&self, key: &str, text: &str) -> Result<bool, DispatchError> {
        let mut holds = lock(&self.inner.away_holds);
        let Some(hold) = holds.get_mut(key) else {
            return Ok(false);
        };
        if hold
            .permit
            .as_ref()
            .is_some_and(|permit| *permit == prompt_digest(text))
        {
            hold.permit = None;
            return Ok(true);
        }
        Err(away_running())
    }

    /// The turn a permitted send began, for the loop to take.
    pub(super) fn away_turn_begun(&self, key: &str, ordinal: u32) {
        if let Some(hold) = lock(&self.inner.away_holds).get_mut(key) {
            hold.turn = Some(ordinal);
        }
    }

    /// Refuses a command that would act on a held thread's session or files.
    pub(super) fn away_unheld(&self, key: &str) -> Result<(), DispatchError> {
        if lock(&self.inner.away_holds).contains_key(key) {
            return Err(away_running());
        }
        Ok(())
    }

    /// Joins each loop's projection to its thread.
    pub(super) fn with_away(&self, view: &mut ThreadsView) {
        let away = lock(&self.inner.away_threads);
        if away.is_empty() {
            return;
        }
        for thread in &mut view.threads {
            thread.away = away.get(&thread.key).cloned();
        }
    }
}

fn away_running() -> DispatchError {
    DispatchError::new(
        THREADS_AWAY_RUNNING_CODE,
        "Away is working in this thread. Stop the loop from its card to take it over; nothing was sent.",
        false,
        true,
    )
}
