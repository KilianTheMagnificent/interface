//! Terminal creation: exact replay, saved and fresh restart authority,
//! managed-workspace verification and the spawn.

use super::authority::{
    authority_unavailable, authority_unproven, request_matches_reopen_authority, saved_authority,
    summary_matches_reopen_authority,
};
use super::spawn::CREATE_EXPOSURE;
use super::{
    CHANNELS, ColdReopenWorkspaceLease, CreateTerminalRequest, DispatchError, INVALID_PAYLOAD_CODE,
    ManagedWorkspaceVerification, Path, PathBuf, ReopenAuthoritySource,
    ReopenAuthorityTransactionKind, TerminalError, TerminalHostDispatcher, TerminalId,
    TerminalLaunchProfileId, TerminalReopenAuthority, TerminalReopenAuthorityState,
    TerminalSummary, Value, VerifiedManagedWorkspace, decode_payload, encode_result,
};

impl TerminalHostDispatcher {
    /// Creates a terminal, or reopens a saved one under its host restart proof.
    pub(super) async fn create_terminal(&self, payload: Value) -> Result<Value, DispatchError> {
        self.ensure_transport()?;
        let request = decode_payload::<CreateTerminalRequest>(CHANNELS.create_terminal, payload)?;
        self.create_terminal_request(request, None).await
    }

