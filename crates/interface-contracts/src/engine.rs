use std::{
    error::Error,
    fmt,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use regex::Regex;
use serde::{
    Deserialize, Serialize,
    de::{Error as _, MapAccess, Visitor},
};
use serde_json::Value;

use crate::{
    ValidationError,
    prompt_images::{PromptImageReference, PromptTurnImage},
    validation::{validate_identifier, windows_absolute},
};

pub const COLLABORATION_MCP_TOOL_NAMES: [&str; 13] = [
    "iface_consult",
    "iface_handoff",
    "iface_collaboration_status",
    "iface_collaboration_cancel",
    "iface_workers",
    "iface_spawn_worker",
    "iface_delegate",
    "iface_await",
    "iface_stop_worker",
    "iface_submit_worker",
    "iface_integrate_worker",
    "iface_rebase_worker",
    "iface_models",
];

pub const COLLABORATION_SOURCE_KEYED_TOOL_NAMES: [&str; 4] = [
    "iface_consult",
    "iface_handoff",
    "iface_collaboration_status",
    "iface_collaboration_cancel",
];

static SOURCE_KEYED_TOOLS: LazyLock<String> =
    LazyLock::new(|| COLLABORATION_SOURCE_KEYED_TOOL_NAMES.join(", "));

const COLLABORATION_HOST_CONTEXT_BODY: &str =
    "Interface host context: your collaboration sourceSessionKey is ";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineId(String);

impl EngineId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a validated provider identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not one of the provider-neutral
    /// identifiers admitted by the legacy contract.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value == "claude"
            || value == "codex"
            || value.strip_prefix("acp:").is_some_and(|v| !v.is_empty())
        {
            Ok(Self(value))
        } else {
            Err(ValidationError::new(
                "engine",
                "must be claude, codex, or an acp engine id",
            ))
        }
    }
}

impl Serialize for EngineId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EngineId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for EngineId {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

pub type SessionKey = String;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Working,
    Waiting,
    Idle,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// An agent teammate's launch access other than Interface's approval gate: the
/// provider's own auto mode, or no permission checks at all.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentAccess {
    Auto,
    Bypass,
}

/// `agentAccess` replaces the approval gate on an agent session; it never
/// combines with the gate or reaches another kind of session.
///
/// # Errors
/// Returns why the combination is refused.
pub fn validate_agent_access(
    access: Option<AgentAccess>,
    approval_gate: Option<bool>,
    kind: SessionKind,
) -> Result<(), &'static str> {
    if access.is_none() {
        Ok(())
    } else if approval_gate == Some(true) {
        Err("agent access cannot be combined with the approval gate")
    } else if kind == SessionKind::Agent {
        Ok(())
    } else {
        Err("agent access applies only to agent sessions")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollaborationMcpDescriptorLimits {
    pub name_chars: usize,
    pub command_bytes: usize,
    pub args: usize,
    pub arg_bytes: usize,
    pub total_arg_bytes: usize,
}

pub const COLLABORATION_MCP_DESCRIPTOR_LIMITS: CollaborationMcpDescriptorLimits =
    CollaborationMcpDescriptorLimits {
        name_chars: 64,
        command_bytes: 4_096,
        args: 32,
        arg_bytes: 4_096,
        total_arg_bytes: 16_384,
    };

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationMcpStdioDescriptor {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<Value>,
}

impl CollaborationMcpStdioDescriptor {
    /// Validates the descriptor before it crosses a provider boundary.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error for unsafe names, paths, arguments, or
    /// environment entries.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut name_chars = self.name.chars();
        let safe_name = self.name.len() <= COLLABORATION_MCP_DESCRIPTOR_LIMITS.name_chars
            && name_chars
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
            && name_chars.all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '_' | '-')
            });
        if !safe_name {
            return Err(ValidationError::new(
                "descriptor.name",
                "must be a safe lowercase identifier",
            ));
        }
        if self.command.is_empty()
            || (!self.command.starts_with('/')
                && !self.command.starts_with("\\\\")
                && !windows_absolute(&self.command))
            || self.command.chars().any(char::is_control)
            || self.command.len() > COLLABORATION_MCP_DESCRIPTOR_LIMITS.command_bytes
        {
            return Err(ValidationError::new(
                "descriptor.command",
                "must be a bounded absolute path",
            ));
        }
        if self.args.len() > COLLABORATION_MCP_DESCRIPTOR_LIMITS.args {
            return Err(ValidationError::new(
                "descriptor.args",
                "contains too many arguments",
            ));
        }
        let mut total = 0;
        for argument in &self.args {
            if argument.chars().any(char::is_control)
                || argument.len() > COLLABORATION_MCP_DESCRIPTOR_LIMITS.arg_bytes
            {
                return Err(ValidationError::new(
                    "descriptor.args",
                    "contains an unsafe or oversized argument",
                ));
            }
            total += argument.len();
            if total > COLLABORATION_MCP_DESCRIPTOR_LIMITS.total_arg_bytes {
                return Err(ValidationError::new(
                    "descriptor.args",
                    "exceeds the total byte limit",
                ));
            }
        }
        if !self.env.is_empty() {
            return Err(ValidationError::new(
                "descriptor.env",
                "must be an empty array",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationMcpSessionRequest {
    pub engine: EngineId,
    pub provisional_session_id: String,
    pub resumed: bool,
}

/// Stable, value-free failure returned by a host-owned collaboration binding.
///
/// Provider adapters deliberately receive no underlying I/O error, socket
/// path, command line, or capability.  The frozen TypeScript adapters likewise
/// replace every broker setup failure with a provider-specific safe notice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollaborationBindingError {
    message: &'static str,
}

impl CollaborationBindingError {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            message: "Interface collaboration is unavailable",
        }
    }

    #[must_use]
    pub const fn inactive() -> Self {
        Self {
            message: "Interface collaboration binding is no longer active",
        }
    }

    #[must_use]
    pub const fn invalid() -> Self {
        Self {
            message: "Interface collaboration binding is invalid",
        }
    }
}

