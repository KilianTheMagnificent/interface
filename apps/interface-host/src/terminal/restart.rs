//! Terminal restart under its saved or live restart proof.

use super::authority::{
    authority_unavailable, authority_unproven, saved_authority, summary_matches_reopen_authority,
};
use super::spawn::RESTART_EXPOSURE;
use super::{
    CHANNELS, DispatchError, PathBuf, ReopenAuthoritySource, ReopenAuthorityTransactionKind,
    TerminalError, TerminalGenerationRequest, TerminalHostDispatcher, TerminalId,
    TerminalReopenAuthority, TerminalReopenAuthorityState, TerminalSummary, Value, decode_payload,
    encode_result, not_replay_safe, terminal_dispatch_error,
};

impl TerminalHostDispatcher {
    /// Restarts a terminal under its saved or live restart proof.
    #[allow(clippy::too_many_lines)] // One ordered restart transaction.
    pub(super) async fn restart_terminal(&self, payload: Value) -> Result<Value, DispatchError> {
        let sender = self.ensure_transport()?;
        let request =
            decode_payload::<TerminalGenerationRequest>(CHANNELS.restart_terminal, payload)?;
        self.ensure_reopen_authority_idle(request.id).await?;
        let terminal = self
            .service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == request.id)
            .ok_or_else(|| terminal_dispatch_error(&TerminalError::NotFound(request.id)))?;
        let (mut authority, authority_source) = self.restart_authority(&terminal).await?;
        let retained = self
            .verify_reopen_authority(&authority, authority_source)
            .await?;
        let verified_managed = retained.as_ref().map(|(managed, _lease)| managed.clone());
        // A settled provider can leave its final receipt attached after
        // PTY exit. Retire it before the authority transaction begins:
        // persistence intentionally returns Retryable while a transaction
        // is open, which would otherwise make restart loop forever.
        self.run_blocking(move |service| service.settle_provider_capture_for_restart(request))
            .await?;
        let settled_terminal = self
            .service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == request.id && terminal.generation == request.generation)
            .ok_or_else(|| terminal_dispatch_error(&TerminalError::NotFound(request.id)))?;
        let refreshed_authority = self.settled_authority(request.id, authority_source).await?;
        if refreshed_authority.root != authority.root
            || refreshed_authority.managed_worktree_key != authority.managed_worktree_key
            || !summary_matches_reopen_authority(&settled_terminal, &refreshed_authority.terminal)
            || settled_terminal.launch_session != refreshed_authority.terminal.launch_session
        {
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "That terminal changed while Interface settled its provider session. Try again.",
                true,
                true,
            ));
        }
        authority = refreshed_authority;
        let retained_root = PathBuf::from(&authority.root);
        let roots = self.approved_roots(Some(retained_root)).await?;
        // A restart can spend time in provider settlement and worktree
        // verification. Recheck the shutdown/transport fence immediately
        // before it is allowed to enter the terminal service.
        self.ensure_transport()?;
        self.begin_reopen_authority_commit(
            request.id,
            Some(authority.clone()),
            ReopenAuthorityTransactionKind::Restart {
                prior_generation: request.generation,
            },
        )
        .await?;
        if let Some(managed) = verified_managed.as_ref() {
            if let Err(error) = self.begin_managed_reopen_proof(request.id, managed) {
                self.finish_reopen_authority_commit(request.id).await;
                return Err(error);
            }
        }
        let summary = match self
            .run_blocking(move |service| service.restart(request, &roots))
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                self.finish_reopen_authority_commit(request.id).await;
                if let Some(managed) = verified_managed.as_ref() {
                    self.finish_managed_reopen_proof(request.id, managed);
                }
                return Err(error);
            }
        };
        let post_retained = match self
            .verify_reopen_authority(&authority, authority_source)
            .await
        {
            Ok(retained) => retained,
            Err(error) => {
                if let Some(managed) = verified_managed.as_ref() {
                    self.finish_managed_reopen_proof(summary.id, managed);
                }
                self.cleanup_uncommitted_or_log(
                    &summary,
                    false,
                    "restarted terminal with changed authority did not close cleanly",
                )
                .await;
                return Err(not_replay_safe(error));
            }
        };
        let authority_stable = match (verified_managed.as_ref(), post_retained.as_ref()) {
            (Some(expected), Some((observed, _lease))) => expected == observed,
            (None, None) => true,
            _ => false,
        };
        if !authority_stable {
            if let Some(managed) = verified_managed.as_ref() {
                self.finish_managed_reopen_proof(summary.id, managed);
            }
            self.cleanup_uncommitted_or_log(
                &summary,
                false,
                "restarted terminal with stale authority did not close cleanly",
            )
            .await;
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "That terminal's workspace changed while Interface was restarting it.",
                true,
                false,
            ));
        }
        let summary = self
            .expose_spawned(
                summary,
                sender,
                verified_managed.as_ref(),
                false,
                &RESTART_EXPOSURE,
            )
            .await?;
        drop(retained);
        drop(post_retained);
        encode_result(summary)
    }

    /// The restart proof a terminal restarts under: its saved-roster proof
    /// when that still describes the running terminal, or the live proof of
    /// a terminal the roster never saved.
    async fn restart_authority(
        &self,
        terminal: &TerminalSummary,
    ) -> Result<(TerminalReopenAuthority, ReopenAuthoritySource), DispatchError> {
        match self.reopen_authority_state(terminal.id).await {
            TerminalReopenAuthorityState::Authority(authority)
                if terminal.root == authority.root
                    && summary_matches_reopen_authority(terminal, &authority.terminal) =>
            {
                Ok((authority, ReopenAuthoritySource::SavedRoster))
            }
            TerminalReopenAuthorityState::Unavailable => Err(authority_unavailable(
                "Interface could not read that terminal's restart proof. Try again.",
            )),
            TerminalReopenAuthorityState::Unsaved => Ok((
                self.live_reopen_authority(terminal.id)
                    .await?
                    .filter(|authority| {
                        terminal.root == authority.root
                            && summary_matches_reopen_authority(terminal, &authority.terminal)
                    })
                    .ok_or_else(|| {
                        authority_unproven(
                            "Interface could not prove that unsaved terminal's live restart authority.",
                        )
                    })?,
                ReopenAuthoritySource::LiveUnsaved,
            )),
            TerminalReopenAuthorityState::Authority(_)
            | TerminalReopenAuthorityState::PendingMigration(_)
            | TerminalReopenAuthorityState::Unproven => Err(authority_unproven(
                "Interface could not prove that terminal's restart authority.",
            )),
        }
    }

    /// The same source's proof again, once provider settlement has run.
    async fn settled_authority(
        &self,
        id: TerminalId,
        source: ReopenAuthoritySource,
    ) -> Result<TerminalReopenAuthority, DispatchError> {
        match source {
            ReopenAuthoritySource::SavedRoster => saved_authority(
                self.reopen_authority_state(id).await,
                "Interface could not read the settled terminal's restart proof. Try again.",
                || {
                    DispatchError::new(
                        "terminal-authority-changed",
                        "That terminal's restart proof changed while Interface settled its provider session. Try again.",
                        true,
                        true,
                    )
                },
            ),
            ReopenAuthoritySource::LiveUnsaved => self.live_reopen_authority(id).await?.ok_or_else(|| {
                DispatchError::new(
                    "terminal-authority-changed",
                    "That terminal lost its restart proof while Interface settled its provider session.",
                    true,
                    true,
                )
            }),
        }
    }
}
