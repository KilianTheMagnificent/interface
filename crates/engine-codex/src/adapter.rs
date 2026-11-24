//! The Codex `EngineAdapter`: probe, listing, stored-session operations,
//! history and usage reads, and creating or resuming sessions. Each part of a
//! session lives in its own submodule:
//!
//! - `lanes` — read-only and control connections to the app-server.
//! - `models` — the model catalogue and Fast service tiers.
//! - `launch` — launch and resume admission.
//! - `open` — opening a session and verifying the opened thread.
//! - `gate` — agent approval modes and checking that Codex applied them.
//! - `session` — the live session handle, closing, and the inbound loop.
//! - `turns` — sending prompts, steering and starting turns.
//! - `commands` — the provider command catalogue and skills.
//! - `dispatch` — running provider commands typed into a session.
//! - `approvals` — approvals and questions, and the facts the policy reads.
//! - `config` — mid-session model, effort, approval and Fast changes.

mod approvals;
mod commands;
mod config;
mod dispatch;
mod gate;
mod lanes;
mod launch;
mod models;
mod open;
mod session;
mod turns;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use interface_common::{
    sync::lock,
    time::{IsoFraction, unix_seconds_iso},
};
use interface_contracts::{
    agents::ApprovalFacts,
    engine::{
        AgentAccess, CollaborationDepthFenceEntrant, CollaborationMcpStdioDescriptor,
        CollaborationProviderBindings, CollaborationSourceRegistrar, ConsultationSafetyEnforcement,
        ConsultationSafetyProfile, EngineAuth, EngineCapabilities, EngineId, EngineInfo,
        EngineModels, EngineSessionListing, EngineUsage, LaunchOptions, ResumeOptions, SandboxMode,
        SessionStatus, SessionSummary,
    },
    ipc::{EngineOp, HistoryPage},
    provider_commands::normalize_provider_commands,
    retention::clip_label,
};
use interface_engine::{
    EngineAdapter, EngineError, EngineErrorKind, ExecutablePath, ForkBoundary, HistoryRequest,
    ProviderCommandRequest, ProviderCommandResult, SessionHandle, VersionCache, cli_version,
    jsonrpc::text_fits_frame, session_core::value_text,
};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    executable::codex_app_server_process,
    mapper::{collapse_whitespace, fork_turn_boundary, map_engine_usage, stored_history_page},
    protocol::CodexMethod,
    transport::CodexTransportLimits,
};

use approvals::codex_approval_facts;
use commands::{codex_builtin_command_candidates, skill_command_candidates};
use lanes::{ControlConnection, SpareLane};
use launch::{validate_launch, validate_resume};
use models::{FastTierCatalogue, model_catalog};
use open::Opening;
use session::{SessionShared, close_shared};

// The tests reach these through `super::*`; this module itself does not use them.
#[cfg(test)]
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
};

#[cfg(test)]
use interface_contracts::engine::{
    CollaborationLease, ContextCompactTrigger, EngineEvent, EngineEventKind, ProviderCommand,
    SessionKind,
};
#[cfg(test)]
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, jsonrpc::RpcId,
    shell_words::shell_word,
};

#[cfg(test)]
use crate::protocol::{ThreadResumeParams, ThreadStartParams};

#[cfg(test)]
use approvals::{
    MAX_FILE_CHANGE_ITEMS, PendingApproval, approval_result, file_change_targets,
    remember_file_change_paths,
};
#[cfg(test)]
use gate::{
    GateRequest, agent_tool_config, configured_mcp_servers, gate_overrides, gate_request,
    matches_reported_implicit_temp, verify_gate,
};
#[cfg(test)]
use launch::ignored_launch_scoping;
#[cfg(test)]
use open::{codex_child_environment, verify_requested_sandbox};
#[cfg(test)]
use turns::{steer_fallback_safe, steer_not_steerable};