impl fmt::Display for CollaborationBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for CollaborationBindingError {}

/// One unique Claude/Grok listener.  Confirmation is part of provider
/// readiness, and disposal must be idempotent.
#[async_trait]
pub trait CollaborationMcpSessionBinding: Send + Sync {
    fn descriptor(&self) -> CollaborationMcpStdioDescriptor;

    async fn confirm_source_session_key(
        &self,
        source_session_key: SessionKey,
    ) -> Result<(), CollaborationBindingError>;

    /// The process the provider runs in, named by the adapter right after
    /// it spawned it. A team tools binding (`iface_team`) answers only that
    /// process and the processes it started, so a command another session
    /// runs cannot speak for this one; until it is named it answers nothing.
    /// Other bindings ignore it.
    async fn provider_process_started(&self, process_id: u32) {
        let _ = process_id;
    }

    async fn dispose(&self) -> Result<(), CollaborationBindingError>;
}

/// Host-only factory for a unique per-session broker binding.
#[async_trait]
pub trait CollaborationMcpSessionFactory: Send + Sync {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Arc<dyn CollaborationMcpSessionBinding>, CollaborationBindingError>;
}

/// The MCP server name of team tools (protocol 57): a Threads thread's agent
/// starts, waits for, reads and messages other threads through it.
pub const TEAM_TOOLS_SERVER: &str = "iface_team";

/// The `iface_team` tools, in catalogue order. Protocol 61 adds
/// `show_visual`; protocol 62 adds the last three: a thread's agent
/// schedules prompts to itself (T3 section 7).
pub const TEAM_TOOL_NAMES: [&str; 13] = [
    "team_info",
    "start_thread",
    "wait_thread",
    "thread_status",
    "stop_thread",
    "list_threads",
    "read_thread",
    "send_to_thread",
    "show_visual",
    "schedule_prompt",
    "list_schedules",
    "cancel_schedule",
    "request_secret",
];

/// Whether a launch or resume may carry team tools at all: an ordinary code
/// session, not behind the approval gate, not an agent teammate, and not a
/// session one hop down (a collaboration peer or a thread team tools started,
/// which run at collaboration depth 1). The host factory decides the rest
/// (the owner's setting, the started-thread ledger).
#[must_use]
pub const fn team_tools_eligible(
    kind: SessionKind,
    approval_gate: bool,
    agent_access: bool,
    collaboration_depth: u8,
) -> bool {
    matches!(kind, SessionKind::Code) && !approval_gate && !agent_access && collaboration_depth == 0
}

/// Host-only factory for one session's `iface_team` server. The caller's
/// identity is the socket the binding serves; it is bound to the session key
/// (`engine#id`, confirmed once the provider answers) and never read from
/// tool input. `Ok(None)` gives the session no team tools (the owner turned
/// them off, or the session is a thread team tools started); an error is a
/// binding that could not be made, and the session starts without it.
#[async_trait]
pub trait TeamToolsFactory: Send + Sync + fmt::Debug {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Option<Arc<dyn CollaborationMcpSessionBinding>>, CollaborationBindingError>;
}

/// Idempotent revoker for one process-scoped Codex source or depth fence.
#[async_trait]
pub trait CollaborationLease: Send + Sync {
    async fn dispose(&self) -> Result<(), CollaborationBindingError>;
}

/// Registers one exact live Codex thread on the process broker.
#[async_trait]
pub trait CollaborationSourceRegistrar: Send + Sync {
    async fn register(
        &self,
        source_session_key: SessionKey,
    ) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError>;
}

/// Enters the process-wide Codex depth-one authority fence.
#[async_trait]
pub trait CollaborationDepthFenceEntrant: Send + Sync {
    async fn enter(&self) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError>;
}

