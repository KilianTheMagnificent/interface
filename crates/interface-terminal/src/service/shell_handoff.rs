//! The service half of the agent-exit shell handoff (host protocol v50; the
//! rule is `crate::shell_handoff`): which exits are still owed a shell, and
//! the create that starts one in the exited agent's folder, under the root
//! it was authorized for.
use super::{
    ApprovedTerminalRoots, AuthorizedDirectory, CreateTerminalRequest, Path, ShellHandoff,
    TerminalError, TerminalGeneration, TerminalId, TerminalService, TerminalSummary, lock,
    root_error,
};
use crate::shell_handoff::{ended_on_its_own_terms, shell_handoff_request};

impl TerminalService {
    /// The shell owed to `id`'s exited `generation`, under the fresh
    /// `shell_id`, or `None` when it is owed none: the exit was not a clean
    /// one of an agent CLI, someone asked that generation to close, it was
    /// already handed one, or the terminal has moved on (restarted, removed).
    #[must_use]
    pub fn shell_handoff_due(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        shell_id: TerminalId,
    ) -> Option<CreateTerminalRequest> {
        let state = lock(&self.shared.state);
        let entry = state.entries.get(&id)?;
        (entry.summary.generation == generation
            && entry.shell_handoff == ShellHandoff::Owed
            && ended_on_its_own_terms(&entry.summary))
        .then(|| shell_handoff_request(&entry.summary, shell_id))
    }

    /// Starts the shell [`Self::shell_handoff_due`] described, in place of
    /// `replaces`. The folder and its root come from the exited entry, never
    /// from the request, and the root must still be approved. The new
    /// summary names `replaces`; the exited entry is marked handed off.
    ///
    /// # Errors
    ///
    /// [`TerminalError::ShellHandoffUnavailable`] when `replaces` is no longer
    /// owed a shell, and otherwise the errors of
    /// [`Self::create_with_provider_history_check`].
    pub fn create_shell_handoff<F>(
        &self,
        request: CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
        replaces: TerminalId,
        history_check: F,
    ) -> Result<TerminalSummary, TerminalError>
    where
        F: FnOnce(CreateTerminalRequest) -> Result<CreateTerminalRequest, TerminalError>,
    {
        self.create_inner(request, approved, None, Some(replaces), history_check)
    }

    /// Where the shell starts: the exited agent's own folder, inside the
    /// approved root that owned it.
    pub(super) fn shell_handoff_directory(
        &self,
        replaces: TerminalId,
        request: &CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
    ) -> Result<AuthorizedDirectory, TerminalError> {
        let (root, cwd) = {
            let state = lock(&self.shared.state);
            let entry = state
                .entries
                .get(&replaces)
                .ok_or(TerminalError::ShellHandoffUnavailable)?;
            if entry.shell_handoff != ShellHandoff::Owed
                || !ended_on_its_own_terms(&entry.summary)
                || entry.summary.cwd != request.cwd
                || request.handoff.is_some()
                || request.sign_in
            {
                return Err(TerminalError::ShellHandoffUnavailable);
            }
            (entry.root.clone(), entry.cwd.clone())
        };
        approved
            .authorize_retained(&cwd, Path::new(&root))
            .map_err(root_error)
    }
}