const EVENT_CAPACITY: usize = 5_000;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Codex 0.157.1 answers at most 100 threads a page whatever this asks
/// (checked 27 Sep 2026), so a larger value would not save a store read.
const LIST_PAGE_SIZE: u64 = 200;
const LIST_MAX_PAGES: usize = 64;
/// Stop asking for further pages once a listing has run this long and return
/// what arrived. Codex reads its whole session store per page (7 s seen for one
/// page over 8 GB), and the runtime drops a listing entirely at 45 s.
const LIST_TIME_BUDGET: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(30)
};
/// How long the runtime keeps the last complete listing before asking Codex
/// again. Every page reads Codex's whole session store, so a refresh inside
/// this window reuses the rows already merged; a session another Codex client
/// creates shows up at most this much later. An explicit refresh starts over.
const LISTING_REUSE: Duration = Duration::from_secs(60);
/// A spare history lane's child exits after sitting unused this long. The
/// shared listing lane keeps its child for the adapter's life.
const SPARE_LANE_IDLE: Duration = Duration::from_secs(120);
const MAX_PROMPT_BYTES: usize = 8 * 1024 * 1024;

const PERMISSION_MODES: [&str; 3] = ["untrusted", "on-request", "never"];
/// A gated agent thread asks before every action Codex does not consider known-safe.
const GATE_APPROVAL_POLICY: &str = "untrusted";
const GATE_APPROVALS_REVIEWER: &str = "user";

/// How an agent thread gets permission: Interface's gate, Codex's own reviewer
/// behind the same sandbox, or no approvals and no sandbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentMode {
    Gated,
    Auto,
    Bypass,
}

impl AgentMode {
    const fn approval_policy(self) -> &'static str {
        match self {
            Self::Gated => GATE_APPROVAL_POLICY,
            Self::Auto => "on-request",
            Self::Bypass => "never",
        }
    }
    const fn approvals_reviewer(self) -> Option<&'static str> {
        match self {
            Self::Gated => Some(GATE_APPROVALS_REVIEWER),
            Self::Auto => Some("auto_review"),
            Self::Bypass => None,
        }
    }
    const fn sandbox(self) -> SandboxMode {
        match self {
            Self::Gated | Self::Auto => SandboxMode::WorkspaceWrite,
            Self::Bypass => SandboxMode::DangerFullAccess,
        }
    }
    /// Gated and Auto switch off the owner's MCP servers and the acting features
    /// that run without an approval; Bypass leaves Codex as the owner set it up.
    const fn restricts(self) -> bool {
        !matches!(self, Self::Bypass)
    }
}

fn agent_mode(approval_gate: Option<bool>, access: Option<AgentAccess>) -> Option<AgentMode> {
    match (approval_gate == Some(true), access) {
        (true, _) => Some(AgentMode::Gated),
        (false, Some(AgentAccess::Auto)) => Some(AgentMode::Auto),
        (false, Some(AgentAccess::Bypass)) => Some(AgentMode::Bypass),
        (false, None) => None,
    }
}

fn codex_chat_refusal() -> &'static str {
    static REFUSAL: LazyLock<String> = LazyLock::new(|| {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/session-kind-contract.json"
        ))
        .expect("session-kind contract fixture");
        fixture["codexChatRefusal"]
            .as_str()
            .expect("codexChatRefusal")
            .to_owned()
    });
    REFUSAL.as_str()
}