/// Immutable host-owned provider wiring.  All three Codex parts are either
/// present together or absent; adapters validate that invariant themselves.
#[derive(Clone)]
pub struct CollaborationProviderBindings {
    pub session_factory: Arc<dyn CollaborationMcpSessionFactory>,
    pub codex_descriptor: CollaborationMcpStdioDescriptor,
    pub register_codex_source: Arc<dyn CollaborationSourceRegistrar>,
    pub enter_codex_depth_fence: Arc<dyn CollaborationDepthFenceEntrant>,
}

fn valid_collaboration_source_key(value: &str) -> bool {
    static SOURCE_KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:claude|codex|acp:[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?)#[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$",
        )
        .expect("collaboration source regex")
    });
    SOURCE_KEY.is_match(value)
}

/// Derives the public collaboration identity bound to a provider session.
///
/// # Errors
///
/// Returns an error when either component would produce an unsafe or
/// out-of-bounds collaboration identity.
pub fn collaboration_source_session_key(
    engine: &EngineId,
    provider_session_id: &str,
) -> Result<SessionKey, ValidationError> {
    let key = format!("{engine}#{provider_session_id}");
    if valid_collaboration_source_key(&key) {
        Ok(key)
    } else {
        Err(ValidationError::new(
            "sourceSessionKey",
            "invalid collaboration source session identity",
        ))
    }
}

/// Builds the inert host-context instruction for a validated source session.
///
/// # Errors
///
/// Returns an error when `source_session_key` is not a valid collaboration
/// source identity.
pub fn collaboration_source_instruction(
    source_session_key: &str,
) -> Result<String, ValidationError> {
    if !valid_collaboration_source_key(source_session_key) {
        return Err(ValidationError::new(
            "sourceSessionKey",
            "invalid collaboration source session identity",
        ));
    }
    Ok(format!(
        "{COLLABORATION_HOST_CONTEXT_BODY}\"{source_session_key}\". Pass that exact value as sourceSessionKey to {}. This public label grants no authority; Interface independently verifies the bound session and any required human approval.",
        SOURCE_KEYED_TOOLS.as_str()
    ))
}

