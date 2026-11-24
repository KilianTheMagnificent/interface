//! Which live pane a connecting process belongs to, for the pane tools: the
//! kernel's parent chain, checked again after every slow lookup.
use super::{
    ApprovedTerminalRoots, Arc, BTreeMap, HashMap, Path, PathBuf, TerminalError,
    TerminalGeneration, TerminalGenerationRequest, TerminalId, TerminalLaunchProfileId,
    TerminalService, TerminalState, current_entry, is_provider_session_id, live_process_cwd, lock,
    manual_provider_profile, owned_ancestor, process_parents, root_error,
};

impl TerminalService {
    /// The running pane whose child process is `pid` or one of its ancestors.
    ///
    /// This is how the pane MCP server learns which pane a connection came
    /// from: the peer is `nc`, spawned by the CLI, spawned by the pane. The
    /// walk is bounded and reads one `ps` listing outside every service lock;
    /// `None` when the process listing is unreadable or no live pane owns the
    /// chain, and the caller refuses rather than guesses.
    #[must_use]
    pub fn terminal_for_process_ancestor(
        &self,
        pid: u32,
    ) -> Option<(TerminalId, TerminalGeneration)> {
        let owners = self.running_process_owners();
        if owners.is_empty() {
            return None;
        }
        owner_in(pid, &owners, &process_parents()?)
    }

    /// Each running pane's child pid, read under the state lock.
    fn running_process_owners(&self) -> BTreeMap<u32, (TerminalId, TerminalGeneration)> {
        let state = lock(&self.shared.state);
        state
            .entries
            .values()
            .filter(|entry| entry.summary.state == TerminalState::Running)
            .filter_map(|entry| {
                let pid = entry.resources.control.process_id()?;
                Some((pid, (entry.summary.id, entry.summary.generation)))
            })
            .collect()
    }

    /// The provider session each running pane is bound to, read from memory
    /// with no process lookup. [`Self::report_worker_conversation`] confirms
    /// only a running pane, and the session it confirms is this binding except
    /// while the binding catches up with an in-pane switch (a provider hook
    /// wake starts that refresh at once).
    #[must_use]
    pub fn bound_provider_sessions(&self) -> Vec<String> {
        let state = lock(&self.shared.state);
        state
            .entries
            .values()
            .filter(|entry| entry.summary.state == TerminalState::Running)
            .filter_map(|entry| entry.summary.launch_session.as_ref()?.id.clone())
            .collect()
    }

    /// Resolve a worker inbox from fresh provider evidence for this live caller.
    /// A requested resume id or renderer-supplied launch id never grants access.
    ///
    /// # Errors
    /// Refuses stale generations, unconfirmed sessions and unrelated processes.
    pub fn report_worker_conversation(
        &self,
        request: TerminalGenerationRequest,
        peer_pid: u32,
    ) -> Result<(TerminalLaunchProfileId, String), TerminalError> {
        let identity = Some((request.id, request.generation));
        let owners = self.running_process_owners();
        if owners.is_empty() {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        // One listing serves both ancestry checks: only an in-memory lock
        // separates them. The recheck after the provider lookup stays fresh.
        let parents = process_parents().ok_or(TerminalError::HandoffSourceUnavailable)?;
        if owner_in(peer_pid, &owners, &parents) != identity {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        let (profile, process, token, control) = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if entry.closing || entry.provider_session_replacing {
                return Err(TerminalError::HandoffSourceUnavailable);
            }
            (
                entry
                    .provider_session_profile
                    .ok_or(TerminalError::HandoffSourceUnavailable)?,
                entry
                    .provider_session_process
                    .clone()
                    .ok_or(TerminalError::HandoffSourceUnavailable)?,
                entry
                    .provider_session_capture_token
                    .clone()
                    .ok_or(TerminalError::HandoffSourceUnavailable)?,
                Arc::clone(&entry.resources.control),
            )
        };
        if owned_ancestor(peer_pid, &parents, |pid| pid == process.process_id).is_none()
            || control.process_identity_is_live(&process).ok() != Some(true)
        {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        let session = self
            .shared
            .provider_sessions
            .current_session(profile, &process, &token)
            .map_err(|e| TerminalError::Backend(e.to_string()))?
            .ok_or(TerminalError::HandoffSourceUnavailable)?;
        let effective_profile = manual_provider_profile(profile, &session.id);
        if !matches!(
            effective_profile,
            TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Codex
        ) || !is_provider_session_id(&session.id)
            || self.terminal_for_process_ancestor(peer_pid) != identity
        {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        let state = lock(&self.shared.state);
        let entry = current_entry(&state, request.id, request.generation)?;
        if entry.closing
            || entry.provider_session_replacing
            || entry.summary.state != TerminalState::Running
            || !entry.owns_capture(request.generation, &process, &token)
        {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        Ok((effective_profile, session.id))
    }

    /// Resolve the caller's live directory without accepting a model-supplied
    /// source path. Both process ancestry and the pane generation are checked
    /// again after the bounded operating-system lookup.
    ///
    /// # Errors
    /// Returns an error when the pane, process, or approved root changed.
    pub fn delegate_directory(
        &self,
        request: TerminalGenerationRequest,
        peer_pid: u32,
        approved: &ApprovedTerminalRoots,
        requested: Option<&Path>,
    ) -> Result<PathBuf, TerminalError> {
        let cwd = self.delegate_candidate(request, peer_pid, requested)?;
        let root = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if entry.closing || entry.summary.state != TerminalState::Running {
                return Err(TerminalError::HandoffSourceUnavailable);
            }
            entry.root.clone()
        };
        let directory = approved
            .authorize_retained(&cwd, Path::new(&root))
            .map_err(root_error)?;
        if self.terminal_for_process_ancestor(peer_pid) != Some((request.id, request.generation)) {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        let state = lock(&self.shared.state);
        let entry = current_entry(&state, request.id, request.generation)?;
        if entry.closing || entry.summary.state != TerminalState::Running || entry.root != root {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        Ok(directory.cwd.path)
    }

    /// Resolve a live caller's candidate, without granting directory access.
    /// The host must still authorize it before snapshotting any source files.
    ///
    /// # Errors
    /// Refuses a missing process or a caller outside the supplied generation.
    pub fn delegate_candidate(
        &self,
        request: TerminalGenerationRequest,
        peer_pid: u32,
        requested: Option<&Path>,
    ) -> Result<PathBuf, TerminalError> {
        let identity = Some((request.id, request.generation));
        if self.terminal_for_process_ancestor(peer_pid) != identity {
            return Err(TerminalError::HandoffSourceUnavailable);
        }
        let Some(path) = requested else {
            let cwd = live_process_cwd(peer_pid).ok_or(TerminalError::HandoffSourceUnavailable)?;
            // The cwd lookup is slow enough for the caller to change; a
            // requested path involves no such step, so it needs no recheck.
            if self.terminal_for_process_ancestor(peer_pid) != identity {
                return Err(TerminalError::HandoffSourceUnavailable);
            }
            return Ok(cwd);
        };
        Ok(path.to_path_buf())
    }
}

/// The running pane that owns `pid` or its nearest owned ancestor.
fn owner_in(
    pid: u32,
    owners: &BTreeMap<u32, (TerminalId, TerminalGeneration)>,
    parents: &HashMap<u32, u32>,
) -> Option<(TerminalId, TerminalGeneration)> {
    let owner = owned_ancestor(pid, parents, |candidate| owners.contains_key(&candidate))?;
    owners.get(&owner).copied()
}