/// Signed in or out by the app-server's own `account/read`: an account is
/// signed in; none, where the configured provider needs `OpenAI` auth, is
/// signed out; none where it does not (a local provider) says nothing.
fn codex_auth(account: &Value) -> Option<EngineAuth> {
    match account.get("account") {
        Some(Value::Object(_)) => Some(EngineAuth::SignedIn),
        Some(Value::Null) | None
            if account.get("requiresOpenaiAuth") == Some(&Value::Bool(true)) =>
        {
            Some(EngineAuth::SignedOut)
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct CodexAdapterConfig {
    pub agent_tools: Option<Arc<dyn interface_contracts::engine::AgentToolsProvider>>,
    /// The host's team tools factory (`iface_team`, protocol 57): an ordinary
    /// code thread gets the server through its thread config.
    pub team_tools: Option<Arc<dyn interface_contracts::engine::TeamToolsFactory>>,
    /// The executable at construction. Launches read [`Self::executable_path`].
    pub executable: PathBuf,
    /// What every launch reads, shared with the host so a refresh can point
    /// the adapter at a CLI installed while Interface runs. `new` starts it
    /// at `executable`.
    pub executable_path: ExecutablePath,
    /// Sent as `clientInfo.version`. Defaults to this crate's version; the
    /// binaries set their release version here at runtime, so this crate never
    /// compiles a release version in.
    pub client_version: String,
    pub transport: CodexTransportLimits,
    pub event_capacity: usize,
    /// Set on every app-server child after its search path: an added provider
    /// account's `CODEX_HOME`. An adapter is one account, so each account has
    /// its own sessions and control lanes. Empty for the Default account.
    pub environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl CodexAdapterConfig {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        let executable = executable.into();
        Self {
            agent_tools: None,
            team_tools: None,
            executable_path: ExecutablePath::new(executable.clone()),
            executable,
            client_version: env!("CARGO_PKG_VERSION").into(),
            transport: CodexTransportLimits::default(),
            event_capacity: EVENT_CAPACITY,
            environment: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
struct CodexCollaborationRuntime {
    descriptor: Option<CollaborationMcpStdioDescriptor>,
    registrar: Option<Arc<dyn CollaborationSourceRegistrar>>,
    depth_fence: Option<Arc<dyn CollaborationDepthFenceEntrant>>,
    process_enabled: Arc<AtomicBool>,
    startup_unavailable: Arc<AtomicBool>,
    adapter_disposed: Arc<AtomicBool>,
}

impl CodexCollaborationRuntime {
    /// Codex raises no approval for MCP tool calls, so a gated thread never sees the
    /// collaboration MCP: a consult or handoff would launch an ungated target.
    fn for_session(&self, approval_gate: bool) -> Self {
        if approval_gate {
            Self {
                descriptor: None,
                registrar: None,
                depth_fence: None,
                ..self.clone()
            }
        } else {
            self.clone()
        }
    }
}

pub struct CodexAdapter {
    id: EngineId,
    config: CodexAdapterConfig,
    sessions: Mutex<Vec<Weak<SessionShared>>>,
    // One supervised, read-only listing/history lane. A checked-out connection stays
    // local to the future: cancellation/failure drops and shuts it down rather
    // than returning an ambiguous operation to the pool. Mutations never use it.
    listing_connection: AsyncMutex<Option<ControlConnection>>,
    // At most four concurrent stored-history reads. Spare children are created
    // only when the shared lane is occupied; each still has one read owner and
    // is discarded on cancellation or failure, or once it sits unused for
    // `spare_idle`. Provider mutations never enter.
    history_connections: Arc<[SpareLane; 3]>,
    spare_idle: Duration,
    /// The last `codex --version` answer, kept while the executable path
    /// resolves to the same file.
    version: VersionCache,
    fast_tiers: Arc<Mutex<FastTierCatalogue>>,
    collaboration: CodexCollaborationRuntime,
}

impl CodexAdapter {
    /// `thread/fork`, whole or through `last_turn_id` (inclusive).
    async fn fork_thread(
        &self,
        session_id: String,
        cwd: Option<String>,
        last_turn_id: Option<String>,
    ) -> Result<String, EngineError> {
        let mut params = json!({"threadId": session_id});
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            params["cwd"] = Value::String(cwd);
        }
        if let Some(last_turn_id) = last_turn_id {
            params["lastTurnId"] = Value::String(last_turn_id);
        }
        let response = self
            .control_request(CodexMethod::THREAD_FORK, Some(params), "session fork")
            .await?;
        value_text(&response, &["thread", "id"])
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                EngineError::definitely_rejected(
                    EngineErrorKind::Protocol,
                    "codex app-server forked the thread but did not name the new one",
                    false,
                )
            })
    }

    /// Construct an adapter without invoking the provider.
    ///
    /// # Errors
    ///
    /// Returns a local validation error for a relative executable or zero
    /// event capacity.
    pub fn new(config: CodexAdapterConfig) -> Result<Self, EngineError> {
        codex_app_server_process(&config.executable).map_err(|error| {
            EngineError::definitely_unsent(EngineErrorKind::InvalidInput, error.to_string(), false)
        })?;
        if config.event_capacity == 0 {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "Codex event capacity must be positive",
                false,
            ));
        }
        Ok(Self {
            id: EngineId::new("codex").map_err(|error| {
                EngineError::definitely_unsent(
                    EngineErrorKind::Internal,
                    format!("invalid built-in Codex id: {error}"),
                    false,
                )
            })?,
            config,
            sessions: Mutex::new(Vec::new()),
            listing_connection: AsyncMutex::new(None),
            history_connections: Arc::default(),
            spare_idle: SPARE_LANE_IDLE,
            version: VersionCache::default(),
            fast_tiers: Arc::new(Mutex::new(FastTierCatalogue::default())),
            collaboration: CodexCollaborationRuntime::default(),
        })
    }

    /// Install the complete host-owned Codex collaboration bundle.
    /// Descriptor, source registrar, and depth fence are installed together.
    ///
    /// # Errors
    ///
    /// Refuses an invalid or environment-bearing process descriptor.
    pub fn with_collaboration_bindings(
        mut self,
        bindings: &CollaborationProviderBindings,
    ) -> Result<Self, EngineError> {
        bindings.codex_descriptor.validate().map_err(|_| {
            EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "Codex collaboration descriptor is invalid",
                false,
            )
        })?;
        self.collaboration = CodexCollaborationRuntime {
            descriptor: Some(bindings.codex_descriptor.clone()),
            registrar: Some(Arc::clone(&bindings.register_codex_source)),
            depth_fence: Some(Arc::clone(&bindings.enter_codex_depth_fence)),
            process_enabled: Arc::new(AtomicBool::new(true)),
            startup_unavailable: Arc::new(AtomicBool::new(false)),
            adapter_disposed: Arc::new(AtomicBool::new(false)),
        };
        Ok(self)
    }

    /// `codex --version`, spawned only when the executable changed since its
    /// last successful answer.
    async fn cli_version(&self) -> Result<String, EngineError> {
        let executable = &self.config.executable_path.get();
        self.version
            .get_or_probe(
                executable,
                cli_version(
                    executable,
                    "Codex",
                    "Codex version probe returned no version",
                    PROBE_TIMEOUT,
                ),
            )
            .await
    }
}