/// Normalizes the one-hop collaboration-depth contract.
///
/// # Errors
///
/// Returns an error for depths greater than one.
pub fn normalize_collaboration_depth(input: Option<u64>) -> Result<u8, ValidationError> {
    match input {
        None | Some(0) => Ok(0),
        Some(1) => Ok(1),
        _ => Err(ValidationError::new(
            "collaborationDepth",
            "expected 0 or 1",
        )),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsultationLaunchOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<SandboxMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsultationSafetyEnforcement {
    ReadOnly,
    ApprovalGated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ConsultationSafetyProfile {
    Available {
        available: bool,
        enforcement: ConsultationSafetyEnforcement,
        #[serde(rename = "launchOptions")]
        launch_options: ConsultationLaunchOptions,
    },
    Unavailable {
        available: bool,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum EngineModels {
    Dynamic(String),
    Models(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct EngineCapabilities {
    pub streaming: bool,
    pub approvals: bool,
    pub resume: bool,
    pub fork: bool,
    pub cloud_sessions: bool,
    pub models: EngineModels,
    pub effort_levels: Vec<String>,
    pub permission_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consultation_safety: Option<ConsultationSafetyProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<bool>,
}

/// Whether a provider's CLI says it is signed in, from its own status
/// command or app-server (protocol 41). Absent when it cannot say.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineAuth {
    SignedIn,
    SignedOut,
}

/// Why a turn failed, when the provider says so in a way the app can act on
/// (protocol 41).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineErrorCause {
    /// The provider refused because its CLI is not signed in.
    SignedOut,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineInfo {
    pub binary_path: String,
    pub version: String,
    pub login_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EngineAuth>,
    pub capabilities: EngineCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog: Option<Vec<ModelOption>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    App,
    Store,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSummary {
    pub engine: EngineId,
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub status: SessionStatus,
    pub cloud: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Option<String>>,
    pub last_activity: String,
    pub first_seen: String,
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_from_summary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SessionOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SessionKind>,
    /// Protocol 68: the permission mode the host runs this session in and
    /// resumes it in (`session-access.v1.json`; `default` when it keeps none
    /// for it). Set only on the desktop's registry pushes and listings, for
    /// sessions with permission controls outside the agent roster; absent
    /// means the host did not say. Never written to the registry cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineSessionListing {
    pub sessions: Vec<SessionSummary>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastModeState {
    Off,
    Cooldown,
    On,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Chat,
    Code,
    Agent,
    Voice,
}

impl SessionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Code => "code",
            Self::Agent => "agent",
            Self::Voice => "voice",
        }
    }
}

impl FromStr for SessionKind {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "chat" => Ok(Self::Chat),
            "code" => Ok(Self::Code),
            "agent" => Ok(Self::Agent),
            "voice" => Ok(Self::Voice),
            _ => Err(ValidationError::new(
                "kind",
                "must be chat, code, agent, or voice",
            )),
        }
    }
}

#[must_use]
pub const fn session_kind_of(kind: Option<SessionKind>) -> SessionKind {
    match kind {
        Some(kind) => kind,
        None => SessionKind::Code,
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchOptions {
    pub cwd: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<SandboxMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_depth: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SessionKind>,
    /// Every tool call must raise `approval.requested`; adapters that cannot guarantee it refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_gate: Option<bool>,
    /// Extra folders granted natively; adapters that cannot grant them refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<String>>,
    /// An agent session's provider-owned access instead of the approval gate; adapters that
    /// cannot apply it refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_access: Option<AgentAccess>,
    /// Protocol 60: the provider account the session runs on (`default` for
    /// the inherited one). Absent means the provider's default account. The
    /// host binds the session to it; it is never part of the session key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResumeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_depth: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SessionKind>,
    /// Every tool call must raise `approval.requested`; adapters that cannot guarantee it refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_gate: Option<bool>,
    /// Extra folders granted natively; adapters that cannot grant them refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<String>>,
    /// An agent session's provider-owned access instead of the approval gate; adapters that
    /// cannot apply it refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_access: Option<AgentAccess>,
    /// The permission mode the owner last chose for this session (at launch or since),
    /// which the host keeps across restarts. Ignored behind the approval gate or an
    /// agent's access; an adapter that does not know the mode resumes in its default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageWindow {
    pub label: String,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineUsage {
    pub engine: EngineId,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Protocol 60: the added provider account these windows belong to;
    /// absent for the Default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// That account's label, beside its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOption {
    pub value: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub supports_effort: bool,
    pub effort_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_fast_mode: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCommandScope {
    pub engine: EngineId,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCommandCatalogue {
    pub scope: ProviderCommandScope,
    pub available: bool,
    pub commands: Vec<ProviderCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanEntryPriority {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlanEntryStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "in_progress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanEntryPriority,
    pub status: PlanEntryStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextCompactTrigger {
    Manual,
    Auto,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextCategory {
    pub name: String,
    pub tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<ContextCategory>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacting: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ContextUsageInput {
    pub used: Option<f64>,
    pub size: Option<f64>,
    pub percent: Option<f64>,
    pub categories: Option<Vec<ContextCategoryInput>>,
    pub auto_compact: Option<bool>,
    pub auto_compact_threshold: Option<f64>,
    pub compacting: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ContextCategoryInput {
    pub name: Option<String>,
    pub tokens: Option<f64>,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn context_tokens(value: Option<f64>, positive: bool) -> Option<u64> {
    let value = value?;
    if !value.is_finite() || value < 0.0 || (positive && value <= 0.0) {
        None
    } else {
        Some(value.round() as u64)
    }
}

#[must_use]
pub fn context_window_remaining(usage: &ContextUsage) -> Option<u64> {
    Some(usage.size?.saturating_sub(usage.used?))
}

#[must_use]
pub fn context_auto_compact_remaining(usage: &ContextUsage) -> Option<u64> {
    if usage.auto_compact != Some(true) {
        return None;
    }
    Some(usage.auto_compact_threshold?.saturating_sub(usage.used?))
}

#[must_use]
pub fn context_usage_change(
    previous: Option<&ContextUsage>,
    current: &ContextUsage,
) -> Option<i128> {
    let before = previous?.used?;
    previous?.size?;
    let after = current.used?;
    current.size?;
    Some(i128::from(after) - i128::from(before))
}

#[must_use]
pub fn context_usage_percent(used: f64, size: f64) -> Option<u8> {
    if !used.is_finite() || !size.is_finite() || used < 0.0 || size <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = ((used / size * 100.0).round().clamp(0.0, 100.0)) as u8;
    Some(percent)
}

#[must_use]
pub fn format_context_tokens(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return String::new();
    }
    if value < 1_000.0 {
        return format!("{:.0}", value.round());
    }
    if value < 10_000.0 {
        let formatted = format!("{:.1}", (value / 100.0).round() / 10.0);
        return format!("{}k", formatted.strip_suffix(".0").unwrap_or(&formatted));
    }
    format!("{:.0}k", (value / 1_000.0).round())
}

#[must_use]
pub fn normalize_context_usage(input: ContextUsageInput) -> Option<ContextUsage> {
    let used = context_tokens(input.used, false);
    let size = context_tokens(input.size, true);
    let compacting = input.compacting == Some(true);
    let (Some(used), Some(size)) = (used, size) else {
        return compacting.then_some(ContextUsage {
            used: None,
            size: None,
            percent: None,
            categories: None,
            auto_compact: None,
            auto_compact_threshold: None,
            compacting: Some(true),
        });
    };
    let provided = input
        .percent
        .filter(|value| value.is_finite())
        .map(|value| value.round().clamp(0.0, 100.0));
    #[allow(clippy::cast_precision_loss)]
    let derived_percent = || context_usage_percent(used as f64, size as f64).map(f64::from);
    let percent = provided.or_else(derived_percent);
    let categories = input.categories.and_then(|rows| {
        let output = rows
            .into_iter()
            .filter_map(|row| {
                let name = row.name?.trim().to_owned();
                let tokens = context_tokens(row.tokens, false)?;
                (!name.is_empty()).then_some(ContextCategory { name, tokens })
            })
            .take(24)
            .collect::<Vec<_>>();
        (!output.is_empty()).then_some(output)
    });
    Some(ContextUsage {
        used: Some(used),
        size: Some(size),
        percent,
        categories,
        auto_compact: input.auto_compact,
        auto_compact_threshold: context_tokens(input.auto_compact_threshold, false),
        compacting: compacting.then_some(true),
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThroughputBasis {
    EndToEnd,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeTone {
    Info,
    Warning,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitNotice {
    pub message: String,
    /// Protocol 66 (T3 section 10): when the limit resets, as the provider
    /// reported it, in the ISO form usage windows use
    /// (`YYYY-MM-DDTHH:MM:SS.mmmZ`); absent when it did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum EngineEventKind {
    #[serde(rename = "session.meta", rename_all = "camelCase")]
    SessionMeta {
        session_id: String,
        model: String,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    #[serde(rename = "message.delta", rename_all = "camelCase")]
    MessageDelta {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    #[serde(rename = "user.message", rename_all = "camelCase")]
    UserMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        images: Option<Vec<PromptTurnImage>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_references: Option<Vec<PromptImageReference>>,
    },
    #[serde(rename = "peer.result", rename_all = "camelCase")]
    PeerResult {
        operation_id: String,
        source_engine: EngineId,
        source_session_id: String,
        objective: String,
        text: String,
        untrusted: bool,
    },
    #[serde(rename = "thinking.signal", rename_all = "camelCase")]
    ThinkingSignal {
        active: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    #[serde(rename = "tool.call", rename_all = "camelCase")]
    ToolCall {
        call_id: String,
        tool: String,
        summary: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    #[serde(rename = "tool.result", rename_all = "camelCase")]
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    #[serde(rename = "approval.requested", rename_all = "camelCase")]
    ApprovalRequested {
        approval_id: String,
        tool: String,
        summary: String,
        input: Value,
    },
    #[serde(rename = "diff.produced", rename_all = "camelCase")]
    DiffProduced {
        call_id: String,
        path: String,
        before: String,
        after: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    #[serde(rename = "status.changed", rename_all = "camelCase")]
    StatusChanged { status: SessionStatus },
    #[serde(rename = "usage.updated", rename_all = "camelCase")]
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate_limited: Option<RateLimitNotice>,
    },
    #[serde(rename = "throughput.updated", rename_all = "camelCase")]
    ThroughputUpdated {
        output_tokens: u64,
        duration_ms: u64,
        basis: ThroughputBasis,
    },
    #[serde(rename = "context.usage", rename_all = "camelCase")]
    ContextUsage { usage: ContextUsage },
    #[serde(rename = "context.compact", rename_all = "camelCase")]
    ContextCompact {
        trigger: ContextCompactTrigger,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<bool>,
    },
    #[serde(rename = "turn.retry", rename_all = "camelCase")]
    TurnRetry {
        attempt: u64,
        max_attempts: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_ms: Option<u64>,
        message: String,
    },
    #[serde(rename = "notice", rename_all = "camelCase")]
    Notice { message: String, tone: NoticeTone },
    #[serde(rename = "error", rename_all = "camelCase")]
    Error {
        message: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<EngineErrorCause>,
    },
    #[serde(rename = "models.available", rename_all = "camelCase")]
    ModelsAvailable { models: Vec<ModelOption> },
    #[serde(rename = "commands.available", rename_all = "camelCase")]
    CommandsAvailable { commands: Vec<ProviderCommand> },
    #[serde(rename = "commands.invalidated")]
    CommandsInvalidated,
    #[serde(rename = "config.changed", rename_all = "camelCase")]
    ConfigChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<Option<String>>,
    },
    #[serde(rename = "access.changed", rename_all = "camelCase")]
    AccessChanged { mode: String },
    #[serde(rename = "fastmode.changed", rename_all = "camelCase")]
    FastmodeChanged {
        state: FastModeState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "plan.updated", rename_all = "camelCase")]
    PlanUpdated { entries: Vec<PlanEntry> },
    #[serde(rename = "subagent.started", rename_all = "camelCase")]
    SubagentStarted {
        agent_key: String,
        subagent_type: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    #[serde(rename = "subagent.finished", rename_all = "camelCase")]
    SubagentFinished {
        agent_key: String,
        ok: bool,
        summary: String,
    },
    /// A page an agent showed through team tools' `show_visual` (protocol
    /// 61). The event names it only; its bytes are read through
    /// `iface:threadJournal` `visual`.
    #[serde(rename = "visual.shown", rename_all = "camelCase")]
    VisualShown {
        id: String,
        title: String,
        bytes: u64,
    },
    #[serde(rename = "session.closed")]
    SessionClosed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EngineEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(flatten)]
    pub event: EngineEventKind,
}

impl<'de> Deserialize<'de> for EngineEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EngineEventVisitor;

        impl<'de> Visitor<'de> for EngineEventVisitor {
            type Value = EngineEvent;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an engine event object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut at = None;
                let mut saw_at = false;
                let mut event_fields = serde_json::Map::new();

                while let Some(key) = map.next_key::<String>()? {
                    let value = map.next_value::<Value>()?;
                    if key == "at" {
                        if saw_at {
                            return Err(M::Error::duplicate_field("at"));
                        }
                        saw_at = true;
                        at = serde_json::from_value(value).map_err(M::Error::custom)?;
                    } else {
                        match event_fields.entry(key) {
                            serde_json::map::Entry::Vacant(slot) => {
                                slot.insert(value);
                            }
                            serde_json::map::Entry::Occupied(slot) => {
                                return Err(M::Error::custom(format_args!(
                                    "duplicate field `{}`",
                                    slot.key()
                                )));
                            }
                        }
                    }
                }

                let event = serde_json::from_value(Value::Object(event_fields))
                    .map_err(M::Error::custom)?;
                Ok(EngineEvent { at, event })
            }
        }

        deserializer.deserialize_map(EngineEventVisitor)
    }
}

impl EngineEvent {
    /// Size of the frozen JSON event without allocating an encoded copy.
    /// This measures content for retention budgets, not allocator overhead.
    /// An unencodable event consumes the entire budget.
    #[must_use]
    pub fn serialized_len(&self) -> usize {
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, self).map_or(usize::MAX, |()| counter.0)
    }

    /// Validates event invariants that Serde alone cannot express.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid timestamps or trusted peer-result payloads.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(at) = &self.at {
            validate_identifier(at, "event.at", 128)?;
        }
        if let EngineEventKind::PeerResult { untrusted, .. } = self.event {
            if !untrusted {
                return Err(ValidationError::new("event.untrusted", "must be true"));
            }
        }
        Ok(())
    }
}

#[must_use]
pub fn session_key(engine: &EngineId, session_id: &str) -> SessionKey {
    format!("{engine}#{session_id}")
}

#[must_use]
pub const fn next_status(previous: SessionStatus, event: &EngineEvent) -> SessionStatus {
    match &event.event {
        EngineEventKind::ApprovalRequested { .. } => SessionStatus::Waiting,
        EngineEventKind::StatusChanged { status } => *status,
        EngineEventKind::Error {
            retryable: false, ..
        } => SessionStatus::Failed,
        EngineEventKind::MessageDelta { .. }
        | EngineEventKind::ToolCall { .. }
        | EngineEventKind::ToolResult { .. }
            if !matches!(previous, SessionStatus::Waiting) =>
        {
            SessionStatus::Working
        }
        _ => previous,
    }
}

/// Host-owned, task-bound tools. No renderer or model can select a descriptor.
pub trait AgentToolsProvider: Send + Sync + std::fmt::Debug {
    fn descriptor_for(&self, cwd: &std::path::Path) -> Option<CollaborationMcpStdioDescriptor>;
    fn descriptor_for_resume(
        &self,
        cwd: &std::path::Path,
        _session_key: &str,
    ) -> Option<CollaborationMcpStdioDescriptor> {
        self.descriptor_for(cwd)
    }
}

#[cfg(test)]
mod provider_v40_tests {
    use serde_json::Value;

    use super::*;
    use crate::terminal::CreateTerminalRequest;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/providers-v41.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn signed_out_providers_and_sign_in_terminals_match_the_shared_fixture() {
        let fixture = fixture();
        let info: EngineInfo =
            serde_json::from_value(fixture["signedOutInfo"].clone()).expect("info");
        assert_eq!(info.auth, Some(EngineAuth::SignedOut));
        assert_eq!(
            serde_json::to_value(&info).expect("encode"),
            fixture["signedOutInfo"]
        );
        let error: EngineEventKind =
            serde_json::from_value(fixture["signedOutError"].clone()).expect("error");
        assert!(matches!(
            error,
            EngineEventKind::Error {
                cause: Some(EngineErrorCause::SignedOut),
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&error).expect("encode"),
            fixture["signedOutError"]
        );
        // An ordinary error keeps the pre-v41 bytes.
        let plain: EngineEventKind =
            serde_json::from_value(fixture["plainError"].clone()).expect("plain");
        assert_eq!(
            serde_json::to_value(&plain).expect("encode"),
            fixture["plainError"]
        );
        let terminal: CreateTerminalRequest =
            serde_json::from_value(fixture["signInTerminal"].clone()).expect("terminal");
        assert!(terminal.sign_in);
        terminal.validate_sign_in().expect("a valid sign-in");
        assert_eq!(
            serde_json::to_value(&terminal).expect("encode"),
            fixture["signInTerminal"]
        );
        for refused in fixture["refusedSignInTerminals"]
            .as_array()
            .expect("refused")
        {
            let request: CreateTerminalRequest =
                serde_json::from_value(refused.clone()).expect("decodes");
            assert!(request.validate_sign_in().is_err(), "{refused}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine(value: &str) -> EngineId {
        value.parse().unwrap()
    }

    #[test]
    fn agent_access_launch_and_resume_options_round_trip_and_validate() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-permission-modes-v32.json"
        ))
        .unwrap();
        for value in fixture["launchOptions"].as_array().unwrap() {
            let options: LaunchOptions = serde_json::from_value(value.clone()).unwrap();
            assert!(options.agent_access.is_some());
            assert_eq!(serde_json::to_value(options).unwrap(), *value);
        }
        for value in fixture["resumeOptions"].as_array().unwrap() {
            let options: ResumeOptions = serde_json::from_value(value.clone()).unwrap();
            assert!(options.agent_access.is_some());
            assert_eq!(serde_json::to_value(options).unwrap(), *value);
        }
        assert!(
            serde_json::from_value::<LaunchOptions>(
                json!({"cwd":"/a","prompt":"","agentAccess":"everything"})
            )
            .is_err()
        );
        assert!(
            serde_json::to_value(LaunchOptions::default())
                .unwrap()
                .get("agentAccess")
                .is_none()
        );
        let agent = SessionKind::Agent;
        assert!(validate_agent_access(None, Some(true), agent).is_ok());
        assert!(validate_agent_access(None, None, SessionKind::Code).is_ok());
        assert!(validate_agent_access(Some(AgentAccess::Auto), None, agent).is_ok());
        assert!(validate_agent_access(Some(AgentAccess::Bypass), Some(false), agent).is_ok());
        assert!(validate_agent_access(Some(AgentAccess::Auto), Some(true), agent).is_err());
        assert!(validate_agent_access(Some(AgentAccess::Bypass), None, SessionKind::Code).is_err());
    }

    #[test]
    fn engine_event_uses_the_exact_legacy_discriminator_and_field_names() {
        let event = EngineEvent {
            at: Some("2026-08-27T12:00:00.000Z".into()),
            event: EngineEventKind::DiffProduced {
                call_id: "call-1".into(),
                path: "/tmp/a.rs".into(),
                before: "old".into(),
                after: "new".into(),
                agent_key: Some("worker-1".into()),
            },
        };
        let golden = json!({
            "at": "2026-08-27T12:00:00.000Z",
            "kind": "diff.produced",
            "callId": "call-1",
            "path": "/tmp/a.rs",
            "before": "old",
            "after": "new",
            "agentKey": "worker-1"
        });
        assert_eq!(serde_json::to_value(&event).unwrap(), golden);
        assert_eq!(
            serde_json::from_value::<EngineEvent>(golden).unwrap(),
            event
        );
    }

    #[test]
    fn engine_event_round_trips_flattened_message_delta_strictly() {
        let golden = json!({
            "at": "2026-08-27T12:00:00.000Z",
            "kind": "message.delta",
            "text": "hello",
            "agentKey": "worker-1"
        });
        let decoded = serde_json::from_value::<EngineEvent>(golden.clone()).unwrap();
        assert_eq!(
            decoded,
            EngineEvent {
                at: Some("2026-08-27T12:00:00.000Z".into()),
                event: EngineEventKind::MessageDelta {
                    text: "hello".into(),
                    agent_key: Some("worker-1".into()),
                },
            }
        );
        assert_eq!(serde_json::to_value(decoded).unwrap(), golden);

        assert!(
            serde_json::from_value::<EngineEvent>(json!({
                "kind": "message.delta",
                "text": "hello",
                "unexpected": true
            }))
            .is_err()
        );
        let duplicate = serde_json::from_str::<EngineEvent>(
            r#"{"kind":"message.delta","text":"a","text":"b"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(duplicate.contains("duplicate field `text`"));
    }

    #[test]
    fn status_transition_keeps_the_waiting_hold_rule() {
        let delta = EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "x".into(),
                agent_key: None,
            },
        };
        assert_eq!(
            next_status(SessionStatus::Idle, &delta),
            SessionStatus::Working
        );
        assert_eq!(
            next_status(SessionStatus::Waiting, &delta),
            SessionStatus::Waiting
        );
        let closed = EngineEvent {
            at: None,
            event: EngineEventKind::SessionClosed,
        };
        assert_eq!(
            next_status(SessionStatus::Working, &closed),
            SessionStatus::Working
        );
    }

    #[test]
    fn collaboration_source_identity_is_bounded_and_inert() {
        let key = collaboration_source_session_key(&engine("acp:grok"), "session-1").unwrap();
        assert_eq!(key, "acp:grok#session-1");
        assert!(collaboration_source_session_key(&engine("claude"), "bad\ntext").is_err());
        let instruction = collaboration_source_instruction(&key).unwrap();
        assert!(instruction.contains("grants no authority"));
        assert!(instruction.contains(SOURCE_KEYED_TOOLS.as_str()));
    }

    #[test]
    fn context_usage_matches_the_legacy_derivations() {
        let usage = normalize_context_usage(ContextUsageInput {
            used: Some(250.4),
            size: Some(1_000.0),
            ..ContextUsageInput::default()
        })
        .unwrap();
        assert_eq!(usage.used, Some(250));
        assert_eq!(usage.percent, Some(25.0));
        assert_eq!(context_window_remaining(&usage), Some(750));
        assert_eq!(format_context_tokens(9_500.0), "9.5k");
        assert_eq!(format_context_tokens(10_500.0), "11k");
    }

    #[test]
    fn descriptor_rejects_environment_and_relative_commands() {
        let valid = CollaborationMcpStdioDescriptor {
            name: "interface".into(),
            command: "/Applications/Interface.app/Contents/MacOS/iface".into(),
            args: vec!["broker".into()],
            env: vec![],
        };
        assert!(valid.validate().is_ok());
        let mut invalid = valid.clone();
        invalid.env.push(json!({"TOKEN": "redacted"}));
        assert!(invalid.validate().is_err());
        invalid.env.clear();
        invalid.command = "iface".into();
        assert!(invalid.validate().is_err());
    }

    /// Protocol 68: a session's `permissionMode` round-trips byte for byte,
    /// `default` included, and stays absent when the host said nothing.
    #[test]
    fn session_mode_fixture_round_trips_the_hosts_permission_mode() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/session-mode-v68.json"
        ))
        .unwrap();
        for (name, mode) in [
            ("withMode", Some("acceptEdits")),
            ("withDefault", Some("default")),
            ("withoutMode", None),
        ] {
            let summary: SessionSummary = serde_json::from_value(fixture[name].clone()).unwrap();
            assert_eq!(summary.permission_mode.as_deref(), mode, "{name}");
            assert_eq!(
                serde_json::to_value(&summary).unwrap(),
                fixture[name],
                "{name}"
            );
        }
    }

    #[test]
    fn session_kind_contract_fixture_decodes_launch_and_summary() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/session-kind-contract.json"
        ))
        .unwrap();
        assert_eq!(fixture["kinds"], json!(["chat", "code", "agent", "voice"]));
        assert_eq!(fixture["defaultKind"], "code");
        for kind in fixture["kinds"].as_array().unwrap() {
            let decoded: SessionKind = serde_json::from_value(kind.clone()).unwrap();
            assert_eq!(decoded.as_str(), kind.as_str().unwrap());
            assert_eq!(
                kind.as_str().unwrap().parse::<SessionKind>().unwrap(),
                decoded
            );
        }
        assert_eq!("voice".parse::<SessionKind>().unwrap(), SessionKind::Voice);
        assert_eq!(session_kind_of(None), SessionKind::Code);
        let launch: LaunchOptions = serde_json::from_value(fixture["launch"].clone()).unwrap();
        assert_eq!(launch.kind, Some(SessionKind::Chat));
        assert_eq!(session_kind_of(launch.kind), SessionKind::Chat);
        let summary: SessionSummary = serde_json::from_value(fixture["summary"].clone()).unwrap();
        assert_eq!(summary.kind, Some(SessionKind::Agent));
        assert!("banana".parse::<SessionKind>().is_err());
        assert!(serde_json::from_value::<SessionKind>(json!("banana")).is_err());
    }

    #[test]
    fn session_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(SessionKind::Chat).unwrap(),
            json!("chat")
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Code).unwrap(),
            json!("code")
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Agent).unwrap(),
            json!("agent")
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Voice).unwrap(),
            json!("voice")
        );
    }

    #[test]
    fn launch_options_omit_absent_kind_bytes() {
        let options = LaunchOptions {
            cwd: "/tmp/chat-home".into(),
            prompt: "hello".into(),
            kind: None,
            ..LaunchOptions::default()
        };
        let encoded = serde_json::to_value(&options).unwrap();
        let object = encoded
            .as_object()
            .expect("LaunchOptions encodes as an object");
        assert!(
            !object.contains_key("kind"),
            "absent kind must be omitted from the wire object: {encoded}"
        );
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(
            serde_json::from_value::<LaunchOptions>(json!({
                "cwd": "/tmp/chat-home",
                "prompt": "hello",
                "kind": "banana"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SessionSummary>(json!({
                "engine": "claude",
                "sessionId": "s1",
                "title": "task",
                "cwd": "/tmp/chat-home",
                "status": "idle",
                "cloud": false,
                "lastActivity": "2026-08-29T00:00:00.000Z",
                "firstSeen": "2026-08-29T00:00:00.000Z",
                "archived": false,
                "kind": "banana"
            }))
            .is_err()
        );
    }

    #[test]
    fn provider_command_wire_format_is_camel_case() {
        let command = ProviderCommand {
            name: "review".into(),
            description: "Review changes".into(),
            argument_hint: Some("<path>".into()),
            aliases: Some(vec!["inspect".into()]),
        };
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({"name":"review","description":"Review changes","argumentHint":"<path>","aliases":["inspect"]})
        );
    }
}
