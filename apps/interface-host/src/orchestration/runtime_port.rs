//! The runtime port orchestration drives providers through: the
//! `OrchestrationRuntimePort` trait, its `SessionRuntime` implementation,
//! the launch-access vocabulary and the durable error codes.

use async_trait::async_trait;
use interface_contracts::{
    engine::{
        EngineEvent, EngineId, LaunchOptions, ModelOption, ProviderCommandCatalogue,
        ProviderCommandScope, SandboxMode, SessionStatus, SessionSummary,
    },
    ipc::{HISTORY_MAX_LIMIT, parse_key},
    orchestration::{AgentNodeState, SettledEffectState},
};
use interface_engine::{BoundedEventStream, DeliveryOutcome};
use interface_orchestration::render_orchestration_transcript;
use interface_runtime::{
    ProviderReadiness, RuntimeDelivery, RuntimeError, RuntimeEventFrame, RuntimeSession,
    SessionOpenOutcome, SessionRuntime,
};

use crate::{
    collaboration::{DesktopCollaborationProjection, combine_provider_and_live_transcript},
    registry_access,
};

const CLAUDE_SETTING_SOURCES: [&str; 3] = ["user", "project", "local"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeAgentDescription {
    pub engine_id: String,
    pub provider_session_id: String,
    pub canonical_cwd: String,
    pub canonical_project_root: Option<String>,
    pub name: Option<String>,
    pub state: AgentNodeState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeLaunchRequest {
    pub engine_id: String,
    pub name: String,
    pub prompt: String,
    pub canonical_cwd: String,
    pub access_mode: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeLaunchSuccess {
    pub session_key: String,
    pub provider_session_id: String,
    pub provider_effect_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalOutcome<T> {
    Succeeded {
        value: T,
        provider_effect_id: Option<String>,
    },
    DefinitelyUnsent {
        code: String,
        message: String,
    },
    Unknown {
        code: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeEngineProbe {
    pub steer: bool,
}

impl<T> ExternalOutcome<T> {
    pub(super) fn effect_state(&self) -> SettledEffectState {
        match self {
            Self::Succeeded { .. } => SettledEffectState::Succeeded,
            Self::DefinitelyUnsent { .. } => SettledEffectState::Failed,
            Self::Unknown { .. } => SettledEffectState::Unknown,
        }
    }

    pub(super) fn error_code(&self) -> Option<String> {
        match self {
            Self::Succeeded { .. } => None,
            Self::DefinitelyUnsent { code, .. } | Self::Unknown { code, .. } => Some(code.clone()),
        }
    }
}

#[async_trait]
pub trait OrchestrationRuntimePort: Send + Sync + 'static {
    async fn launch(&self, request: RuntimeLaunchRequest) -> ExternalOutcome<RuntimeLaunchSuccess>;
    async fn send(&self, session_key: &str, body: String) -> ExternalOutcome<()>;
    async fn stop(&self, session_key: &str) -> ExternalOutcome<()>;
    /// The session's last `lines` transcript lines: the provider's stored
    /// history, then whatever `live` (the desktop host's process-live
    /// collaboration projection) holds for the session.
    async fn transcript(
        &self,
        session_key: &str,
        lines: usize,
        live: Option<&DesktopCollaborationProjection>,
    ) -> Result<Vec<String>, RuntimeError>;
    /// Discovers the provider commands for the exact launch scope without
    /// creating a persistent provider session or sending a model turn.
    ///
    /// # Errors
    ///
    /// Returns a runtime scope, provider-discovery, or lifecycle failure.
    async fn provider_command_catalogue(
        &self,
        scope: ProviderCommandScope,
    ) -> Result<ProviderCommandCatalogue, RuntimeError>;
    /// Resolves a durable provider session into the fields needed to adopt it.
    ///
    /// # Errors
    ///
    /// Returns a runtime error when the canonical session registry cannot be read.
    fn describe_session(
        &self,
        session_key: &str,
    ) -> Result<Option<RuntimeAgentDescription>, RuntimeError>;
    async fn take_events(
        &self,
        _session_key: &str,
    ) -> Result<Option<BoundedEventStream<RuntimeEventFrame>>, RuntimeError> {
        Ok(None)
    }
    /// Returns the canonical desktop session projection after a launch or
    /// lifecycle event. Test-only ports may decline this optional projection.
    ///
    /// # Errors
    ///
    /// Returns a runtime registry error when canonical summaries cannot be read.
    fn desktop_session_summaries(&self) -> Result<Option<Vec<SessionSummary>>, RuntimeError> {
        Ok(None)
    }
    /// The registry's content revision for the two-rate registry push. `None`
    /// keeps every registry round at 50 ms.
    fn desktop_registry_content_revision(&self) -> Option<u64> {
        None
    }
    /// Reports whether a live provider attachment still owns the session.
    ///
    /// # Errors
    ///
    /// Returns a runtime lifecycle error when attachment ownership cannot be read.
    fn desktop_session_is_active(&self, _session_key: &str) -> Result<Option<bool>, RuntimeError> {
        Ok(None)
    }
    /// Applies the same terminal lifecycle projection used by ordinary chat
    /// sessions before the desktop receives its final close event.
    ///
    /// # Errors
    ///
    /// Returns a runtime registry error when the canonical event cannot be saved.
    fn project_desktop_event(
        &self,
        _session_key: &str,
        _event: &EngineEvent,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn engine_ids(&self) -> Vec<String>;
    /// Whether the registered adapter exposes provider-owned transcript replay.
    fn supports_provider_history(&self, engine_id: &str) -> bool;
    /// Probe one registered adapter. `None` means the probe did not succeed.
    async fn probe_orchestration_engine(&self, engine_id: &str) -> Option<RuntimeEngineProbe>;
    async fn model_catalogue(&self, engine_id: &str) -> Option<Vec<ModelOption>>;
}

#[async_trait]
impl OrchestrationRuntimePort for SessionRuntime {
    async fn launch(&self, request: RuntimeLaunchRequest) -> ExternalOutcome<RuntimeLaunchSuccess> {
        let engine = match request.engine_id.parse::<EngineId>() {
            Ok(engine) => engine,
            Err(error) => {
                return ExternalOutcome::DefinitelyUnsent {
                    code: "UNKNOWN_ENGINE".into(),
                    message: error.to_string(),
                };
            }
        };
        let (permission_mode, sandbox_mode) =
            orchestration_launch_access(&request.engine_id, &request.access_mode);
        let setting_sources = orchestration_setting_sources(&request.engine_id);
        let outcome = self
            .start_session(
                &engine,
                LaunchOptions {
                    cwd: request.canonical_cwd,
                    prompt: request.prompt,
                    model: request.model,
                    effort: request.effort,
                    name: Some(request.name),
                    permission_mode,
                    sandbox_mode,
                    setting_sources,
                    worktree: Some(false),
                    ..LaunchOptions::default()
                },
            )
            .await;
        match outcome {
            SessionOpenOutcome::Ready(RuntimeSession {
                key, session_id, ..
            }) => {
                let provider_effect_id = Some(key.clone());
                ExternalOutcome::Succeeded {
                    value: RuntimeLaunchSuccess {
                        session_key: key,
                        provider_session_id: session_id,
                        provider_effect_id: provider_effect_id.clone(),
                    },
                    provider_effect_id,
                }
            }
            SessionOpenOutcome::DefinitelyUnsent(error)
            | SessionOpenOutcome::DefinitelyRejected(error) => ExternalOutcome::DefinitelyUnsent {
                code: runtime_error_code(error.kind).to_owned(),
                message: error.message,
            },
            SessionOpenOutcome::OutcomeUnknown(error)
            | SessionOpenOutcome::ProviderReadyLocalFailure { error, .. } => {
                ExternalOutcome::Unknown {
                    code: runtime_error_code(error.kind).to_owned(),
                    message: error.message,
                }
            }
        }
    }

    async fn send(&self, key: &str, body: String) -> ExternalOutcome<()> {
        runtime_delivery_outcome(self.send(key, body).await)
    }

    async fn stop(&self, key: &str) -> ExternalOutcome<()> {
        runtime_delivery_outcome(self.cancel_session(key).await)
    }

    async fn transcript(
        &self,
        key: &str,
        lines: usize,
        live: Option<&DesktopCollaborationProjection>,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut events = self
            .read_provider_history(key, HISTORY_MAX_LIMIT)
            .await?
            .events;
        if let Some(projection) = live {
            let live = projection.live_transcript(key).map_err(|_| {
                RuntimeError::definitely_unsent(
                    interface_runtime::RuntimeErrorKind::History,
                    "Collaboration transcript projection is unavailable.",
                    false,
                )
            })?;
            events = combine_provider_and_live_transcript(events, live);
        }
        Ok(render_orchestration_transcript(&events, lines))
    }

    async fn provider_command_catalogue(
        &self,
        scope: ProviderCommandScope,
    ) -> Result<ProviderCommandCatalogue, RuntimeError> {
        self.list_provider_commands(scope).await
    }

    fn describe_session(&self, key: &str) -> Result<Option<RuntimeAgentDescription>, RuntimeError> {
        let (engine, provider_session_id) = parse_key(key).map_err(|error| {
            RuntimeError::definitely_unsent(
                interface_runtime::RuntimeErrorKind::InvalidInput,
                error.to_string(),
                false,
            )
        })?;
        registry_access::read(self, |registry| {
            registry.get(key).map(|summary| RuntimeAgentDescription {
                engine_id: engine.to_string(),
                provider_session_id,
                canonical_cwd: summary.cwd.clone(),
                canonical_project_root: summary.project_root.clone(),
                name: (!summary.title.is_empty()).then(|| summary.title.clone()),
                state: session_state(summary.status),
            })
        })
    }

    async fn take_events(
        &self,
        key: &str,
    ) -> Result<Option<BoundedEventStream<RuntimeEventFrame>>, RuntimeError> {
        self.take_session_events(key).await.map(Some)
    }

    fn desktop_session_summaries(&self) -> Result<Option<Vec<SessionSummary>>, RuntimeError> {
        registry_access::sorted_visible(self).map(Some)
    }

    fn desktop_registry_content_revision(&self) -> Option<u64> {
        registry_access::content_revision(self)
    }

    fn desktop_session_is_active(&self, session_key: &str) -> Result<Option<bool>, RuntimeError> {
        self.session_is_live(session_key).map(Some)
    }

    fn project_desktop_event(
        &self,
        session_key: &str,
        event: &EngineEvent,
    ) -> Result<(), RuntimeError> {
        registry_access::apply_event(self, session_key, event)
    }

    fn engine_ids(&self) -> Vec<String> {
        self.registered_engines()
            .into_iter()
            .map(|engine| engine.to_string())
            .collect()
    }

    fn supports_provider_history(&self, engine_id: &str) -> bool {
        if !self
            .registered_engines()
            .iter()
            .any(|engine| engine.as_str() == engine_id)
        {
            return false;
        }
        // Interface 0.1.169 registers exactly these provider-store readers.
        // Qwen and custom portable ACP adapters deliberately expose no stored
        // transcript surface.
        matches!(engine_id, "codex" | "claude" | "acp:grok")
    }

    async fn probe_orchestration_engine(&self, engine_id: &str) -> Option<RuntimeEngineProbe> {
        let engine = engine_id.parse::<EngineId>().ok()?;
        match self.probe_provider(&engine).await {
            ProviderReadiness::Ready { info, .. } => Some(RuntimeEngineProbe {
                steer: info.capabilities.steer == Some(true),
            }),
            ProviderReadiness::Unavailable { .. } => None,
        }
    }

    async fn model_catalogue(&self, engine_id: &str) -> Option<Vec<ModelOption>> {
        let engine = engine_id.parse::<EngineId>().ok()?;
        self.orchestration_model_catalogue(&engine).await
    }
}

pub(super) fn orchestration_launch_access(
    engine_id: &str,
    access_mode: &str,
) -> (Option<String>, Option<SandboxMode>) {
    let automatic = match engine_id {
        "claude" | "acp:grok" => Some("auto"),
        "codex" => Some("never"),
        _ => None,
    };
    if engine_id == "claude" {
        return match access_mode {
            "read-only" => (Some("plan".to_owned()), None),
            "isolated" | "coordinated" | "project" => {
                (Some(automatic.unwrap_or("acceptEdits").to_owned()), None)
            }
            _ => (Some(access_mode.to_owned()), None),
        };
    }
    match access_mode {
        "project" | "workspace-write" | "isolated" | "coordinated" => (
            automatic.map(ToOwned::to_owned),
            Some(SandboxMode::WorkspaceWrite),
        ),
        "read-only" => (
            automatic.map(ToOwned::to_owned),
            Some(SandboxMode::ReadOnly),
        ),
        "danger-full-access" => (
            automatic.map(ToOwned::to_owned),
            Some(SandboxMode::DangerFullAccess),
        ),
        _ => (Some(access_mode.to_owned()), None),
    }
}

pub(super) fn orchestration_setting_sources(engine_id: &str) -> Option<Vec<String>> {
    (engine_id == "claude").then(|| {
        CLAUDE_SETTING_SOURCES
            .iter()
            .map(ToString::to_string)
            .collect()
    })
}

/// The durable orchestration error code for a runtime failure. These strings
/// are written into the byte-pinned orchestration store, so they are spelled
/// out here rather than derived from `Debug` output.
pub(super) fn runtime_error_code(kind: interface_runtime::RuntimeErrorKind) -> &'static str {
    use interface_runtime::RuntimeErrorKind as Kind;
    match kind {
        Kind::InvalidInput => "INVALIDINPUT",
        Kind::DuplicateEngine => "DUPLICATEENGINE",
        Kind::EngineNotRegistered => "ENGINENOTREGISTERED",
        Kind::SessionNotActive => "SESSIONNOTACTIVE",
        Kind::Lifecycle => "LIFECYCLE",
        Kind::Provider => "PROVIDER",
        Kind::Registry => "REGISTRY",
        Kind::History => "HISTORY",
        Kind::Internal => "INTERNAL",
    }
}

/// The durable orchestration error code for a delivery failure, spelled out
/// for the same reason as [`runtime_error_code`].
pub(super) fn engine_error_code(kind: interface_engine::EngineErrorKind) -> &'static str {
    use interface_engine::EngineErrorKind as Kind;
    match kind {
        Kind::InvalidInput => "INVALIDINPUT",
        Kind::Unsupported => "UNSUPPORTED",
        Kind::Unavailable => "UNAVAILABLE",
        Kind::Lifecycle => "LIFECYCLE",
        Kind::Capacity => "CAPACITY",
        Kind::Timeout => "TIMEOUT",
        Kind::Transport => "TRANSPORT",
        Kind::Protocol => "PROTOCOL",
        Kind::Provider => "PROVIDER",
        Kind::Internal => "INTERNAL",
    }
}

fn runtime_delivery_outcome(delivery: RuntimeDelivery) -> ExternalOutcome<()> {
    match delivery.outcome {
        DeliveryOutcome::Confirmed(()) => ExternalOutcome::Succeeded {
            value: (),
            provider_effect_id: None,
        },
        DeliveryOutcome::DefinitelyUnsent(error) | DeliveryOutcome::DefinitelyRejected(error) => {
            ExternalOutcome::DefinitelyUnsent {
                code: engine_error_code(error.kind).to_owned(),
                message: error.message,
            }
        }
        DeliveryOutcome::OutcomeUnknown(error) => ExternalOutcome::Unknown {
            code: engine_error_code(error.kind).to_owned(),
            message: error.message,
        },
    }
}

fn session_state(status: SessionStatus) -> AgentNodeState {
    match status {
        SessionStatus::Working => AgentNodeState::Working,
        SessionStatus::Waiting => AgentNodeState::Waiting,
        SessionStatus::Idle => AgentNodeState::Idle,
        SessionStatus::Failed => AgentNodeState::Failed,
    }
}
