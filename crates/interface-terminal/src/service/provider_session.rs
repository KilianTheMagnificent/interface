//! What a provider adapter hands the terminal service: session identities, the
//! durable capture owner, the lookup that proves them, and the lifecycle sink
//! that files a reopen binding.
use super::{
    Path, PathBuf, TerminalBackendError, TerminalGeneration, TerminalId, TerminalLaunchProfileId,
    TerminalLaunchSessionV1, TerminalLifecycleEvent, TerminalProcessIdentity, TerminalSummary,
};

/// One provider-owned durable session transition, corroborated outside the
/// terminal byte stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalProviderSessionTransition {
    Startup,
    Resume,
    Clear,
    Compact,
    Fork,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalProviderSession {
    pub id: String,
    pub transition: TerminalProviderSessionTransition,
}

/// Exact durable owner state for a manually invoked foreground provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalProviderSessionCaptureProcess {
    pub process: TerminalProcessIdentity,
    pub cwd: PathBuf,
}

/// Durable ownership inputs for one provider-session capture generation.
///
/// The provider adapter commits this record before the PTY can execute. A
/// startup reconciler can therefore map a crash-surviving provider receipt
/// back to exactly one terminal generation without trusting terminal bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalProviderSessionCaptureOwner {
    pub terminal_id: TerminalId,
    pub terminal_generation: TerminalGeneration,
    pub capture_token: String,
    pub root: String,
    pub cwd: String,
    pub prior_launch_session: Option<TerminalLaunchSessionV1>,
    /// True when the provider is a foreground child of a retained login shell
    /// rather than the terminal's direct process.
    pub nested_shell: bool,
}

/// Host-owned lookup for a provider's live process-to-session pointer.
///
/// Terminal bytes are attacker-controlled from the service's point of view,
/// so OSC frames may only wake this lookup; they never supply the durable
/// provider identifier themselves.
pub trait TerminalProviderSessionLookup: Send + Sync {
    /// Durably register one capture owner before its provider can execute.
    ///
    /// The default is intentionally inert for providers without private
    /// receipt capture.
    ///
    /// # Errors
    ///
    /// Returns an error when ownership cannot be committed safely. The
    /// terminal must not spawn in that case.
    fn begin_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    /// Attach the exact direct-child process identity immediately after spawn
    /// and before the terminal generation becomes observable.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable owner cannot be advanced exactly.
    fn record_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    /// Read the exact process identity durably attached to a retained-shell
    /// capture. This lets teardown settle a wrapper-owned receipt even when
    /// the corresponding OSC wake was lost before the in-memory attachment.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable capture owner is malformed or cannot
    /// be read safely.
    fn capture_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        Ok(None)
    }

    /// Prevent any later retained-shell wrapper from claiming this capture or
    /// replacing its owner, and return the durable owner process if one was
    /// already committed. This freezes ownership only: an already-owned
    /// provider may still commit its delayed `SessionStart` receipt until
    /// [`Self::final_session`] performs the full receipt seal.
    /// Terminal-generation teardown uses this as the atomic boundary between
    /// possible wrapper startup and workspace-lease release.
    ///
    /// # Errors
    ///
    /// Returns an error when the capture cannot be sealed or its owner cannot
    /// be read safely. Callers must retain the terminal resources and retry.
    fn seal_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        self.capture_process(profile_id, capture_token)
    }

    /// Atomically move a retained-shell capture from one finished provider
    /// process to the exact already-running replacement. The predecessor
    /// checkpoint remains intact until the replacement session is filed.
    ///
    /// # Errors
    ///
    /// Returns an error unless the owner still names `previous`, that exact
    /// process is gone, and `replacement` plus its cwd are safe.
    fn replace_nested_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _previous: &TerminalProcessIdentity,
        _replacement: &TerminalProcessIdentity,
        _cwd: &Path,
        _prior_launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Err(TerminalBackendError::new(
            "nested provider-session capture cannot change process",
        ))
    }

    /// Return a nested capture owner to its retained-shell state after the
    /// exact foreground provider process has exited.
    ///
    /// Implementations must compare the complete expected process identity
    /// and update the owner without deleting/recreating the generation. That
    /// keeps a concurrently registered next invocation from losing its sole
    /// receipt.
    ///
    /// # Errors
    ///
    /// Returns an error unless the nested owner still names `process`, that
    /// exact process is gone, and the replacement cwd is safe.
    fn rearm_nested_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _process: &TerminalProcessIdentity,
        _cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        Err(TerminalBackendError::new(
            "nested provider-session capture cannot be rearmed",
        ))
    }

    /// Advance the durable predecessor after the host has filed one provider
    /// transition. A later crash receipt may be applied only on top of this
    /// exact checkpoint, preventing an older generation from rolling a newer
    /// saved binding backward.
    ///
    /// # Errors
    ///
    /// Returns an error when the checkpoint cannot be committed exactly. The
    /// caller must keep the receipt retryable in that case.
    fn checkpoint_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _launch_session: &TerminalLaunchSessionV1,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    /// Return the provider-owned durable session for one exact spawned process
    /// and generation-scoped capture token.
    ///
    /// # Errors
    ///
    /// Returns an error when provider metadata exists but is malformed,
    /// unsafe, or does not describe the supplied spawn-time identity.
    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>;

    /// Read the final durable receipt for an exact, proven-dead nested
    /// provider without sealing its reusable retained-shell generation.
    /// Callers must durably persist and checkpoint any returned session before
    /// asking the provider to rearm or replace this owner.
    ///
    /// # Errors
    ///
    /// Returns an error unless the durable owner still names `process`, the
    /// exact process is gone, and the receipt remains safe and stable.
    fn settled_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        self.current_session(profile_id, process, capture_token)
    }

    /// Perform the final receipt lookup after this exact process generation
    /// has been reaped but before its public state becomes terminal. The
    /// service, not terminal bytes, establishes the reaped boundary.
    ///
    /// The default preserves simple deterministic lookups; production
    /// providers may relax only live-process checks that are impossible after
    /// reaping while retaining generation-token and durable-store validation.
    ///
    /// # Errors
    ///
    /// Returns the same metadata validation errors as [`Self::current_session`].
    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        self.current_session(profile_id, process, capture_token)
    }

    /// Retire one generation's private capture material after its process has
    /// been reaped. Implementations may leave crash remnants for a later
    /// bounded startup sweep, but must never affect provider transcripts.
    ///
    /// # Errors
    ///
    /// Returns an error when recognized private capture material cannot be
    /// removed safely.
    fn end_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }
}

pub(super) struct UnavailableProviderSessionLookup;

impl TerminalProviderSessionLookup for UnavailableProviderSessionLookup {
    fn current_session(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(None)
    }
}

/// Sink for low-volume terminal lifecycle events. Raw PTY bytes never pass
/// through this hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalProviderSessionPersistence {
    Durable,
    NotSaved,
    RetryableFailure,
}

pub trait TerminalLifecycleSink: Send + Sync {
    fn publish(&self, event: TerminalLifecycleEvent);

    /// Durably file the complete provider-owned reopen identity before its
    /// generation-scoped receipt is retired. Hosts without a saved-workspace
    /// store accept the default; the production desktop host overrides this
    /// and commits launch, session, and cwd as one authority transition.
    fn persist_provider_reopen(
        &self,
        _terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        TerminalProviderSessionPersistence::NotSaved
    }
}