#[async_trait]
impl EngineAdapter for CodexAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }

    fn executable(&self) -> Option<PathBuf> {
        Some(self.config.executable_path.get())
    }

    fn listing_reuse(&self) -> Duration {
        LISTING_REUSE
    }

    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        let kept = self
            .version
            .get(&self.config.executable_path.get())
            .is_some();
        let version = self.cli_version().await?;
        let mut connection = match self.initialized_connection().await {
            Ok(connection) => connection,
            Err(error) => {
                // A kept version says nothing about whether the executable
                // still starts. Ask again, so this fails exactly as an
                // uncached probe would have: with the version probe's error if
                // that fails too, otherwise with the start error.
                if kept {
                    self.version.forget();
                    self.cli_version().await?;
                }
                return Err(error);
            }
        };
        Self::drain_control_requests(&mut connection, "probe", true);
        let models_response = connection
            .transport
            .request(CodexMethod::MODEL_LIST, &json!({}))
            .await
            .into_result()
            .ok();
        let models = models_response.as_ref().map_or_else(Vec::new, |response| {
            let (models, catalogue) = model_catalog(response);
            *lock(&self.fast_tiers) = catalogue;
            models
        });
        let account = connection
            .transport
            .request(CodexMethod::ACCOUNT_READ, &json!({}))
            .await
            .into_result()
            .ok();
        let login_identity = account.as_ref().and_then(|account| {
            value_text(account, &["account", "email"])
                .or_else(|| value_text(account, &["account", "type"]))
                .map(ToOwned::to_owned)
        });
        let auth = account.as_ref().and_then(codex_auth);
        let effort_levels = models.iter().fold(Vec::new(), |mut output, model| {
            for level in &model.effort_levels {
                if !output.contains(level) {
                    output.push(level.clone());
                }
            }
            output
        });
        let capabilities = EngineCapabilities {
            streaming: true,
            approvals: true,
            resume: true,
            fork: true,
            cloud_sessions: false,
            models: if models.is_empty() {
                EngineModels::Dynamic("dynamic".into())
            } else {
                EngineModels::Models(models.iter().map(|model| model.value.clone()).collect())
            },
            effort_levels,
            permission_modes: PERMISSION_MODES.map(str::to_owned).into(),
            consultation_safety: Some(ConsultationSafetyProfile::Available {
                available: true,
                enforcement: ConsultationSafetyEnforcement::ReadOnly,
                launch_options: interface_contracts::engine::ConsultationLaunchOptions {
                    permission_mode: Some("on-request".into()),
                    sandbox_mode: Some(SandboxMode::ReadOnly),
                    setting_sources: None,
                    disallowed_tools: None,
                },
            }),
            steer: Some(true),
        };
        Ok(EngineInfo {
            auth,
            binary_path: self
                .config
                .executable_path
                .get()
                .to_string_lossy()
                .into_owned(),
            version,
            login_identity,
            capabilities,
            display_name: None,
            model_catalog: (!models.is_empty()).then_some(models),
        })
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        let mut slot = self.listing_connection.lock().await;
        let connection = match self.checkout_lane(&mut slot, "session listing").await {
            Ok(connection) => connection,
            // No Codex on this Mac: there is nothing to list. That is an
            // answer, like a provider without a store listing, and must not
            // be reported as a refresh that broke. A Codex that is there and
            // will not start stays a failure.
            Err(_) if !self.config.executable_path.get().is_file() => {
                return Ok(EngineSessionListing {
                    sessions: Vec::new(),
                    complete: false,
                    unavailable: Some(true),
                    error: Some("Codex is not installed.".into()),
                });
            }
            Err(error) => return Err(error),
        };

        let mut sessions = Vec::new();
        let mut session_positions = HashMap::new();
        let mut complete = true;
        let mut out_of_time = false;
        let started = Instant::now();
        'listing: for archived in [false, true] {
            let mut cursor: Option<String> = None;
            let mut seen = HashSet::new();
            for _ in 0..LIST_MAX_PAGES {
                if (archived || cursor.is_some()) && started.elapsed() >= LIST_TIME_BUDGET {
                    complete = false;
                    out_of_time = true;
                    break 'listing;
                }
                let mut params = Map::new();
                params.insert("limit".into(), Value::from(LIST_PAGE_SIZE));
                if archived {
                    params.insert("archived".into(), Value::Bool(true));
                }
                if let Some(cursor) = &cursor {
                    params.insert("cursor".into(), Value::String(cursor.clone()));
                }
                let page = match connection
                    .transport
                    .request(CodexMethod::THREAD_LIST, &Value::Object(params))
                    .await
                    .into_result()
                {
                    Ok(page) => page,
                    Err(error) if archived => {
                        complete = false;
                        let _ = error;
                        break;
                    }
                    Err(error) => return Err(error),
                };
                for session in parse_thread_page(&self.id, &page, archived) {
                    if let Some(position) = session_positions.get(&session.session_id).copied() {
                        sessions[position] = session;
                    } else {
                        session_positions.insert(session.session_id.clone(), sessions.len());
                        sessions.push(session);
                    }
                }
                let next = page
                    .get("nextCursor")
                    .and_then(Value::as_str)
                    .filter(|next| !next.is_empty())
                    .map(ToOwned::to_owned);
                let Some(next) = next else {
                    break;
                };
                if !seen.insert(next.clone()) {
                    complete = false;
                    break;
                }
                cursor = Some(next);
            }
            if cursor.is_some() && seen.len() == LIST_MAX_PAGES {
                complete = false;
            }
        }
        if (complete || out_of_time) && !connection.transport.is_closed() {
            *slot = Some(connection);
        }
        Ok(EngineSessionListing {
            sessions,
            complete,
            unavailable: None,
            error: (!complete).then(|| {
                if out_of_time {
                    "Codex was slow to list sessions; the newest were refreshed and older \
                     ones keep their cached entries."
                        .into()
                } else {
                    "The archived-session portion of the Codex listing was incomplete.".into()
                }
            }),
        })
    }

    fn supported_ops(&self) -> Vec<EngineOp> {
        vec![
            EngineOp::RenameSession,
            EngineOp::DeleteSession,
            EngineOp::ForkSession,
            EngineOp::SetArchived,
            EngineOp::ReadUsage,
        ]
    }

    fn supports_history(&self) -> bool {
        true
    }

    async fn rename_session(
        &self,
        session_id: String,
        title: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control_request(
            CodexMethod::THREAD_SET_NAME,
            Some(json!({"threadId": session_id, "name": title})),
            "session rename",
        )
        .await
        .map(|_| ())
    }

    async fn delete_session(
        &self,
        session_id: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control_request(
            CodexMethod::THREAD_DELETE,
            Some(json!({"threadId": session_id})),
            "session deletion",
        )
        .await?;
        let live = lock(&self.sessions)
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|session| *lock(&session.session_id) == session_id)
            .collect::<Vec<_>>();
        for session in live {
            close_shared(&session).await;
        }
        Ok(())
    }

    async fn fork_session(
        &self,
        session_id: String,
        cwd: Option<String>,
    ) -> Result<String, EngineError> {
        self.fork_thread(session_id, cwd, None).await
    }

    /// `thread/fork` through the turn `boundary` names (`lastTurnId` is
    /// inclusive). The turn list is the same `thread/read` the stored
    /// history comes from; any failure before `thread/fork` is sent is
    /// definitely-unsent.
    async fn fork_session_at(
        &self,
        session_id: String,
        cwd: Option<String>,
        boundary: ForkBoundary,
    ) -> Result<String, EngineError> {
        let response = self
            .history_read_request(session_id.clone())
            .await
            .map_err(|error| {
                EngineError::definitely_unsent(
                    error.kind,
                    format!(
                        "Codex could not read this conversation's turns, so nothing was forked: {}",
                        error.message
                    ),
                    error.retryable,
                )
            })?;
        let turns = response
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let last_turn_id = fork_turn_boundary(turns, &boundary).map_err(|reason| {
            EngineError::definitely_unsent(EngineErrorKind::InvalidInput, reason, false)
        })?;
        self.fork_thread(session_id, cwd, Some(last_turn_id)).await
    }

    async fn set_archived(
        &self,
        session_id: String,
        archived: bool,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        self.control_request(
            if archived {
                CodexMethod::THREAD_ARCHIVE
            } else {
                CodexMethod::THREAD_UNARCHIVE
            },
            Some(json!({"threadId": session_id})),
            "provider archive update",
        )
        .await
        .map(|_| ())
    }

    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        let lane = match self.read_lane("account usage read").await {
            Ok(lane) => lane,
            Err(error) => return Ok(unavailable_usage(&self.id, &error.message)),
        };
        let limits = match lane
            .connection
            .transport
            .request_without_params(CodexMethod::ACCOUNT_RATE_LIMITS_READ)
            .await
            .into_result()
        {
            Ok(limits) => limits,
            Err(error) => {
                lane.finish();
                return Ok(unavailable_usage(&self.id, &error.message));
            }
        };
        let token_usage = lane
            .connection
            .transport
            .request_without_params(CodexMethod::ACCOUNT_USAGE_READ)
            .await
            .into_result()
            .ok();
        lane.finish();
        Ok(map_engine_usage(&limits, token_usage.as_ref()))
    }

    async fn list_provider_commands(
        &self,
        scope: ProviderCommandRequest,
    ) -> Result<ProviderCommandResult, EngineError> {
        if scope.setting_sources.is_some() {
            return Ok(ProviderCommandResult::unavailable(
                "Codex does not use Claude setting-source scopes.",
            ));
        }
        let params = if scope.cwd.is_empty() {
            json!({})
        } else {
            json!({"cwds": [scope.cwd]})
        };
        let response = match self.read_lane("command discovery").await {
            Ok(lane) => {
                let response = lane
                    .connection
                    .transport
                    .request(CodexMethod::SKILLS_LIST, &params)
                    .await
                    .into_result();
                lane.finish();
                response
            }
            Err(error) => Err(error),
        };
        let Ok(response) = response else {
            return Ok(ProviderCommandResult::unavailable(
                "Interface could not read commands from Codex for this project.",
            ));
        };
        let Some(skills) = skill_command_candidates(&response, &scope.cwd) else {
            return Ok(ProviderCommandResult::unavailable(
                "Codex could not completely read skills for this project.",
            ));
        };
        let mut candidates = codex_builtin_command_candidates();
        candidates.extend(skills);
        Ok(ProviderCommandResult {
            available: true,
            commands: normalize_provider_commands(&candidates),
            reason: None,
        })
    }

    async fn read_history(
        &self,
        session_id: String,
        request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        let response = self.history_read_request(session_id).await?;
        let turns = response
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        Ok(stored_history_page(turns, request.limit))
    }

    fn create_session(
        &self,
        options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        validate_launch(&options)?;
        // A prompt no frame can carry is refused here, before thread/start
        // creates a provider thread its turn/start could never reach.
        if !text_fits_frame(&options.prompt, self.config.transport.max_frame_bytes) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "Codex launch prompt must be bounded",
                false,
            ));
        }
        self.make_handle(Opening::Create { options })
    }

    fn resume_session(
        &self,
        session_id: String,
        cwd: String,
        options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        validate_resume(&session_id, &cwd, &options)?;
        self.make_handle(Opening::Resume {
            session_id,
            cwd,
            options,
        })
    }

    fn approval_facts(&self, tool: &str, input: &Value) -> ApprovalFacts {
        codex_approval_facts(tool, input)
    }

    async fn dispose(&self) -> Result<(), EngineError> {
        self.collaboration
            .adapter_disposed
            .store(true, Ordering::Release);
        self.listing_connection.lock().await.take();
        for lane in self.history_connections.iter() {
            lane.slot.lock().await.take();
        }
        let sessions = std::mem::take(&mut *lock(&self.sessions));
        for session in sessions.into_iter().filter_map(|session| session.upgrade()) {
            close_shared(&session).await;
        }
        Ok(())
    }
}

