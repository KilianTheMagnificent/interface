use async_trait::async_trait;
use interface_contracts::engine::{
    EngineEvent, EngineId, EngineInfo, EngineSessionListing, EngineUsage, LaunchOptions,
    ProviderCommand, ResumeOptions,
};
use interface_contracts::ipc::{EngineOp, HistoryPage, unavailable_history};

use crate::{BoundedEventStream, DeliveryOutcome, EngineError, EngineErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalVerdict {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApprovalResponse {
    pub note: Option<String>,
    pub answers: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadySession {
    pub session_id: String,
}

/// Exact launch scope used for control-plane command discovery.
///
/// The engine id is deliberately absent: the caller has already selected an
/// adapter, and carrying a second provider identity would permit a mismatched
/// scope to be answered by the wrong process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderCommandRequest {
    pub cwd: String,
    pub model: Option<String>,
    /// `Some([])` is distinct from `None` for Claude settings isolation.
    pub setting_sources: Option<Vec<String>>,
}

/// Provider-neutral pre-launch command answer before the host stamps scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCommandResult {
    pub available: bool,
    pub commands: Vec<ProviderCommand>,
    pub reason: Option<String>,
}

impl ProviderCommandResult {
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            commands: Vec::new(),
            reason: Some(reason.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRequest {
    pub cwd: String,
    pub limit: usize,
}

/// One live attachment to a provider-owned, resumable session.
#[async_trait]
pub trait SessionHandle: Send + Sync {
    fn engine(&self) -> &EngineId;

    /// Provisional before `ready`, definitive afterwards.
    fn session_id(&self) -> String;

    /// Resolves only after provider readiness and a definitive provider id.
    async fn ready(&self) -> Result<ReadySession, EngineError>;

    /// The event stream has exactly one consumer.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error when the stream was already taken.
    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError>;

    async fn send(&self, text: String) -> DeliveryOutcome<()>;

    /// `send`, firing `dispatched` once the prompt frame is queued for the
    /// provider, or can no longer be; dropping it counts as firing it. The
    /// runtime lets an approval or cancel through after that point instead of
    /// after the whole send, so a send that resolves only when the provider's
    /// turn ends (ACP's `session/prompt`) can still be answered or stopped.
    /// The default fires when `send` returns, which is exact for an adapter
    /// whose send resolves on write acceptance.
    async fn send_reporting_dispatch(
        &self,
        text: String,
        dispatched: tokio::sync::oneshot::Sender<()>,
    ) -> DeliveryOutcome<()> {
        let outcome = self.send(text).await;
        let _ = dispatched.send(());
        outcome
    }

    async fn interrupt(&self) -> DeliveryOutcome<()>;

    /// Detach without deleting the provider transcript.
    async fn close(&self) -> Result<(), EngineError>;

    async fn respond_to_approval(
        &self,
        approval_id: String,
        verdict: ApprovalVerdict,
        response: ApprovalResponse,
    ) -> DeliveryOutcome<()>;

    async fn set_model(&self, _model: Option<String>) -> DeliveryOutcome<()> {
        unsupported("model changes")
    }

    async fn set_effort(&self, _effort: Option<String>) -> DeliveryOutcome<()> {
        unsupported("effort changes")
    }

    async fn set_fast_mode(&self, _enabled: bool) -> DeliveryOutcome<()> {
        unsupported("fast mode")
    }

    async fn set_permission_mode(&self, _mode: String) -> DeliveryOutcome<()> {
        unsupported("permission changes")
    }
}

/// Provider adapter boundary shared by desktop, CLI, and orchestration hosts.
#[async_trait]
pub trait EngineAdapter: Send + Sync {
    fn id(&self) -> &EngineId;

    async fn probe(&self) -> Result<EngineInfo, EngineError>;

    /// The file this adapter launches now, when it has one (a refresh may
    /// repoint it). The runtime keeps a
    /// successful probe only while this path still resolves to the same file
    /// (`FileVersion`); without one a probe is kept for seconds, not minutes.
    fn executable(&self) -> Option<std::path::PathBuf> {
        None
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError>;

    /// How long the runtime may keep this provider's last complete listing
    /// instead of asking again. Zero, the default, asks on every refresh.
    fn listing_reuse(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }

    /// Whether the runtime keeps `session_id`'s row although a complete
    /// listing left it out: a session whose provider account was removed
    /// (protocol 60) is history no store lists any more. False by default.
    fn keeps_unlisted_session(&self, _session_id: &str) -> bool {
        false
    }

    /// Optional engine-store operations advertised to the renderer.
    ///
    /// Returning a method from this list is a promise that the corresponding
    /// implementation below changes the provider's own store. Local registry
    /// mutations must never masquerade as provider support.
    fn supported_ops(&self) -> Vec<EngineOp> {
        Vec::new()
    }

    /// Whether this adapter can replay provider-owned stored transcripts.
    ///
    /// The method is explicit because every Rust implementation necessarily
    /// has the default `read_history` method. Feature-detecting that method,
    /// as the TypeScript bridge did, would otherwise misreport an unsupported
    /// provider as a complete-but-empty history source.
    fn supports_history(&self) -> bool {
        false
    }

    async fn rename_session(
        &self,
        _session_id: String,
        _title: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        Err(unsupported_error("session rename"))
    }

    async fn delete_session(
        &self,
        _session_id: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        Err(unsupported_error("session deletion"))
    }

    async fn fork_session(
        &self,
        _session_id: String,
        _cwd: Option<String>,
    ) -> Result<String, EngineError> {
        Err(unsupported_error("session fork"))
    }

    /// Forks keeping the transcript through the last answer to the owner
    /// prompt `boundary` names, and nothing after it. Answers the new
    /// provider session id. Providers that cannot cut a transcript at a turn
    /// boundary, or cannot find the prompt, refuse definitely-unsent.
    async fn fork_session_at(
        &self,
        _session_id: String,
        _cwd: Option<String>,
        _boundary: ForkBoundary,
    ) -> Result<String, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            FORK_AT_UNSUPPORTED,
            false,
        ))
    }

    async fn set_archived(
        &self,
        _session_id: String,
        _archived: bool,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        Err(unsupported_error("provider archive"))
    }

    async fn adopt_session(
        &self,
        _session_id: String,
        _adopt: bool,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        Err(unsupported_error("provider session adoption"))
    }

    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        Err(unsupported_error("account usage"))
    }

    /// Every usage row this provider meters: its own [`Self::read_usage`],
    /// then (protocol 60) one row per added provider account, each carrying
    /// `account` and `account_label`. Only the first row may be an error.
    async fn read_usage_rows(&self) -> Vec<Result<EngineUsage, EngineError>> {
        vec![self.read_usage().await]
    }

    /// Control-plane-only command discovery. Implementations must not create a
    /// persistent provider session or send a model turn.
    async fn list_provider_commands(
        &self,
        _scope: ProviderCommandRequest,
    ) -> Result<ProviderCommandResult, EngineError> {
        Ok(ProviderCommandResult::unavailable(
            "This provider does not expose pre-launch command discovery.",
        ))
    }

    /// Replay provider-owned history. Unsupported providers answer explicitly;
    /// they never collapse "cannot read" into an empty transcript.
    async fn read_history(
        &self,
        _session_id: String,
        _request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        Ok(unavailable_history(
            "This provider does not expose stored transcript replay.",
        ))
    }

    /// Returns immediately with a handle whose readiness may still be pending.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent error when launch options fail local validation.
    fn create_session(&self, options: LaunchOptions)
    -> Result<Box<dyn SessionHandle>, EngineError>;

    /// Attach to an existing provider session without starting a prompt.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent error when the identifier or resume policy is invalid.
    fn resume_session(
        &self,
        session_id: String,
        cwd: String,
        options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError>;

    async fn dispose(&self) -> Result<(), EngineError> {
        Ok(())
    }

    /// Provider-neutral description of an approval request for Interface's agent policy.
    /// Unrecognized tools must stay `Unknown`, which the policy sends to the owner.
    fn approval_facts(
        &self,
        tool: &str,
        _input: &serde_json::Value,
    ) -> interface_contracts::agents::ApprovalFacts {
        interface_contracts::agents::ApprovalFacts::Unknown {
            tool: tool.to_owned(),
        }
    }
}

fn unsupported<T>(capability: &str) -> DeliveryOutcome<T> {
    DeliveryOutcome::DefinitelyUnsent(unsupported_error(capability))
}

/// The default `fork_session_at` refusal.
pub const FORK_AT_UNSUPPORTED: &str = "This provider cannot fork at an earlier turn.";

/// The `fork_session_at` refusal when no prompt in the provider's history
/// matches the boundary.
pub const FORK_BOUNDARY_NOT_FOUND: &str =
    "Interface can't find that message in the provider's history any more.";

/// Where [`EngineAdapter::fork_session_at`] cuts: after the last answer to
/// one owner prompt, named by identity rather than by position. Counting
/// prompts from the start of the transcript goes wrong whenever the thread
/// host did not record every prompt (history from before the thread, a
/// prompt sent from a terminal), so the adapter walks the provider's
/// prompts from the newest and takes the (`later_same_digest` + 1)-th one
/// whose [`prompt_digest`] is `prompt_digest`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkBoundary {
    /// The kept turn's `ThreadTurn.promptDigest`.
    pub prompt_digest: String,
    /// How many later recorded turns carry the same digest.
    pub later_same_digest: u32,
}