    /// The create transaction. `shell_handoff` names the exited agent whose
    /// pane a host-started shell takes (protocol 50): its folder and root
    /// come from that agent's entry, never from the request.
    #[allow(clippy::too_many_lines)] // One ordered create transaction.
    pub(super) async fn create_terminal_request(
        &self,
        mut request: CreateTerminalRequest,
        shell_handoff: Option<TerminalId>,
    ) -> Result<Value, DispatchError> {
        let sender = self.ensure_transport()?;
        if request.validate_sign_in().is_err() {
            return Err(DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "A sign-in terminal takes a provider profile and nothing else.",
                false,
                true,
            ));
        }
        self.ensure_reopen_authority_idle(request.id).await?;
        if let Some(summary) = self.exact_create_replay(&request).await? {
            return encode_result(summary);
        }
        let authority = self.create_authority(&request).await?;
        let retained_authority_root = authority
            .as_ref()
            .map(|authority| PathBuf::from(&authority.root));
        let retained = self
            .retained_managed_root(&mut request, authority.clone())
            .await?;
        let mut roots = self.approved_roots(retained_authority_root.clone()).await?;
        // Cold reopen already knows this workspace, but a new terminal ID
        // has no per-terminal proof. Recover only an exact saved host root,
        // never a renderer-remembered cwd or a descendant of that root.
        let fresh_root_authority = if authority.is_none()
            && request.handoff.is_none()
            && shell_handoff.is_none()
            && roots.owning_root(Path::new(&request.cwd)).ok().as_deref()
                != Some(Path::new(&request.cwd))
        {
            let cwd = PathBuf::from(&request.cwd);
            self.desktop_blocking(move |desktop| desktop.saved_terminal_root_authority(&cwd))
                .await?
        } else {
            None
        };
        let fresh_managed = if let Some(proof) = fresh_root_authority.as_ref() {
            let verified = self
                .verify_reopen_authority(proof, ReopenAuthoritySource::SavedRoster)
                .await?;
            roots = self
                .approved_roots(Some(PathBuf::from(&proof.root)))
                .await?;
            verified
        } else {
            None
        };
        // A project moved behind a symlink leaves the renderer naming its
        // old path. A fresh launch there starts in the approved root the
        // link resolves to; a saved reopen keeps the recorded path its
        // restart proof was matched against.
        if authority.is_none()
            && request.handoff.is_none()
            && shell_handoff.is_none()
            && let Some(root) = roots.resolve_linked_root(Path::new(&request.cwd))
            && let Some(root) = root.to_str()
        {
            root.clone_into(&mut request.cwd);
        }
        let managed_spawn = if authority.is_none() && fresh_root_authority.is_none() {
            self.verify_managed_spawn(&request, shell_handoff).await?
        } else {
            None
        };
        let verified_managed = retained
            .as_ref()
            .or(fresh_managed.as_ref())
            .or(managed_spawn.as_ref())
            .map(|(managed, _lease)| managed.clone());
        // `close` and transport-failure teardown set their atomic fences
        // before waiting for this dispatch gate. Do not let a create that
        // was paused in workspace verification spawn after either fence.
        self.ensure_transport()?;
        self.begin_reopen_authority_commit(
            request.id,
            authority.clone(),
            ReopenAuthorityTransactionKind::Create,
        )
        .await?;
        if let Some(managed) = verified_managed.as_ref() {
            if let Err(error) = self.begin_managed_reopen_proof(request.id, managed) {
                self.finish_reopen_authority_commit(request.id).await;
                return Err(error);
            }
        }
        let terminal_id = request.id;
        let claude_projects_root = self.claude_projects_root.clone();
        let summary = match self
            .run_blocking(move |service| {
                let recover = |request| {
                    recover_legacy_claude_binding(request, claude_projects_root.as_deref())
                };
                match (shell_handoff, retained_authority_root.as_deref()) {
                    (Some(replaces), _) => {
                        service.create_shell_handoff(request, &roots, replaces, recover)
                    }
                    (None, Some(root)) => service
                        .create_with_provider_history_check_in_retained_root(
                            request, &roots, root, recover,
                        ),
                    (None, None) => {
                        service.create_with_provider_history_check(request, &roots, recover)
                    }
                }
            })
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                self.finish_reopen_authority_commit(terminal_id).await;
                if let Some(managed) = verified_managed.as_ref() {
                    self.finish_managed_reopen_proof(terminal_id, managed);
                }
                return Err(error);
            }
        };
        let post_spawn_verification = self
            .worktree_release
            .verify_managed_workspace(
                Path::new(&summary.root),
                verified_managed
                    .as_ref()
                    .map(|managed| managed.key.as_str()),
            )
            .await;
        let mut authority_stable = match (verified_managed.as_ref(), post_spawn_verification) {
            (Some(expected), ManagedWorkspaceVerification::Verified(observed)) => {
                expected == &observed
            }
            (None, ManagedWorkspaceVerification::NotManaged) => true,
            _ => false,
        };
        if let Some(expected) = authority.as_ref() {
            authority_stable &= matches!(
                self.reopen_authority_state(summary.id).await,
                TerminalReopenAuthorityState::Authority(current) if &current == expected
            );
        }
        if let Some(expected) = fresh_root_authority.as_ref() {
            authority_stable &= matches!(
                self.reopen_authority_state(expected.terminal.id).await,
                TerminalReopenAuthorityState::Authority(current) if &current == expected
            );
        }
        if !authority_stable {
            if let Some(managed) = verified_managed.as_ref() {
                self.finish_managed_reopen_proof(summary.id, managed);
            }
            self.cleanup_uncommitted_or_log(
                &summary,
                true,
                "terminal with changed workspace authority did not close cleanly",
            )
            .await;
            return Err(DispatchError::new(
                "terminal-authority-changed",
                "The terminal workspace changed while Interface was starting it. Try again.",
                true,
                false,
            ));
        }
        let summary = self
            .expose_spawned(
                summary,
                sender,
                verified_managed.as_ref(),
                true,
                &CREATE_EXPOSURE,
            )
            .await?;
        drop(retained);
        drop(managed_spawn);
        encode_result(summary)
    }

    /// A renderer retry for a terminal that this host already owns is pure
    /// observation, answered with the existing terminal. It is resolved
    /// before consulting sidecar or worktree state: those mutable verifiers
    /// must not turn an exact replay into a rollback that tears down the
    /// original process.
    async fn exact_create_replay(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<Option<TerminalSummary>, DispatchError> {
        let replay_request = request.clone();
        let exact_replay = self
            .run_blocking(move |service| service.exact_create_replay(&replay_request))
            .await;
        match exact_replay {
            Err(error) if error.code == "terminal-id-conflict" => {
                // A provider receipt may have advanced the private
                // authority beyond the renderer's saved launchSession
                // before the first create response arrived. In that one
                // host-proven case, compare every original create field
                // except the provider binding and return the existing PTY.
                let authoritative_replay = self
                    .live_reopen_authority(request.id)
                    .await
                    .ok()
                    .flatten()
                    .filter(|authority| {
                        request_matches_reopen_authority(request, &authority.terminal)
                            && self
                                .service
                                .snapshot()
                                .terminals
                                .into_iter()
                                .any(|terminal| {
                                    terminal.id == request.id
                                        && terminal.root == authority.root
                                        && summary_matches_reopen_authority(
                                            &terminal,
                                            &authority.terminal,
                                        )
                                        && terminal.launch_session
                                            == authority.terminal.launch_session
                                })
                    });
                if authoritative_replay.is_some() {
                    let replay_request = request.clone();
                    if let Some(summary) = self
                        .run_blocking(move |service| {
                            service.exact_create_replay_with_authoritative_provider_session(
                                &replay_request,
                            )
                        })
                        .await?
                    {
                        return Ok(Some(summary));
                    }
                }
                Err(error)
            }
            replay => replay,
        }
    }

    /// The saved restart proof a create reopens under, or `None` for a
    /// terminal the roster has never saved. A pending legacy proof is
    /// migrated first.
    async fn create_authority(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<Option<TerminalReopenAuthority>, DispatchError> {
        const UNAVAILABLE: &str =
            "Interface could not read that saved terminal's restart proof. Try again.";
        const UNPROVEN: &str = "Interface could not prove that saved terminal's restart authority.";
        match self.reopen_authority_state(request.id).await {
            TerminalReopenAuthorityState::Unsaved => Ok(None),
            TerminalReopenAuthorityState::Authority(authority) => Ok(Some(authority)),
            TerminalReopenAuthorityState::PendingMigration(terminal) => {
                self.migrate_terminal_reopen_authority(request, Some(terminal))
                    .await?;
                saved_authority(
                    self.reopen_authority_state(request.id).await,
                    UNAVAILABLE,
                    || authority_unproven(UNPROVEN),
                )
                .map(Some)
            }
            TerminalReopenAuthorityState::Unproven => Err(authority_unproven(UNPROVEN)),
            TerminalReopenAuthorityState::Unavailable => Err(authority_unavailable(UNAVAILABLE)),
        }
    }

    pub(super) async fn retained_managed_root(
        &self,
        request: &mut CreateTerminalRequest,
        authority: Option<TerminalReopenAuthority>,
    ) -> Result<Option<(VerifiedManagedWorkspace, ColdReopenWorkspaceLease)>, DispatchError> {
        let Some(authority) = authority else {
            return Ok(None);
        };
        if request.handoff.is_some()
            || !request_matches_reopen_authority(request, &authority.terminal)
        {
            return Err(DispatchError::new(
                "terminal-reopen-authority-mismatch",
                "That saved terminal no longer matches its host restart proof.",
                false,
                true,
            ));
        }

        // The renderer's saved copy may be one shutdown behind the receipt
        // finalizer. Only the host-private binding selects the durable Claude
        // transcript on a cold reopen.
        let retained = self
            .verify_reopen_authority(&authority, ReopenAuthoritySource::SavedRoster)
            .await?;
        // The frozen `gemini` wire id now launches Antigravity. Older builds
        // persisted Gemini CLI's synthetic conversation handle in both the
        // renderer roster and this host-private authority sidecar, but `agy`
        // cannot resume that synthetic handle. Treat that legacy binding as retired before
        // launch resolution; the successful create commit rewrites the
        // authority without it.
        if authority
            .terminal
            .launch
            .as_ref()
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Gemini)
            && authority
                .terminal
                .launch_session
                .as_ref()
                .and_then(|session| session.id.as_deref())
                .is_none_or(|id| {
                    interface_terminal::native_checkpoint::session_id(
                        TerminalLaunchProfileId::Gemini,
                        id,
                    )
                    .is_none()
                })
        {
            request.launch_session = None;
        } else {
            request
                .launch_session
                .clone_from(&authority.terminal.launch_session);
        }
        Ok(retained)
    }

    /// Verifies a newly launched terminal's managed-worktree identity before
    /// the PTY starts. Keeping this short lease alive until the terminal's
    /// normal lease is installed closes both the cleanup race and the failure
    /// mode where a transient verifier outage left only a keyless proof.
    pub(super) async fn verify_managed_spawn(
        &self,
        request: &CreateTerminalRequest,
        shell_handoff: Option<TerminalId>,
    ) -> Result<Option<(VerifiedManagedWorkspace, ColdReopenWorkspaceLease)>, DispatchError> {
        // A pass-off or a handoff shell lives in its source pane's root.
        let workspace = request
            .handoff
            .as_ref()
            .map(|handoff| handoff.source_terminal_id)
            .or(shell_handoff)
            .and_then(|source| {
                self.service
                    .snapshot()
                    .terminals
                    .into_iter()
                    .find(|terminal| terminal.id == source)
                    .map(|terminal| PathBuf::from(terminal.root))
            })
            .unwrap_or_else(|| PathBuf::from(&request.cwd));
        let first = match self
            .worktree_release
            .verify_managed_workspace(&workspace, None)
            .await
        {
            ManagedWorkspaceVerification::Verified(verified) => verified,
            ManagedWorkspaceVerification::NotManaged => return Ok(None),
            ManagedWorkspaceVerification::Unavailable => {
                return Err(authority_unavailable(
                    "Interface could not verify that managed workspace. Try again.",
                ));
            }
        };
        let lease = ColdReopenWorkspaceLease::acquire(&self.desktop, &first.root, request.id)?;
        let verified = match self
            .worktree_release
            .verify_managed_workspace(&first.root, Some(&first.key))
            .await
        {
            ManagedWorkspaceVerification::Verified(verified) if verified == first => verified,
            ManagedWorkspaceVerification::Verified(_)
            | ManagedWorkspaceVerification::NotManaged => {
                return Err(DispatchError::new(
                    "terminal-authority-changed",
                    "That managed workspace changed while Interface was verifying it. Try again.",
                    true,
                    true,
                ));
            }
            ManagedWorkspaceVerification::Unavailable => {
                return Err(authority_unavailable(
                    "Interface could not finish verifying that managed workspace. Try again.",
                ));
            }
        };
        Ok(Some((verified, lease)))
    }
}

/// Turn only a provider-proven absent legacy Claude UUID into the native
/// resume picker. Store failures retain no guessing authority and stop before
/// a PTY is spawned.
pub(super) fn recover_legacy_claude_binding(
    mut request: CreateTerminalRequest,
    claude_projects_root: Option<&Path>,
) -> Result<CreateTerminalRequest, TerminalError> {
    let Some(root) = claude_projects_root else {
        return Ok(request);
    };
    if request
        .launch
        .is_none_or(|launch| launch.profile_id != TerminalLaunchProfileId::Claude)
    {
        return Ok(request);
    }
    let Some(session) = request.launch_session.as_mut() else {
        return Ok(request);
    };
    let Some(id) = session.id.as_deref() else {
        return Ok(request);
    };
    if !session.resume {
        return Ok(request);
    }
    match engine_claude::session_exists(root, id) {
        Ok(true) => {}
        Ok(false) => session.id = None,
        Err(_) => {
            return Err(TerminalError::ProviderSessionLookupFailed(
                TerminalLaunchProfileId::Claude,
            ));
        }
    }
    Ok(request)
}