fn safe_line(value: &str, maximum: usize) -> String {
    let line = collapse_whitespace(value);
    if line.len() <= maximum {
        return line;
    }
    let mut end = maximum.saturating_sub('…'.len_utf8());
    while !line.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", line[..end].trim_end())
}

fn map_text<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn unavailable_usage(engine: &EngineId, message: &str) -> EngineUsage {
    EngineUsage {
        engine: engine.clone(),
        available: false,
        plan: None,
        windows: Vec::new(),
        cost_usd: None,
        note: Some(format!("Codex could not report plan limits: {message}")),
        account: None,
        account_label: None,
    }
}

fn parse_thread_page(engine: &EngineId, page: &Value, archived: bool) -> Vec<SessionSummary> {
    let now = now_iso();
    page.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|thread| {
            let session_id = thread.get("id")?.as_str()?.to_owned();
            let status = match thread
                .get("status")
                .and_then(Value::as_object)
                .and_then(|status| status.get("type"))
                .and_then(Value::as_str)
            {
                Some("active") => SessionStatus::Working,
                Some("systemError") => SessionStatus::Failed,
                _ => SessionStatus::Idle,
            };
            // The first non-empty string of name, then preview (0.1.169's
            // threadTitle): Codex sends `"name": null` for unnamed threads.
            // A name is Codex's own title (`titleFromSummary`); the preview
            // is the first prompt, as Interface's launch title is.
            let named = thread
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.is_empty());
            let title = ["name", "preview"]
                .into_iter()
                .find_map(|field| {
                    thread
                        .get(field)
                        .and_then(Value::as_str)
                        .filter(|title| !title.is_empty())
                })
                .map_or_else(
                    || "Codex session".into(),
                    |title| clip_label(&collapse_whitespace(title), 120.0),
                );
            let created = thread
                .get("createdAt")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
                .map_or_else(|| now.clone(), unix_timestamp);
            let updated = thread
                .get("updatedAt")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
                .map_or_else(|| created.clone(), unix_timestamp);
            Some(SessionSummary {
                engine: engine.clone(),
                session_id,
                title,
                cwd: thread
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                status,
                cloud: false,
                model: None,
                effort: None,
                last_activity: updated,
                first_seen: created,
                archived,
                size: None,
                title_from_summary: named.then_some(true),
                origin: None,
                project_root: None,
                worktree_home: None,
                worktree_path: None,
                setting_sources: None,
                disallowed_tools: None,
                kind: None,
                permission_mode: None,
            })
        })
        .collect()
}

/// Codex's stored form always prints a zero sub-second field.
pub(crate) fn unix_timestamp(seconds: i64) -> String {
    unix_seconds_iso(seconds, IsoFraction::Zeros)
}

fn now_iso() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        });
    unix_timestamp(seconds)
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