impl ForkBoundary {
    /// Whether `prompt` is a prompt with this boundary's digest.
    #[must_use]
    pub fn matches(&self, prompt: &str) -> bool {
        prompt_digest(prompt) == self.prompt_digest
    }

    /// Of `prompts`, newest first, the one the boundary names.
    pub fn find_newest_first<T>(
        &self,
        prompts: impl IntoIterator<Item = (T, impl AsRef<str>)>,
    ) -> Option<T> {
        prompts
            .into_iter()
            .filter(|(_, prompt)| self.matches(prompt.as_ref()))
            .nth(usize::try_from(self.later_same_digest).unwrap_or(usize::MAX))
            .map(|(found, _)| found)
    }
}

/// `ThreadTurn.promptDigest`: the first 16 hex of sha256 of the prompt with
/// `"\r\n"` folded to `"\n"`, then trimmed. `interface_settings::threads`
/// digests recorded prompts the same way; the thread host's tests pin the
/// two together.
#[must_use]
pub fn prompt_digest(prompt: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let normalized = prompt.replace("\r\n", "\n");
    Sha256::digest(normalized.trim().as_bytes())[..8]
        .iter()
        .fold(String::with_capacity(16), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn unsupported_error(capability: &str) -> EngineError {
    EngineError::definitely_unsent(
        EngineErrorKind::Unsupported,
        format!("engine does not support {capability}"),
        false,
    )
}
