use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::agents::ApprovalFacts;
use interface_contracts::engine::{
    CollaborationMcpSessionBinding, CollaborationMcpSessionFactory, CollaborationMcpSessionRequest,
    ConsultationSafetyEnforcement, ConsultationSafetyProfile, EngineCapabilities, EngineErrorCause,
    EngineEvent, EngineEventKind, EngineId, EngineInfo, EngineModels, EngineSessionListing,
    EngineUsage, LaunchOptions, ModelOption, NoticeTone, ResumeOptions, SessionKind, SessionStatus,
    collaboration_source_session_key, normalize_collaboration_depth,
};
use interface_contracts::ipc::{EngineOp, HistoryPage};
use interface_contracts::provider_commands::app_owned_provider_command_error;
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EngineAdapter,
    EngineError, EngineErrorKind, ExecutablePath, HistoryRequest, ProcessSpec,
    ProviderCommandRequest, ProviderCommandResult, ReadySession, SessionHandle, VersionCache,
    blocking, bounded_event_stream_with_budget, cli_version,
    jsonrpc::{RpcId, RpcRemoteError, text_fits_frame},
    session_core::{ReadyState, SessionCore, validate_cwd, value_text},
    shell_words::shell_word,
    supervised::InboundReceiver,
};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::{
    grok_store,
    mapper::AcpEventMapper,
    profile::{
        AcpProfileError, BuiltinAcpProvider, ValidatedAcpProfile, acp_process_spec,
        builtin_profile, builtin_search_path,
    },
    protocol::{
        AcpContentBlock, AcpInbound, AcpMcpServer, AcpMethod, AcpPhase, AcpSessionState,
        InitializeParams, PermissionOption, PromptMetadata, SessionCancelParams, SessionMetadata,
        SessionNewParams, SessionOpenParams, SessionPromptParams, permission_response,
        resume_method_for, selected_protocol_version_is_supported,
    },
    qwen::{
        QWEN_MODEL_ID, QwenBackendError, QwenBackendLifecycle, QwenBackendRuntime,
        QwenBackendRuntimeOptions, qwen_process_environment,
    },
    transport::{AcpConnection, AcpTransport, AcpTransportEvent, AcpTransportLimits},
};

const DEFAULT_EVENT_CAPACITY: usize = 5_000;
const MAX_PROMPT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_PERMISSIONS: usize = 128;
const MAX_PRE_READY_UPDATES: usize = 100;
const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const PROBE_VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const GROK_PERMISSION_MODES: [&str; 3] = ["default", "auto", "bypassPermissions"];
const GROK_USAGE_UNAVAILABLE: &str = "Interface could not read Grok subscription usage.";
/// Probe P3 found no way to force Grok Build (or a custom agent) to ask over the
/// owner's own always-approve configuration, so agent launches are refused.
const ACP_APPROVAL_GATE_REFUSAL: &str =
    "This ACP agent cannot enforce Interface's approval gate yet, so it can't run as an agent.";
/// `rawInput` keys that name the files an ACP tool touches.
const RAW_INPUT_PATH_KEYS: [&str; 3] = ["path", "file_path", "abs_path"];
const ACP_CHAT_REFUSAL: &str = "Grok, Qwen and custom ACP agents cannot run a tool-less chat: ACP has no way to remove an agent's tools. Choose a Claude model for Chat.";

static NEXT_PROVISIONAL_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_APPROVAL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpProviderFlavor {
    Grok,
    Qwen,
    Custom,
}

#[derive(Clone, Debug)]
pub struct AcpAdapterConfig {
    pub profile: ValidatedAcpProfile,
    pub flavor: AcpProviderFlavor,
    /// Sent as `clientInfo.version`. Defaults to this crate's version; the
    /// binaries set their release version here at runtime, so this crate never
    /// compiles a release version in.
    pub client_version: String,
    pub transport: AcpTransportLimits,
    pub prompt_timeout: Duration,
    pub event_capacity: usize,
    /// Used only by the built-in Grok flavor; ACP v1 itself has no store API.
    pub grok_sessions_root: PathBuf,
    /// Cached login-shell path to `qwen-mlx-server` for the Qwen backend.
    /// Hosts should pass the process-wide lookup; this crate never spawns a login shell.
    pub login_shell_path: Option<PathBuf>,
    /// What every launch runs, starting at `profile.executable_path` and
    /// shared with the host, which repoints a built-in provider's CLI when a
    /// refresh finds it elsewhere. A custom profile's path is the owner's own
    /// and is never repointed.
    pub executable: ExecutablePath,
    /// Set on every child after Interface's own entries: an added provider
    /// account's `GROK_HOME` (pair it with `grok_sessions_root` at that
    /// directory's `sessions`). Empty for the Default account.
    pub environment: Vec<(OsString, OsString)>,
}

impl AcpAdapterConfig {
    #[must_use]
    pub fn custom(profile: ValidatedAcpProfile) -> Self {
        Self {
            executable: ExecutablePath::new(profile.executable_path.clone()),
            profile,
            flavor: AcpProviderFlavor::Custom,
            client_version: env!("CARGO_PKG_VERSION").into(),
            transport: AcpTransportLimits::default(),
            prompt_timeout: DEFAULT_PROMPT_TIMEOUT,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            grok_sessions_root: grok_store::default_sessions_root(),
            login_shell_path: None,
            environment: Vec::new(),
        }
    }

    /// Construct the reserved Grok or Qwen profile with exact no-shell argv.
    ///
    /// # Errors
    ///
    /// Returns a profile error for an unsafe executable path.
    pub fn builtin(
        provider: BuiltinAcpProvider,
        executable_path: PathBuf,
    ) -> Result<Self, AcpProfileError> {
        let mut config = Self::custom(builtin_profile(provider, executable_path)?);
        config.flavor = match provider {
            BuiltinAcpProvider::Grok => AcpProviderFlavor::Grok,
            BuiltinAcpProvider::Qwen => AcpProviderFlavor::Qwen,
        };
        Ok(config)
    }
}

pub struct AcpAdapter {
    id: EngineId,
    config: AcpAdapterConfig,
    sessions: Mutex<Vec<Weak<SessionShared>>>,
    collaboration_mcp_factory: Option<Arc<dyn CollaborationMcpSessionFactory>>,
    /// The host's team tools factory (`iface_team`, protocol 57). Every ACP
    /// flavor takes stdio MCP servers in `session/new` and `session/load`.
    team_tools: Option<Arc<dyn interface_contracts::engine::TeamToolsFactory>>,
    active_collaboration_bindings:
        Arc<Mutex<HashMap<String, Arc<dyn CollaborationMcpSessionBinding>>>>,
    qwen_backend: Option<Arc<dyn QwenBackendLifecycle>>,
    /// Effort values reported by the last successful provider probe. Portable
    /// ACP processes have no standard effort control, so this catalogue is the
    /// only evidence that a configured process understands a requested level.
    probed_effort_levels: Arc<Mutex<Vec<String>>>,
    /// Parsed Grok `summary.json` fields keyed by file identity, so a focus
    /// refresh re-reads only sessions whose summary changed.
    grok_listing: Arc<Mutex<grok_store::GrokListingCache>>,
    /// The last `grok --version` answer, kept while the executable path
    /// resolves to the same file (symlinks followed).
    grok_version: VersionCache,
    disposed: Arc<AtomicBool>,
}

/// Shuts a one-shot connection's agent down however its caller ends. The
/// handshake's drain task owns an `Arc` of the transport, which keeps the
/// agent's stdin open, so a caller dropped between the handshake and its
/// explicit `shutdown()` (a runtime timeout on a slow billing read) would
/// otherwise leave the agent running until the app exits. `shutdown()` is
/// idempotent, so the explicit calls stay.
struct ShutdownOnDrop(Arc<AcpTransport>);

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

impl AcpAdapter {
    /// Construct a provider adapter without invoking the configured process.
    ///
    /// # Errors
    ///
    /// Returns a local validation error for inconsistent built-in identity or
    /// non-positive runtime bounds.
    pub fn new(config: AcpAdapterConfig) -> Result<Self, EngineError> {
        let expected_slug = match config.flavor {
            AcpProviderFlavor::Grok => Some("grok"),
            AcpProviderFlavor::Qwen => Some("qwen"),
            AcpProviderFlavor::Custom => None,
        };
        if expected_slug.is_some_and(|slug| slug != config.profile.slug) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "ACP built-in flavor does not match the validated profile slug",
                false,
            ));
        }
        if config.event_capacity == 0 || config.prompt_timeout.is_zero() {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "ACP event capacity and prompt timeout must be positive",
                false,
            ));
        }
        let qwen_backend = (config.flavor == AcpProviderFlavor::Qwen).then(|| {
            Arc::new(QwenBackendRuntime::new(QwenBackendRuntimeOptions {
                login_shell_path: config.login_shell_path.clone(),
                ..QwenBackendRuntimeOptions::default()
            })) as Arc<dyn QwenBackendLifecycle>
        });
        Ok(Self {
            id: config.profile.engine_id.clone(),
            config,
            sessions: Mutex::new(Vec::new()),
            collaboration_mcp_factory: None,
            team_tools: None,
            active_collaboration_bindings: Arc::new(Mutex::new(HashMap::new())),
            qwen_backend,
            probed_effort_levels: Arc::new(Mutex::new(Vec::new())),
            grok_listing: Arc::new(Mutex::new(grok_store::GrokListingCache::default())),
            grok_version: VersionCache::default(),
            disposed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Install the host-owned collaboration factory for the built-in Grok
    /// adapter. Portable Qwen and custom ACP profiles never receive it.
    ///
    /// # Errors
    ///
    /// Refuses collaboration injection for any flavor other than Grok.
    pub fn with_collaboration_factory(
        mut self,
        factory: Arc<dyn CollaborationMcpSessionFactory>,
    ) -> Result<Self, EngineError> {
        if self.config.flavor != AcpProviderFlavor::Grok {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "collaboration MCP is reserved for the built-in Grok adapter",
                false,
            ));
        }
        self.collaboration_mcp_factory = Some(factory);
        Ok(self)
    }

    /// Installs the host's team tools factory: an ordinary code session (one
    /// hop up) asks it for its `iface_team` server, on every flavor.
    #[must_use]
    pub fn with_team_tools(
        mut self,
        factory: Arc<dyn interface_contracts::engine::TeamToolsFactory>,
    ) -> Self {
        self.team_tools = Some(factory);
        self
    }

    /// Replace the built-in Qwen backend owner, primarily for deterministic
    /// host tests or an explicitly preconfigured local wrapper.
    ///
    /// # Errors
    ///
    /// Refuses the lifecycle on Grok and custom ACP profiles.
    pub fn with_qwen_backend_runtime(
        mut self,
        runtime: Arc<dyn QwenBackendLifecycle>,
    ) -> Result<Self, EngineError> {
        if self.config.flavor != AcpProviderFlavor::Qwen {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "Qwen backend lifecycle is reserved for the built-in Qwen adapter",
                false,
            ));
        }
        self.qwen_backend = Some(runtime);
        Ok(self)
    }

    fn connect(
        &self,
        cwd: &Path,
        session: Option<&SessionConfig>,
    ) -> Result<AcpConnection, EngineError> {
        let spec = process_spec(&self.config, cwd, session)?;
        spawn_connection(&self.config, &spec, self.qwen_backend.as_ref())
    }

    fn connect_grok_usage(&self, cwd: &Path) -> Result<AcpConnection, EngineError> {
        let mut spec = process_spec(&self.config, cwd, None)?;
        insert_before_stdio(&mut spec, vec![OsString::from("--no-leader")]);
        spawn_connection(&self.config, &spec, None)
    }

    /// `--version` cannot change while the path still resolves to the same
    /// binary (the CLI runs with `--no-auto-update`), so a probe spawns it only
    /// when that file changed.
    async fn grok_version(&self) -> Result<String, EngineError> {
        let executable = &self.config.executable.get();
        self.grok_version
            .get_or_probe(
                executable,
                cli_version(
                    executable,
                    "Grok",
                    "`grok --version` returned no version string",
                    PROBE_VERSION_TIMEOUT,
                ),
            )
            .await
    }

    /// Every error path shuts the child down, so a caller that gives up on the
    /// handshake never has to unwind the transport itself.
    async fn handshake(
        &self,
        connection: &mut AcpConnection,
        operation: &'static str,
        require_supported_version: bool,
    ) -> Result<Value, EngineError> {
        let initialized = match connection
            .transport
            .request(
                AcpMethod::INITIALIZE,
                &InitializeParams::interface(&self.config.client_version),
            )
            .await
            .into_result()
        {
            Ok(initialized) => initialized,
            Err(error) => {
                connection.transport.shutdown();
                return Err(error);
            }
        };
        if require_supported_version && !selected_protocol_version_is_supported(&initialized) {
            connection.transport.shutdown();
            return Err(EngineError::definitely_rejected(
                EngineErrorKind::Protocol,
                "ACP agent selected an unsupported protocol version",
                false,
            ));
        }
        Self::drain_control_requests(connection, operation);
        Ok(initialized)
    }

    /// The spawned drain task holds the transport, so a one-shot connection
    /// whose caller is dropped mid-request needs [`ShutdownOnDrop`].
    fn drain_control_requests(connection: &mut AcpConnection, operation: &'static str) {
        let transport = Arc::clone(&connection.transport);
        let mut inbound = std::mem::replace(&mut connection.inbound, InboundReceiver::closed());
        tokio::spawn(async move {
            while let Some(event) = inbound.recv().await {
                if let AcpTransportEvent::Inbound(
                    AcpInbound::PermissionRequest { id, .. }
                    | AcpInbound::ServerRequest(interface_engine::jsonrpc::RpcServerRequest {
                        id,
                        ..
                    }),
                ) = event
                {
                    let _ = transport
                        .server_error(
                            &id,
                            &RpcRemoteError {
                                code: -32_601,
                                message: format!(
                                    "client request unavailable during ACP {operation}"
                                ),
                                data: None,
                            },
                        )
                        .await;
                }
            }
        });
    }

    fn make_handle(&self, opening: Opening) -> Result<Box<dyn SessionHandle>, EngineError> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "ACP adapter is disposed",
                false,
            ));
        }
        tokio::runtime::Handle::try_current().map_err(|_| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "ACP sessions require a running Tokio runtime",
                false,
            )
        })?;
        let (events_tx, events_rx) = bounded_event_stream_with_budget(
            self.config.event_capacity,
            interface_engine::event_budget::PROVIDER_EVENT_BYTES,
        );
        let provisional_id = match &opening {
            Opening::Create { .. } => format!(
                "pending-acp-{}",
                NEXT_PROVISIONAL_ID.fetch_add(1, Ordering::Relaxed)
            ),
            Opening::Resume { session_id, .. } => session_id.clone(),
        };
        // The permission rule is enforced once, by the caller: create_session
        // runs validate_launch and resume_session pins "default".
        let session_config = session_config(&opening);
        validate_config(&session_config)?;
        let collaboration_depth = opening_collaboration_depth(&opening)?;
        let (core, ready_rx) = SessionCore::new(events_tx);
        let shared = Arc::new(SessionShared {
            core,
            engine_id: self.id.clone(),
            flavor: self.config.flavor,
            session_id: Mutex::new(provisional_id),
            protocol: Mutex::new(AcpSessionState::default()),
            config: Mutex::new(session_config),
            pending_permissions: Mutex::new(HashMap::new()),
            pre_ready_updates: Mutex::new(Vec::new()),
            mapper: Mutex::new(AcpEventMapper::default()),
            prompt_timeout: self.config.prompt_timeout,
            collaboration_depth,
            collaboration_mcp_factory: (self.config.flavor == AcpProviderFlavor::Grok)
                .then(|| self.collaboration_mcp_factory.clone())
                .flatten(),
            collaboration_binding: Mutex::new(None),
            team_tools: self.team_tools.clone().filter(|_| {
                interface_contracts::engine::team_tools_eligible(
                    interface_contracts::engine::session_kind_of(opening_kind(&opening)),
                    false,
                    false,
                    collaboration_depth,
                )
            }),
            team_binding: Mutex::new(None),
            active_collaboration_bindings: Arc::clone(&self.active_collaboration_bindings),
            qwen_backend: self.qwen_backend.clone(),
            probed_effort_levels: Arc::clone(&self.probed_effort_levels),
            adapter_disposed: Arc::clone(&self.disposed),
            max_frame_bytes: self.config.transport.max_frame_bytes,
        });
        let handle = AcpSessionHandle {
            shared: Arc::clone(&shared),
            ready: ready_rx,
            events: Some(events_rx),
        };
        {
            let mut sessions = lock(&self.sessions);
            sessions.retain(|session| session.strong_count() > 0);
            sessions.push(Arc::downgrade(&shared));
        }
        let config = self.config.clone();
        tokio::spawn(async move { open_session(shared, opening, config).await });
        Ok(Box::new(handle))
    }
}

#[async_trait]
impl EngineAdapter for AcpAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }

    fn executable(&self) -> Option<PathBuf> {
        Some(self.config.executable.get())
    }

    #[allow(clippy::too_many_lines)]
    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        if let Some(qwen_backend) = &self.qwen_backend {
            qwen_backend
                .prepare_probe()
                .await
                .map_err(|error| qwen_backend_error(&error))?;
        }
        let grok_version = if self.config.flavor == AcpProviderFlavor::Grok {
            Some(self.grok_version().await?)
        } else {
            None
        };
        let cwd = std::env::current_dir().map_err(|error| {
            EngineError::definitely_unsent(
                EngineErrorKind::Unavailable,
                format!("could not determine ACP probe cwd: {error}"),
                true,
            )
        })?;
        let mut connection = self.connect(&cwd, None)?;
        let _stop = ShutdownOnDrop(Arc::clone(&connection.transport));
        let initialized = self.handshake(&mut connection, "probe", true).await?;
        let capabilities_value = initialized.get("agentCapabilities");
        // Grok's load behavior is a verified built-in extension even when an
        // older handshake omits the portable ACP capability bit.
        let can_resume = self.config.flavor == AcpProviderFlavor::Grok
            || capabilities_value.is_some_and(|caps| resume_method_for(caps).is_some());
        let mut models = model_catalog(&initialized, self.config.flavor);
        if self.config.flavor == AcpProviderFlavor::Grok && models.is_empty() {
            models = connection
                .transport
                .request("x.ai/models/list", &json!({}))
                .await
                .into_result()
                .map_or_else(
                    |_| Vec::new(),
                    |payload| model_catalog(&payload, self.config.flavor),
                );
        }
        let advertised_effort_levels = effort_levels(&models);
        lock(&self.probed_effort_levels).clone_from(&advertised_effort_levels);
        let permission_modes = if self.config.flavor == AcpProviderFlavor::Grok {
            GROK_PERMISSION_MODES.map(str::to_owned).into()
        } else {
            vec!["default".into()]
        };
        let consultation_safety = if self.config.flavor == AcpProviderFlavor::Grok {
            ConsultationSafetyProfile::Available {
                available: true,
                enforcement: ConsultationSafetyEnforcement::ApprovalGated,
                launch_options: interface_contracts::engine::ConsultationLaunchOptions {
                    permission_mode: Some("default".into()),
                    sandbox_mode: None,
                    setting_sources: None,
                    disallowed_tools: None,
                },
            }
        } else {
            ConsultationSafetyProfile::Unavailable {
                available: false,
                reason: format!(
                    "{} ACP does not advertise an enforceable consultation profile.",
                    self.config.profile.display_name
                ),
            }
        };
        let version = grok_version.unwrap_or_else(|| {
            value_text(&initialized, &["agentInfo", "version"])
                .unwrap_or("ACP v1")
                .to_owned()
        });
        connection.transport.shutdown();
        Ok(EngineInfo {
            auth: None,
            binary_path: self.config.executable.get().to_string_lossy().into_owned(),
            version,
            login_identity: None,
            capabilities: EngineCapabilities {
                streaming: true,
                approvals: true,
                resume: can_resume,
                fork: false,
                cloud_sessions: false,
                models: if models.is_empty() {
                    EngineModels::Dynamic("dynamic".into())
                } else {
                    EngineModels::Models(models.iter().map(|model| model.value.clone()).collect())
                },
                effort_levels: advertised_effort_levels,
                permission_modes,
                consultation_safety: Some(consultation_safety),
                steer: None,
            },
            display_name: (self.config.flavor != AcpProviderFlavor::Grok)
                .then(|| self.config.profile.display_name.clone()),
            model_catalog: (!models.is_empty()).then_some(models),
        })
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        if self.config.flavor == AcpProviderFlavor::Grok {
            let root = self.config.grok_sessions_root.clone();
            let engine = self.id.clone();
            let cache = Arc::clone(&self.grok_listing);
            let listed = run_grok_store(false, move || {
                grok_store::list_sessions_cached(&root, &engine, &mut lock(&cache))
            })
            .await?;
            return match listed {
                Ok(sessions) => Ok(EngineSessionListing {
                    sessions,
                    complete: true,
                    unavailable: None,
                    error: None,
                }),
                Err(_) => Ok(EngineSessionListing {
                    sessions: Vec::new(),
                    complete: false,
                    unavailable: None,
                    error: Some("Grok could not read stored sessions.".into()),
                }),
            };
        }
        Ok(EngineSessionListing {
            sessions: Vec::new(),
            complete: false,
            unavailable: Some(true),
            error: Some(format!(
                "{} ACP does not expose a stored-session listing.",
                self.config.profile.display_name
            )),
        })
    }

    fn supported_ops(&self) -> Vec<EngineOp> {
        if self.config.flavor == AcpProviderFlavor::Grok {
            vec![EngineOp::DeleteSession, EngineOp::ReadUsage]
        } else {
            Vec::new()
        }
    }

    fn supports_history(&self) -> bool {
        self.config.flavor == AcpProviderFlavor::Grok
    }

    async fn delete_session(
        &self,
        session_id: String,
        cwd: Option<String>,
    ) -> Result<(), EngineError> {
        if self.config.flavor != AcpProviderFlavor::Grok {
            return Err(local_error(
                EngineErrorKind::Unsupported,
                "portable ACP v1 does not expose stored-session deletion",
            ));
        }
        let root = self.config.grok_sessions_root.clone();
        let cwd = cwd.unwrap_or_default();
        run_grok_store(true, move || grok_store::delete(&root, &session_id, &cwd))
            .await?
            .map_err(|error| grok_store_error(&error))
    }

    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        if self.config.flavor != AcpProviderFlavor::Grok {
            return Err(local_error(
                EngineErrorKind::Unsupported,
                "portable ACP v1 does not expose account usage",
            ));
        }
        let Ok(cwd) = std::env::current_dir() else {
            return Ok(grok_store::unavailable_usage(GROK_USAGE_UNAVAILABLE));
        };
        let Ok(mut connection) = self.connect_grok_usage(&cwd) else {
            return Ok(grok_store::unavailable_usage(GROK_USAGE_UNAVAILABLE));
        };
        let _stop = ShutdownOnDrop(Arc::clone(&connection.transport));
        if self
            .handshake(&mut connection, "billing read", false)
            .await
            .is_err()
        {
            return Ok(grok_store::unavailable_usage(GROK_USAGE_UNAVAILABLE));
        }
        let billing = connection
            .transport
            .request("_x.ai/billing", &json!({}))
            .await
            .into_result();
        connection.transport.shutdown();
        Ok(billing.map_or_else(
            |_| grok_store::unavailable_usage(GROK_USAGE_UNAVAILABLE),
            |payload| grok_store::billing_usage(&payload, grok_store::current_time_ms()),
        ))
    }

    async fn list_provider_commands(
        &self,
        _scope: ProviderCommandRequest,
    ) -> Result<ProviderCommandResult, EngineError> {
        Ok(ProviderCommandResult::unavailable(
            if self.config.flavor == AcpProviderFlavor::Grok {
                "Grok exposes project commands only after a chat session starts. Interface did not create a hidden chat for autocomplete.".into()
            } else {
                format!(
                    "{} ACP exposes commands only inside a chat session. Interface did not create a hidden session for autocomplete.",
                    self.config.profile.display_name
                )
            },
        ))
    }

    async fn read_history(
        &self,
        session_id: String,
        request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        if self.config.flavor == AcpProviderFlavor::Grok {
            let root = self.config.grok_sessions_root.clone();
            run_grok_store(false, move || {
                grok_store::history(&root, &session_id, &request.cwd, request.limit)
            })
            .await
        } else {
            Ok(interface_contracts::ipc::unavailable_history(format!(
                "{} ACP does not expose stored transcript replay.",
                self.config.profile.display_name
            )))
        }
    }

    fn create_session(
        &self,
        options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        let allowed_efforts = lock(&self.probed_effort_levels).clone();
        validate_launch(self.config.flavor, &options, &allowed_efforts)?;
        // A prompt no frame can carry is refused here, before session/new
        // creates a provider session its prompt could never reach.
        if !text_fits_frame(&options.prompt, self.config.transport.max_frame_bytes) {
            return Err(local_error(
                EngineErrorKind::InvalidInput,
                "ACP launch prompt must be non-empty and bounded",
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
        let allowed_efforts = lock(&self.probed_effort_levels).clone();
        validate_resume(
            self.config.flavor,
            &session_id,
            &cwd,
            &options,
            &allowed_efforts,
        )?;
        self.make_handle(Opening::Resume {
            session_id,
            cwd,
            options,
        })
    }

    fn approval_facts(&self, tool: &str, input: &Value) -> ApprovalFacts {
        acp_approval_facts(tool, input)
    }

    async fn dispose(&self) -> Result<(), EngineError> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let sessions = std::mem::take(&mut *lock(&self.sessions));
        for session in sessions.into_iter().filter_map(|session| session.upgrade()) {
            close_shared(&session).await;
        }
        if let Some(qwen_backend) = &self.qwen_backend {
            qwen_backend
                .dispose()
                .await
                .map_err(|error| qwen_backend_error(&error))?;
        }
        Ok(())
    }
}

enum Opening {
    Create {
        options: LaunchOptions,
    },
    Resume {
        session_id: String,
        cwd: String,
        options: ResumeOptions,
    },
}

#[derive(Clone, Debug)]
struct SessionConfig {
    model: Option<String>,
    effort: Option<String>,
    permission: String,
    spawn_permission: String,
}

#[derive(Clone, Debug)]
struct PendingPermission {
    rpc_id: RpcId,
    options: Vec<PermissionOption>,
    responding: bool,
}

struct SessionShared {
    core: SessionCore<AcpTransportEvent>,
    engine_id: EngineId,
    flavor: AcpProviderFlavor,
    session_id: Mutex<String>,
    protocol: Mutex<AcpSessionState>,
    config: Mutex<SessionConfig>,
    pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    pre_ready_updates: Mutex<Vec<(String, crate::protocol::AcpSessionUpdate)>>,
    mapper: Mutex<AcpEventMapper>,
    prompt_timeout: Duration,
    collaboration_depth: u8,
    collaboration_mcp_factory: Option<Arc<dyn CollaborationMcpSessionFactory>>,
    collaboration_binding: Mutex<Option<PreparedCollaborationBinding>>,
    /// Set only for a session that may carry team tools.
    team_tools: Option<Arc<dyn interface_contracts::engine::TeamToolsFactory>>,
    /// This session's `iface_team` binding, revoked when it ends.
    team_binding: Mutex<Option<Arc<interface_engine::team_tools::TeamBinding>>>,
    active_collaboration_bindings:
        Arc<Mutex<HashMap<String, Arc<dyn CollaborationMcpSessionBinding>>>>,
    qwen_backend: Option<Arc<dyn QwenBackendLifecycle>>,
    probed_effort_levels: Arc<Mutex<Vec<String>>>,
    adapter_disposed: Arc<AtomicBool>,
    /// The connection's frame bound; a prompt no frame can carry is refused.
    max_frame_bytes: usize,
}

struct PreparedCollaborationBinding {
    binding: Arc<dyn CollaborationMcpSessionBinding>,
    identity: String,
    confirmed: bool,
}

struct AcpSessionHandle {
    shared: Arc<SessionShared>,
    ready: watch::Receiver<ReadyState>,
    events: Option<BoundedEventStream<EngineEvent>>,
}

impl AcpSessionHandle {
    async fn prompt(
        &self,
        text: String,
        dispatched: Option<oneshot::Sender<()>>,
    ) -> DeliveryOutcome<()> {
        if let Some(message) = app_owned_provider_command_error(&text) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                message,
                false,
            ));
        }
        if let Err(error) = self.shared.core.wait_ready(self.ready.clone()).await {
            return DeliveryOutcome::from_error(error);
        }
        send_prompt(&self.shared, text, dispatched).await
    }
}

#[async_trait]
impl SessionHandle for AcpSessionHandle {
    fn engine(&self) -> &EngineId {
        &self.shared.engine_id
    }

    fn session_id(&self) -> String {
        lock(&self.shared.session_id).clone()
    }

    async fn ready(&self) -> Result<ReadySession, EngineError> {
        self.shared.core.wait_ready(self.ready.clone()).await
    }

    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        self.events.take().ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "ACP event stream was already taken",
                false,
            )
        })
    }

    async fn send(&self, text: String) -> DeliveryOutcome<()> {
        self.prompt(text, None).await
    }

    /// `session/prompt` answers only when the turn ends, so report the
    /// moment its frame is queued: an approval or Stop for this turn is
    /// written after it without waiting for the answer.
    async fn send_reporting_dispatch(
        &self,
        text: String,
        dispatched: oneshot::Sender<()>,
    ) -> DeliveryOutcome<()> {
        self.prompt(text, Some(dispatched)).await
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        if let Err(error) = self.shared.core.wait_ready(self.ready.clone()).await {
            return DeliveryOutcome::from_error(error);
        }
        if self.shared.core.is_closed() {
            return self.shared.core.closed_outcome("interrupt");
        }
        let session_id = lock(&self.shared.protocol)
            .session_id()
            .map(ToOwned::to_owned);
        let Some(session_id) = session_id else {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "ACP session id is not ready",
                false,
            ));
        };
        let transport = match self.shared.core.transport() {
            Ok(transport) => transport,
            Err(error) => return DeliveryOutcome::from_error(error),
        };
        cancel_pending_permissions(&self.shared, &transport).await;
        let outcome = transport
            .notify(
                AcpMethod::SESSION_CANCEL,
                &SessionCancelParams::mouse(session_id),
            )
            .await;
        if outcome.is_confirmed() {
            lock(&self.shared.protocol).note_cancelled();
            for event in lock(&self.shared.mapper).settle_open("Stopped") {
                let _ = self.shared.core.emit(event);
            }
            let _ = self.shared.core.emit(EngineEvent {
                at: None,
                event: EngineEventKind::ThinkingSignal {
                    active: false,
                    agent_key: None,
                },
            });
            let _ = self.shared.core.emit(EngineEvent {
                at: None,
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            });
        }
        outcome
    }

    async fn close(&self) -> Result<(), EngineError> {
        close_shared(&self.shared).await;
        Ok(())
    }

    async fn respond_to_approval(
        &self,
        approval_id: String,
        verdict: ApprovalVerdict,
        _response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        if self.shared.core.is_closed() {
            return self.shared.core.closed_outcome("answer approval");
        }
        let pending = {
            let mut permissions = lock(&self.shared.pending_permissions);
            match permissions.get_mut(&approval_id) {
                Some(pending) if !pending.responding => {
                    pending.responding = true;
                    Some(pending.clone())
                }
                _ => None,
            }
        };
        let Some(pending) = pending else {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                format!("ACP approval {approval_id} is not pending"),
                false,
            ));
        };
        let outcome = async {
            let response =
                permission_response(&pending.options, verdict == ApprovalVerdict::Allow, false);
            let value = match serde_json::to_value(response) {
                Ok(value) => value,
                Err(error) => {
                    return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                        EngineErrorKind::Internal,
                        format!("could not encode ACP permission response: {error}"),
                        false,
                    ));
                }
            };
            let transport = match self.shared.core.transport() {
                Ok(transport) => transport,
                Err(error) => return DeliveryOutcome::from_error(error),
            };
            transport.server_result(&pending.rpc_id, value).await
        }
        .await;
        if matches!(outcome, DeliveryOutcome::DefinitelyUnsent(_)) {
            // No byte reached the agent, so the request is still open on its
            // side: keep it answerable by a retry, or by the `cancelled` an
            // interrupt or close sends, unless turn cleanup already cleared it.
            if let Some(entry) = lock(&self.shared.pending_permissions).get_mut(&approval_id) {
                entry.responding = false;
            }
        } else {
            let (removed, none_left) = {
                let mut permissions = lock(&self.shared.pending_permissions);
                let removed = permissions.remove(&approval_id).is_some();
                (removed, permissions.is_empty())
            };
            if outcome.is_confirmed() && removed && none_left {
                let _ = self.shared.core.emit(EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged {
                        status: SessionStatus::Working,
                    },
                });
            }
        }
        outcome
    }

    async fn set_model(&self, model: Option<String>) -> DeliveryOutcome<()> {
        if let Err(error) = lock(&self.shared.core.lifecycle).require_sendable() {
            return DeliveryOutcome::DefinitelyUnsent(error.into());
        }
        if model
            .as_deref()
            .is_some_and(|value| !valid_acp_token(value))
        {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "ACP model must be bounded text without a leading '-' or control characters",
                false,
            ));
        }
        if self.shared.flavor == AcpProviderFlavor::Qwen
            && model.as_deref().is_none_or(|model| model == QWEN_MODEL_ID)
        {
            lock(&self.shared.config).model = Some(QWEN_MODEL_ID.into());
            let _ = self.shared.core.emit(EngineEvent {
                at: None,
                event: EngineEventKind::ConfigChanged {
                    model: Some(QWEN_MODEL_ID.into()),
                    effort: None,
                },
            });
            return DeliveryOutcome::Confirmed(());
        }
        DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "ACP v1 cannot change the process model mid-session",
            false,
        ))
    }

    async fn set_effort(&self, effort: Option<String>) -> DeliveryOutcome<()> {
        if let Err(error) = lock(&self.shared.core.lifecycle).require_sendable() {
            return DeliveryOutcome::DefinitelyUnsent(error.into());
        }
        if !effort.as_deref().is_some_and(valid_acp_token) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "ACP effort must be explicit non-empty text without a leading '-' or control characters",
                false,
            ));
        }
        if self.shared.flavor != AcpProviderFlavor::Grok
            && effort
                .as_ref()
                .is_some_and(|effort| !lock(&self.shared.probed_effort_levels).contains(effort))
        {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                format!(
                    "{} ACP profile does not offer the effort level \"{}\"",
                    self.shared.engine_id,
                    effort.as_deref().unwrap_or_default()
                ),
                false,
            ));
        }
        if !matches!(lock(&self.shared.protocol).phase(), AcpPhase::Ready) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "ACP applies effort to the next prompt; the current prompt was unchanged",
                false,
            ));
        }
        lock(&self.shared.config).effort.clone_from(&effort);
        let _ = self.shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::ConfigChanged {
                model: None,
                effort: Some(effort),
            },
        });
        DeliveryOutcome::Confirmed(())
    }

    async fn set_permission_mode(&self, mode: String) -> DeliveryOutcome<()> {
        if let Err(error) = lock(&self.shared.core.lifecycle).require_sendable() {
            return DeliveryOutcome::DefinitelyUnsent(error.into());
        }
        if self.shared.flavor != AcpProviderFlavor::Grok {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                "portable ACP v1 has no permission-mode setter",
                false,
            ));
        }
        if !GROK_PERMISSION_MODES.contains(&mode.as_str()) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "unsupported Grok permission mode",
                false,
            ));
        }
        let mut config = lock(&self.shared.config);
        if config.spawn_permission == "bypassPermissions" && mode != "bypassPermissions" {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Grok bypass is a spawn-time policy and cannot be relaxed in this process",
                false,
            ));
        }
        config.permission.clone_from(&mode);
        drop(config);
        let _ = self.shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::AccessChanged { mode },
        });
        DeliveryOutcome::Confirmed(())
    }
}

#[allow(clippy::too_many_lines)]
async fn prepare_grok_collaboration(
    shared: &Arc<SessionShared>,
    opening: &Opening,
) -> Result<Vec<AcpMcpServer>, EngineError> {
    if shared.flavor != AcpProviderFlavor::Grok || shared.collaboration_depth == 1 {
        return Ok(Vec::new());
    }
    let Some(factory) = shared.collaboration_mcp_factory.as_ref() else {
        return Ok(Vec::new());
    };
    let resumed = matches!(opening, Opening::Resume { .. });
    let provisional_session_id = lock(&shared.session_id).clone();
    let Ok(binding) = factory
        .create(CollaborationMcpSessionRequest {
            engine: shared.engine_id.clone(),
            provisional_session_id,
            resumed,
        })
        .await
    else {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Notice {
                message: "Interface collaboration is unavailable for this session; Grok started without the broker.".into(),
                tone: NoticeTone::Warning,
            },
        });
        return Ok(Vec::new());
    };
    let descriptor = binding.descriptor();
    if descriptor.validate().is_err() {
        let _ = binding.dispose().await;
        return Err(local_error(
            EngineErrorKind::Protocol,
            "Grok collaboration binding is invalid",
        ));
    }
    let Ok(identity) = serde_json::to_string(&(
        descriptor.name.as_str(),
        descriptor.command.as_str(),
        &descriptor.args,
    )) else {
        let _ = binding.dispose().await;
        return Err(local_error(
            EngineErrorKind::Internal,
            "Grok collaboration descriptor identity failed",
        ));
    };
    if shared.core.is_closed() || shared.adapter_disposed.load(Ordering::Acquire) {
        let _ = binding.dispose().await;
        return Err(local_error(
            EngineErrorKind::Lifecycle,
            "Grok session closed before collaboration setup finished",
        ));
    }
    let duplicate = {
        let mut active = lock(&shared.active_collaboration_bindings);
        if active.contains_key(&identity) {
            true
        } else {
            active.insert(identity.clone(), Arc::clone(&binding));
            false
        }
    };
    if duplicate {
        let _ = binding.dispose().await;
        return Err(local_error(
            EngineErrorKind::Protocol,
            "Grok collaboration factory reused an active session descriptor",
        ));
    }

    if resumed {
        let resumed_session_id = lock(&shared.session_id).clone();
        let Ok(source_session_key) =
            collaboration_source_session_key(&shared.engine_id, &resumed_session_id)
        else {
            lock(&shared.active_collaboration_bindings).remove(&identity);
            let _ = binding.dispose().await;
            return Err(local_error(
                EngineErrorKind::Protocol,
                "Grok collaboration source identity is invalid",
            ));
        };
        if binding
            .confirm_source_session_key(source_session_key)
            .await
            .is_err()
        {
            lock(&shared.active_collaboration_bindings).remove(&identity);
            let _ = binding.dispose().await;
            return Err(local_error(
                EngineErrorKind::Unavailable,
                "Grok collaboration source confirmation failed",
            ));
        }
    }
    if shared.core.is_closed() || shared.adapter_disposed.load(Ordering::Acquire) {
        lock(&shared.active_collaboration_bindings).remove(&identity);
        let _ = binding.dispose().await;
        return Err(local_error(
            EngineErrorKind::Lifecycle,
            "Grok session closed before collaboration setup finished",
        ));
    }
    *lock(&shared.collaboration_binding) = Some(PreparedCollaborationBinding {
        binding,
        identity,
        confirmed: resumed,
    });
    Ok(vec![AcpMcpServer {
        name: descriptor.name,
        command: descriptor.command,
        args: descriptor.args,
        env: Vec::new(),
    }])
}

async fn confirm_fresh_grok_source(
    shared: &Arc<SessionShared>,
    provider_id: &str,
) -> Result<(), EngineError> {
    let binding = lock(&shared.collaboration_binding)
        .as_ref()
        .filter(|prepared| !prepared.confirmed)
        .map(|prepared| Arc::clone(&prepared.binding));
    let Some(binding) = binding else {
        return Ok(());
    };
    let source_session_key = collaboration_source_session_key(&shared.engine_id, provider_id)
        .map_err(|_| {
            local_error(
                EngineErrorKind::Protocol,
                "Grok collaboration source identity is invalid",
            )
        })?;
    binding
        .confirm_source_session_key(source_session_key)
        .await
        .map_err(|_| {
            local_error(
                EngineErrorKind::Unavailable,
                "Grok collaboration source confirmation failed",
            )
        })?;
    if let Some(prepared) = lock(&shared.collaboration_binding).as_mut() {
        prepared.confirmed = true;
    }
    Ok(())
}

/// Team tools (protocol 57): asks the host for this session's `iface_team`
/// binding and answers the server for `session/new` or `session/load`. A
/// resumed session's key is known, so its binding is confirmed at once; a
/// new one's once the agent names it. The agent's process is named to the
/// binding once it is spawned (`open_session_inner`).
async fn prepare_team_tools(shared: &Arc<SessionShared>, opening: &Opening) -> Vec<AcpMcpServer> {
    use interface_engine::team_tools::{TEAM_TOOLS_UNAVAILABLE, TeamSetup, create_team_binding};
    let Some(factory) = shared.team_tools.clone() else {
        return Vec::new();
    };
    let resumed = matches!(opening, Opening::Resume { .. });
    let provisional = lock(&shared.session_id).clone();
    let binding =
        match create_team_binding(&factory, &shared.engine_id, &provisional, resumed).await {
            TeamSetup::None => return Vec::new(),
            TeamSetup::Ready(binding)
                if !resumed || binding.confirm(&shared.engine_id, &provisional).await =>
            {
                binding
            }
            TeamSetup::Ready(_) | TeamSetup::Unavailable => {
                team_tools_unavailable(shared, TEAM_TOOLS_UNAVAILABLE);
                return Vec::new();
            }
        };
    let descriptor = binding.descriptor().clone();
    *lock(&shared.team_binding) = Some(Arc::new(binding));
    vec![AcpMcpServer {
        name: descriptor.name,
        command: descriptor.command,
        args: descriptor.args,
        env: Vec::new(),
    }]
}

/// Confirms a new session's team tools binding for the id the agent gave
/// it; a refusal revokes it.
async fn confirm_fresh_team_tools(shared: &Arc<SessionShared>, provider_id: &str, fresh: bool) {
    if !fresh {
        return;
    }
    let binding = lock(&shared.team_binding).clone();
    if let Some(binding) = binding
        && !binding.confirm(&shared.engine_id, provider_id).await
    {
        lock(&shared.team_binding).take();
        team_tools_unavailable(shared, interface_engine::team_tools::TEAM_TOOLS_UNAVAILABLE);
    }
}

fn team_tools_unavailable(shared: &Arc<SessionShared>, message: &str) {
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::Notice {
            message: message.into(),
            tone: NoticeTone::Warning,
        },
    });
}

async fn dispose_collaboration(shared: &Arc<SessionShared>) {
    let team = lock(&shared.team_binding).take();
    if let Some(team) = team {
        team.dispose().await;
    }
    let Some(prepared) = lock(&shared.collaboration_binding).take() else {
        return;
    };
    if lock(&shared.active_collaboration_bindings)
        .remove(&prepared.identity)
        .is_some()
    {
        let _ = prepared.binding.dispose().await;
    }
}

/// Unwind a half-opened session: report the failure, stop the process, and
/// release the collaboration binding in that order.
async fn abort_open(
    shared: &Arc<SessionShared>,
    transport: Option<&AcpTransport>,
    error: Option<EngineError>,
) {
    if let Some(error) = error {
        shared.core.fail_open(error);
    }
    if let Some(transport) = transport {
        transport.shutdown();
    }
    dispose_collaboration(shared).await;
}

async fn close_shared(shared: &Arc<SessionShared>) {
    if shared.core.closed.swap(true, Ordering::AcqRel) {
        dispose_collaboration(shared).await;
        return;
    }
    lock(&shared.core.lifecycle).begin_close();
    dispose_collaboration(shared).await;
    let was_prompting = matches!(
        lock(&shared.protocol).phase(),
        AcpPhase::Prompting | AcpPhase::PromptOutcomeUnknown
    );
    lock(&shared.protocol).begin_close();
    if let Ok(transport) = shared.core.transport() {
        cancel_pending_permissions(shared, &transport).await;
        if was_prompting {
            let session_id = lock(&shared.session_id).clone();
            let _ = transport
                .notify(
                    AcpMethod::SESSION_CANCEL,
                    &SessionCancelParams::mouse(session_id),
                )
                .await;
        }
        transport.shutdown();
    } else {
        lock(&shared.pending_permissions).clear();
    }
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::SessionClosed,
    });
    shared.core.events.close();
    let _ = lock(&shared.protocol).mark_closed();
    let _ = lock(&shared.core.lifecycle).mark_closed();
    shared
        .core
        .fail_pending_readiness(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "ACP session was closed before readiness",
            false,
        ));
}

/// Resolve every parked ACP permission as a cancellation before sending the
/// provider's turn-cancel notification. A cancellation is deliberately not a
/// deny option: it must not select a provider action the user never chose.
async fn cancel_pending_permissions(shared: &SessionShared, transport: &AcpTransport) {
    let permissions = std::mem::take(&mut *lock(&shared.pending_permissions));
    for (_, permission) in permissions {
        if permission.responding {
            continue;
        }
        let _ = transport
            .server_result(
                &permission.rpc_id,
                json!({"outcome": {"outcome": "cancelled"}}),
            )
            .await;
    }
}

struct OpenAbort {
    transport: Option<Arc<AcpTransport>>,
    error: Option<EngineError>,
}

impl OpenAbort {
    fn without_transport(error: EngineError) -> Self {
        Self {
            transport: None,
            error: Some(error),
        }
    }

    fn with_transport(transport: &Arc<AcpTransport>, error: Option<EngineError>) -> Self {
        Self {
            transport: Some(Arc::clone(transport)),
            error,
        }
    }
}

async fn open_session(shared: Arc<SessionShared>, opening: Opening, config: AcpAdapterConfig) {
    if let Some(qwen_backend) = &shared.qwen_backend {
        if let Err(error) = qwen_backend
            .prepare()
            .await
            .map_err(|error| qwen_backend_error(&error))
        {
            shared.core.fail_open(error);
            return;
        }
        if shared.core.is_closed() || shared.adapter_disposed.load(Ordering::Acquire) {
            return;
        }
    }
    if let Err(abort) = open_session_inner(&shared, opening, config).await {
        abort_open(&shared, abort.transport.as_deref(), abort.error).await;
    }
}

async fn open_session_inner(
    shared: &Arc<SessionShared>,
    opening: Opening,
    config: AcpAdapterConfig,
) -> Result<(), OpenAbort> {
    let cwd = match &opening {
        Opening::Create { options } => options.cwd.clone(),
        Opening::Resume { cwd, .. } => cwd.clone(),
    };
    let mut mcp_servers = prepare_grok_collaboration(shared, &opening)
        .await
        .map_err(OpenAbort::without_transport)?;
    mcp_servers.extend(prepare_team_tools(shared, &opening).await);
    let process_config = lock(&shared.config).clone();
    let spec = process_spec(&config, Path::new(&cwd), Some(&process_config))
        .map_err(OpenAbort::without_transport)?;
    let connection = spawn_connection(&config, &spec, shared.qwen_backend.as_ref())
        .map_err(OpenAbort::without_transport)?;
    // The agent is told of the team tools server only in `session/new` or
    // `session/load`, below: its process is named to the binding first.
    let team = lock(&shared.team_binding).clone();
    if let Some(team) = team {
        team.provider_started(connection.transport.process_id())
            .await;
    }
    let transport = Arc::clone(&connection.transport);
    shared.core.attach_transport(&transport);
    tokio::spawn(session_inbound_loop(
        Arc::clone(shared),
        connection.inbound,
        Arc::clone(&transport),
    ));
    open_on_transport(shared, opening, config, &transport, cwd, mcp_servers)
        .await
        .map_err(|error| OpenAbort::with_transport(&transport, error))
}

/// Runs the handshake and opens the session on a spawned agent. `Err(None)`
/// means the session closed meanwhile, so the agent is shut down quietly;
/// `Err(Some(error))` fails the open.
async fn open_on_transport(
    shared: &Arc<SessionShared>,
    opening: Opening,
    config: AcpAdapterConfig,
    transport: &Arc<AcpTransport>,
    cwd: String,
    mcp_servers: Vec<AcpMcpServer>,
) -> Result<(), Option<EngineError>> {
    if shared.core.is_closed() {
        return Err(None);
    }
    lock(&shared.protocol).begin_initialize()?;
    let initialized = transport
        .request(
            AcpMethod::INITIALIZE,
            &InitializeParams::interface(config.client_version),
        )
        .await
        .into_result()?;
    lock(&shared.protocol).accept_initialize(&initialized)?;
    lock(&shared.protocol).begin_open()?;
    let (response, expected_id, title, initial_prompt) = match &opening {
        Opening::Create { options } => {
            let mode = lock(&shared.config).permission.clone();
            let metadata = grok_metadata(shared.flavor, &mode);
            (
                transport
                    .request(
                        AcpMethod::SESSION_NEW,
                        &SessionNewParams {
                            cwd: options.cwd.clone(),
                            mcp_servers,
                            metadata,
                        },
                    )
                    .await,
                None,
                options.name.clone(),
                Some(options.prompt.clone()),
            )
        }
        Opening::Resume {
            session_id, cwd, ..
        } => {
            let method = lock(&shared.protocol).resume_method();
            let Some(method) = method else {
                return Err(Some(EngineError::definitely_unsent(
                    EngineErrorKind::Unsupported,
                    "ACP agent does not advertise session/resume or session/load",
                    false,
                )));
            };
            (
                transport
                    .request(
                        method.method(),
                        &SessionOpenParams {
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            mcp_servers,
                        },
                    )
                    .await,
                Some(session_id.clone()),
                None,
                None,
            )
        }
    };
    let response = response.into_result()?;
    let provider_id = if let Some(expected_id) = expected_id {
        lock(&shared.protocol)
            .accept_resume(&expected_id, &response)
            .map(ToOwned::to_owned)?
    } else {
        lock(&shared.protocol)
            .accept_open(&response)
            .map(ToOwned::to_owned)?
    };
    confirm_fresh_grok_source(shared, &provider_id).await?;
    confirm_fresh_team_tools(
        shared,
        &provider_id,
        matches!(opening, Opening::Create { .. }),
    )
    .await;
    if shared.core.is_closed() {
        return Err(None);
    }
    lock(&shared.session_id).clone_from(&provider_id);
    publish_ready(shared, &initialized, &response, provider_id, cwd, title)?;
    if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.is_empty()) {
        let _ = send_prompt(shared, prompt, None).await;
    }
    Ok(())
}

/// Marks the opened session ready, publishes it and replays the updates that
/// arrived for it before its id was known. `Err(None)` means the session closed
/// first or its event stream refused an update.
fn publish_ready(
    shared: &Arc<SessionShared>,
    initialized: &Value,
    response: &Value,
    provider_id: String,
    cwd: String,
    title: Option<String>,
) -> Result<(), Option<EngineError>> {
    if lock(&shared.core.lifecycle).mark_ready().is_err() {
        return Err(None);
    }
    let model = value_text(response, &["currentModelId"])
        .or_else(|| value_text(initialized, &["currentModelId"]))
        .map(ToOwned::to_owned)
        .or_else(|| lock(&shared.config).model.clone())
        .unwrap_or_default();
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::SessionMeta {
            session_id: provider_id.clone(),
            model,
            cwd,
            title,
        },
    });
    let permission = lock(&shared.config).permission.clone();
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::AccessChanged { mode: permission },
    });
    let buffered = std::mem::take(&mut *lock(&shared.pre_ready_updates));
    for (session_id, update) in buffered {
        if session_id != provider_id {
            continue;
        }
        lock(&shared.protocol).observe_session_update(&session_id);
        let mapped = lock(&shared.mapper).map(&update);
        for event in mapped {
            if !shared.core.emit(event) {
                return Err(None);
            }
        }
    }
    let _ = shared.core.ready.send(ReadyState::Ready(provider_id));
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn session_inbound_loop(
    shared: Arc<SessionShared>,
    mut inbound: InboundReceiver<AcpTransportEvent>,
    transport: Arc<AcpTransport>,
) {
    let mut engine_events = Vec::new();
    'inbound: while let Some(event) = inbound.recv().await {
        if shared.core.is_closed() {
            break;
        }
        match event {
            AcpTransportEvent::Inbound(AcpInbound::SessionUpdate { session_id, update }) => {
                let matched = {
                    let mut protocol = lock(&shared.protocol);
                    let matched = protocol
                        .session_id()
                        .map(|expected| expected == session_id.as_str());
                    if matched == Some(true) {
                        protocol.observe_session_update(&session_id);
                    }
                    matched
                };
                match matched {
                    None => {
                        let mut buffered = lock(&shared.pre_ready_updates);
                        if buffered.len() == MAX_PRE_READY_UPDATES {
                            drop(buffered);
                            // End the handle before the error is visible, so
                            // readiness cannot be marked in between.
                            lock(&shared.core.lifecycle).mark_failed();
                            let _ = shared.core.emit(EngineEvent {
                                at: None,
                                event: EngineEventKind::Error {
                                    cause: None,
                                    message: "ACP sent too many updates before session readiness"
                                        .into(),
                                    retryable: false,
                                },
                            });
                            transport.shutdown();
                            break 'inbound;
                        }
                        buffered.push((session_id, update));
                    }
                    Some(false) => {}
                    Some(true) => {
                        engine_events.clear();
                        lock(&shared.mapper).map_into(&mut engine_events, &update);
                        for event in engine_events.drain(..) {
                            if !shared.core.emit(event) {
                                break 'inbound;
                            }
                        }
                    }
                }
            }
            AcpTransportEvent::Inbound(AcpInbound::PermissionRequest { id, request }) => {
                handle_permission(&shared, &transport, id, request).await;
            }
            AcpTransportEvent::Inbound(AcpInbound::ServerRequest(request)) => {
                let _ = transport
                    .server_error(
                        &request.id,
                        &RpcRemoteError {
                            code: -32_601,
                            message: format!("unsupported ACP client request {}", request.method),
                            data: None,
                        },
                    )
                    .await;
            }
            AcpTransportEvent::Inbound(
                AcpInbound::Notification(_)
                | AcpInbound::ResponseSettled { .. }
                | AcpInbound::Diagnostic(_),
            ) => {}
            AcpTransportEvent::Inbound(AcpInbound::FramingError(error)) => {
                let _ = shared.core.emit(EngineEvent {
                    at: None,
                    event: EngineEventKind::Notice {
                        message: format!("ACP ignored a malformed frame: {error}"),
                        tone: NoticeTone::Warning,
                    },
                });
            }
            AcpTransportEvent::Inbound(AcpInbound::InvalidMessage(message)) => {
                let _ = shared.core.emit(EngineEvent {
                    at: None,
                    event: EngineEventKind::Notice {
                        message,
                        tone: NoticeTone::Warning,
                    },
                });
            }
            AcpTransportEvent::Closed(message) => {
                for event in lock(&shared.mapper).settle_open("ACP process ended") {
                    let _ = shared.core.emit(event);
                }
                shared.core.fail_transport(message);
                break;
            }
        }
    }
    shared
        .core
        .fail_transport("ACP inbound routing ended unexpectedly".into());
    dispose_collaboration(&shared).await;
}

async fn handle_permission(
    shared: &Arc<SessionShared>,
    transport: &Arc<AcpTransport>,
    id: RpcId,
    request: crate::protocol::PermissionRequestParams,
) {
    let mismatched = lock(&shared.protocol).session_id() != Some(request.session_id.as_str());
    if mismatched {
        let _ = transport
            .server_error(
                &id,
                &RpcRemoteError {
                    code: -32_602,
                    message: "permission request belongs to a different ACP session".into(),
                    data: None,
                },
            )
            .await;
        return;
    }
    if lock(&shared.pending_permissions).len() >= MAX_PENDING_PERMISSIONS {
        let _ = transport
            .server_error(
                &id,
                &RpcRemoteError {
                    code: -32_000,
                    message: "ACP permission queue capacity was reached".into(),
                    data: None,
                },
            )
            .await;
        return;
    }
    let permission = lock(&shared.config).permission.clone();
    if permission == "bypassPermissions" {
        let result = serde_json::to_value(permission_response(&request.options, true, true))
            .unwrap_or_else(|_| json!({"outcome": {"outcome": "cancelled"}}));
        let _ = transport.server_result(&id, result).await;
        return;
    }
    let approval_id = format!(
        "{}:{}:{}",
        shared.engine_id,
        request.session_id,
        NEXT_APPROVAL_ID.fetch_add(1, Ordering::Relaxed)
    );
    let tool_call = request.tool_call.as_object();
    let tool = tool_call
        .and_then(|tool| tool.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let summary = tool_call
        .and_then(|tool| tool.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("ACP agent requests permission")
        .to_owned();
    let input = tool_call
        .and_then(|tool| tool.get("rawInput"))
        .cloned()
        .unwrap_or_else(|| request.tool_call.clone());
    lock(&shared.pending_permissions).insert(
        approval_id.clone(),
        PendingPermission {
            rpc_id: id,
            options: request.options,
            responding: false,
        },
    );
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input,
        },
    });
}

#[allow(clippy::too_many_lines)]
/// Sends one prompt and waits for the turn to end. `dispatched` fires once
/// the `session/prompt` frame is queued, or when the prompt is refused first.
async fn send_prompt(
    shared: &Arc<SessionShared>,
    text: String,
    dispatched: Option<oneshot::Sender<()>>,
) -> DeliveryOutcome<()> {
    if text.is_empty()
        || text.len() > MAX_PROMPT_BYTES
        || !text_fits_frame(&text, shared.max_frame_bytes)
    {
        return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            "ACP prompt must be non-empty and bounded",
            false,
        ));
    }
    if shared.core.is_closed() {
        return shared.core.closed_outcome("send");
    }
    if let Err(error) = lock(&shared.core.lifecycle).require_sendable() {
        let error: EngineError = error.into();
        return DeliveryOutcome::from_error(error);
    }
    let transport = match shared.core.transport() {
        Ok(transport) => transport,
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    let session_id = {
        let mut protocol = lock(&shared.protocol);
        if let Err(error) = protocol.begin_prompt() {
            return DeliveryOutcome::from_error(error);
        }
        protocol.session_id().map(ToOwned::to_owned)
    };
    let Some(session_id) = session_id else {
        return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "ACP session id is not ready",
            false,
        ));
    };
    let config = lock(&shared.config).clone();
    let metadata = prompt_metadata(shared.flavor, &config);
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    });
    let outcome = transport
        .request_with_timeout_reporting_dispatch(
            AcpMethod::SESSION_PROMPT,
            &SessionPromptParams {
                session_id,
                prompt: vec![AcpContentBlock::Text { text }],
                metadata,
            },
            shared.prompt_timeout,
            dispatched,
        )
        .await;
    lock(&shared.protocol).finish_prompt(&outcome);
    let (settle, message) = match &outcome {
        DeliveryOutcome::Confirmed(response) => (true, stop_reason_message(response)),
        DeliveryOutcome::DefinitelyRejected(error) | DeliveryOutcome::DefinitelyUnsent(error) => {
            (true, Some(error.message.clone()))
        }
        DeliveryOutcome::OutcomeUnknown(error) => (false, Some(error.message.clone())),
    };
    // ACP's reserved refusal for an agent that needs a sign-in (-32000,
    // "Authentication required"); its code does not survive the RPC error.
    let cause = matches!(&outcome, DeliveryOutcome::DefinitelyRejected(error)
        if error.message.to_ascii_lowercase().ends_with("authentication required"))
    .then_some(EngineErrorCause::SignedOut);
    if settle {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::ThinkingSignal {
                active: false,
                agent_key: None,
            },
        });
    }
    if let Some(message) = message {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Error {
                cause,
                message,
                retryable: true,
            },
        });
    }
    if settle {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
        });
    }
    outcome.map(|_| ())
}

fn stop_reason_message(response: &Value) -> Option<String> {
    match response.get("stopReason").and_then(Value::as_str) {
        Some("end_turn" | "cancelled") => None,
        Some(reason) => Some(format!("ACP stopped the turn: {reason}")),
        None => Some("ACP ended a prompt without a stop reason".to_owned()),
    }
}

fn session_config(opening: &Opening) -> SessionConfig {
    match opening {
        Opening::Create { options } => {
            let permission = options
                .permission_mode
                .clone()
                .unwrap_or_else(|| "default".into());
            SessionConfig {
                model: options.model.clone(),
                effort: options.effort.clone(),
                spawn_permission: permission.clone(),
                permission,
            }
        }
        Opening::Resume { options, .. } => SessionConfig {
            model: options.model.clone(),
            effort: options.effort.clone(),
            permission: "default".into(),
            spawn_permission: "default".into(),
        },
    }
}

/// The kind of session an opening asks for (absent: code).
const fn opening_kind(opening: &Opening) -> Option<SessionKind> {
    match opening {
        Opening::Create { options } => options.kind,
        Opening::Resume { options, .. } => options.kind,
    }
}

fn opening_collaboration_depth(opening: &Opening) -> Result<u8, EngineError> {
    let depth = match opening {
        Opening::Create { options } => options.collaboration_depth,
        Opening::Resume { options, .. } => options.collaboration_depth,
    };
    normalize_collaboration_depth(depth.map(u64::from)).map_err(|error| {
        EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("invalid ACP collaboration options: {error}"),
            false,
        )
    })
}

fn process_spec(
    config: &AcpAdapterConfig,
    cwd: &Path,
    session: Option<&SessionConfig>,
) -> Result<ProcessSpec, EngineError> {
    let mut spec = acp_process_spec(&config.profile, cwd);
    spec.program = config.executable.get();
    if config.flavor != AcpProviderFlavor::Grok {
        return Ok(spec);
    }
    let Some(session) = session else {
        return Ok(spec);
    };
    let additions = resolve_grok_launch(session)?;
    insert_before_stdio(&mut spec, additions);
    Ok(spec)
}

fn resolve_grok_launch(session: &SessionConfig) -> Result<Vec<OsString>, EngineError> {
    let mut additions = Vec::new();
    if session.spawn_permission == "bypassPermissions" {
        additions.push(OsString::from("--always-approve"));
    }
    if let Some(model) = &session.model {
        if !valid_acp_token(model) {
            return Err(local_error(
                EngineErrorKind::InvalidInput,
                "ACP model cannot be empty, start with '-', or contain control characters",
            ));
        }
        additions.extend([OsString::from("--model"), OsString::from(model)]);
    }
    if let Some(effort) = &session.effort {
        if !valid_acp_token(effort) {
            return Err(local_error(
                EngineErrorKind::InvalidInput,
                "ACP effort cannot be empty, start with '-', or contain control characters",
            ));
        }
        additions.extend([OsString::from("--reasoning-effort"), OsString::from(effort)]);
    }
    Ok(additions)
}

/// Grok reads its own flags before the `stdio` transport token.
fn insert_before_stdio(spec: &mut ProcessSpec, additions: Vec<OsString>) {
    let insert_at = spec
        .args
        .iter()
        .position(|argument| argument == "stdio")
        .unwrap_or(spec.args.len());
    spec.args.splice(insert_at..insert_at, additions);
}

/// Host-owned environment overlay for a built-in ACP child.
///
/// engine-claude augments its child's PATH the same way, in the same order:
/// the resolved executable's own directory (which an explicit configuration
/// override already decided) leads, then the provider's known install
/// locations, then the inherited PATH. A packaged launch cannot rely on the
/// PATH the desktop shell happened to inherit. Custom profiles are owner-argv,
/// not app-resolved, so they keep the ordinary environment.
fn builtin_child_environment(
    flavor: AcpProviderFlavor,
    executable: &Path,
    current_path: Option<&OsStr>,
    home: Option<&Path>,
) -> Result<Vec<(OsString, OsString)>, EngineError> {
    let provider = match flavor {
        AcpProviderFlavor::Grok => BuiltinAcpProvider::Grok,
        AcpProviderFlavor::Qwen => BuiltinAcpProvider::Qwen,
        AcpProviderFlavor::Custom => return Ok(Vec::new()),
    };
    let search_path =
        builtin_search_path(provider, current_path, home, executable).map_err(|error| {
            EngineError::definitely_unsent(EngineErrorKind::InvalidInput, error.to_string(), false)
        })?;
    Ok(vec![(OsString::from("PATH"), search_path)])
}

fn spawn_connection(
    config: &AcpAdapterConfig,
    spec: &ProcessSpec,
    qwen_backend: Option<&Arc<dyn QwenBackendLifecycle>>,
) -> Result<AcpConnection, EngineError> {
    let limits = &config.transport;
    let mut environment = builtin_child_environment(
        config.flavor,
        &spec.program,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )?;
    if let Some(qwen_backend) = qwen_backend {
        environment.extend(qwen_process_environment(qwen_backend.settings_path()));
    }
    environment.extend(config.environment.iter().cloned());
    if environment.is_empty() {
        AcpTransport::spawn(spec, limits)
    } else {
        AcpTransport::spawn_with_environment(spec, limits, &environment)
    }
}

fn qwen_backend_error(error: &QwenBackendError) -> EngineError {
    let kind = if matches!(
        error,
        QwenBackendError::Disposed | QwenBackendError::Cancelled
    ) {
        EngineErrorKind::Lifecycle
    } else {
        EngineErrorKind::Unavailable
    };
    EngineError::definitely_unsent(kind, error.to_string(), error.retryable())
}

fn grok_metadata(flavor: AcpProviderFlavor, permission: &str) -> Option<SessionMetadata> {
    if flavor != AcpProviderFlavor::Grok {
        return None;
    }
    match permission {
        "auto" => Some(SessionMetadata {
            auto_mode: Some(true),
            yolo_mode: None,
        }),
        "bypassPermissions" => Some(SessionMetadata {
            auto_mode: None,
            yolo_mode: Some(true),
        }),
        _ => None,
    }
}

fn prompt_metadata(flavor: AcpProviderFlavor, config: &SessionConfig) -> Option<PromptMetadata> {
    let metadata = PromptMetadata {
        reasoning_effort: config.effort.clone(),
        auto_mode: (flavor == AcpProviderFlavor::Grok && config.permission == "auto")
            .then_some(true),
        yolo_mode: (flavor == AcpProviderFlavor::Grok && config.permission == "bypassPermissions")
            .then_some(true),
    };
    (metadata.reasoning_effort.is_some()
        || metadata.auto_mode.is_some()
        || metadata.yolo_mode.is_some())
    .then_some(metadata)
}

fn validate_launch(
    flavor: AcpProviderFlavor,
    options: &LaunchOptions,
    allowed_efforts: &[String],
) -> Result<(), EngineError> {
    if options.kind == Some(SessionKind::Chat) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            ACP_CHAT_REFUSAL,
            false,
        ));
    }
    refuse_approval_gate(
        options.approval_gate,
        options.additional_directories.as_deref(),
        options.agent_access,
    )?;
    validate_cwd("ACP", &options.cwd)?;
    if options.prompt.is_empty() || options.prompt.len() > MAX_PROMPT_BYTES {
        return Err(local_error(
            EngineErrorKind::InvalidInput,
            "ACP launch prompt must be non-empty and bounded",
        ));
    }
    if let Some(message) = app_owned_provider_command_error(&options.prompt) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            message,
            false,
        ));
    }
    if !effort_advertised(flavor, options.effort.as_ref(), allowed_efforts) {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "portable ACP profile cannot enforce an effort level it did not advertise",
        ));
    }
    if options
        .model
        .as_deref()
        .is_some_and(|model| !model_supported(flavor, model))
    {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "portable ACP profiles cannot apply that process model at launch",
        ));
    }
    if options.cloud == Some(true)
        || options.worktree == Some(true)
        || options.sandbox_mode.is_some()
        || options.setting_sources.is_some()
        || options
            .allowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
        || options
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
    {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "ACP v1 does not support the requested launch controls",
        ));
    }
    let permission = options.permission_mode.as_deref().unwrap_or("default");
    if flavor == AcpProviderFlavor::Grok {
        if !GROK_PERMISSION_MODES.contains(&permission) {
            return Err(local_error(
                EngineErrorKind::InvalidInput,
                "unsupported Grok permission mode",
            ));
        }
    } else if permission != "default" {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "portable ACP profiles support only default permission handling",
        ));
    }
    Ok(())
}

fn validate_resume(
    flavor: AcpProviderFlavor,
    session_id: &str,
    cwd: &str,
    options: &ResumeOptions,
    allowed_efforts: &[String],
) -> Result<(), EngineError> {
    if options.kind == Some(SessionKind::Chat) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            ACP_CHAT_REFUSAL,
            false,
        ));
    }
    refuse_approval_gate(
        options.approval_gate,
        options.additional_directories.as_deref(),
        options.agent_access,
    )?;
    validate_cwd("ACP", cwd)?;
    if session_id.trim().is_empty() || session_id.len() > 512 {
        return Err(local_error(
            EngineErrorKind::InvalidInput,
            "ACP resume session id must be bounded non-empty text",
        ));
    }
    if options.setting_sources.is_some()
        || options
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
    {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "ACP resume cannot restore setting-source or tool overrides",
        ));
    }
    if !effort_advertised(flavor, options.effort.as_ref(), allowed_efforts) {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "portable ACP profile cannot restore an effort level it did not advertise",
        ));
    }
    if options
        .model
        .as_deref()
        .is_some_and(|model| !model_supported(flavor, model))
    {
        return Err(local_error(
            EngineErrorKind::Unsupported,
            "portable ACP profiles cannot restore that process model",
        ));
    }
    Ok(())
}

/// ACP cannot run agents: not behind the gate, and not in an agent's Auto or
/// Bypass mode either, which Interface has no way to confirm the agent applied.
fn refuse_approval_gate(
    approval_gate: Option<bool>,
    additional_directories: Option<&[String]>,
    agent_access: Option<interface_contracts::engine::AgentAccess>,
) -> Result<(), EngineError> {
    if approval_gate == Some(true)
        || agent_access.is_some()
        || additional_directories.is_some_and(|dirs| !dirs.is_empty())
    {
        Err(local_error(
            EngineErrorKind::Unsupported,
            ACP_APPROVAL_GATE_REFUSAL,
        ))
    } else {
        Ok(())
    }
}

/// Provider-neutral facts for an ACP permission request, keyed by the tool
/// call's `kind` with its `rawInput`. Anything unresolvable stays `Unknown`.
fn acp_approval_facts(kind: &str, input: &Value) -> ApprovalFacts {
    let facts = match kind {
        "read" => raw_input_paths(input)
            .filter(|paths| !paths.is_empty())
            .map(|paths| ApprovalFacts::Read { paths }),
        "edit" | "move" => raw_input_paths(input)
            .filter(|paths| !paths.is_empty())
            .map(|paths| ApprovalFacts::Write { paths }),
        "delete" => raw_input_paths(input)
            .filter(|paths| !paths.is_empty())
            .map(|paths| ApprovalFacts::Delete { paths }),
        "search" => raw_input_paths(input).map(|paths| ApprovalFacts::Search { paths }),
        "execute" => raw_command(input.get("command")).map(|command| ApprovalFacts::Shell {
            command,
            cwd: input
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }),
        "fetch" => input
            .get("url")
            .and_then(Value::as_str)
            .map(|url| ApprovalFacts::WebFetch {
                url: url.to_owned(),
                method: "GET".into(),
            }),
        "think" => Some(ApprovalFacts::Internal {
            tool: kind.to_owned(),
        }),
        _ => None,
    };
    facts.unwrap_or_else(|| ApprovalFacts::Unknown {
        tool: kind.to_owned(),
    })
}

/// Every path named under the known `rawInput` keys, in key order. `None`
/// when the input is not an object or any of those keys holds a non-path.
fn raw_input_paths(input: &Value) -> Option<Vec<String>> {
    let input = input.as_object()?;
    let mut paths = Vec::new();
    for key in RAW_INPUT_PATH_KEYS {
        if let Some(value) = input.get(key) {
            paths.push(value.as_str()?.to_owned());
        }
    }
    if let Some(values) = input.get("paths") {
        for value in values.as_array()? {
            paths.push(value.as_str()?.to_owned());
        }
    }
    Some(paths)
}

fn raw_command(command: Option<&Value>) -> Option<String> {
    let text = match command? {
        Value::String(text) => text.clone(),
        Value::Array(parts) if !parts.is_empty() => parts
            .iter()
            .map(|part| part.as_str().map(shell_word))
            .collect::<Option<Vec<_>>>()?
            .join(" "),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn model_supported(flavor: AcpProviderFlavor, model: &str) -> bool {
    match flavor {
        AcpProviderFlavor::Grok => true,
        AcpProviderFlavor::Qwen => model == QWEN_MODEL_ID,
        AcpProviderFlavor::Custom => false,
    }
}

fn effort_advertised(
    flavor: AcpProviderFlavor,
    effort: Option<&String>,
    allowed: &[String],
) -> bool {
    flavor == AcpProviderFlavor::Grok || effort.is_none_or(|effort| allowed.contains(effort))
}

fn valid_acp_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}

fn validate_config(config: &SessionConfig) -> Result<(), EngineError> {
    if config
        .model
        .as_deref()
        .is_some_and(|model| !valid_acp_token(model))
        || config
            .effort
            .as_deref()
            .is_some_and(|effort| !valid_acp_token(effort))
    {
        return Err(local_error(
            EngineErrorKind::InvalidInput,
            "ACP model and effort must be bounded non-empty text without a leading '-' or control characters",
        ));
    }
    Ok(())
}

fn local_error(kind: EngineErrorKind, message: &str) -> EngineError {
    EngineError::definitely_unsent(kind, message, false)
}

async fn run_grok_store<T: Send + 'static>(
    mutating: bool,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, EngineError> {
    blocking::run(mutating, "Grok provider-store task failed", work).await
}

fn grok_store_error(error: &std::io::Error) -> EngineError {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
            EngineError::definitely_rejected(
                EngineErrorKind::Provider,
                "Grok session was not found in the provider store",
                false,
            )
        }
        _ => EngineError::outcome_unknown(
            EngineErrorKind::Transport,
            "Grok provider-store deletion did not complete cleanly",
            true,
        ),
    }
}

fn model_catalog(initialized: &Value, flavor: AcpProviderFlavor) -> Vec<ModelOption> {
    let advertised = initialized
        .get("_meta")
        .and_then(|metadata| metadata.get("modelState"))
        .and_then(|state| state.get("availableModels"))
        .and_then(Value::as_array)
        .or_else(|| {
            initialized
                .get("models")
                .and_then(|models| models.get("availableModels"))
                .and_then(Value::as_array)
        })
        .or_else(|| initialized.get("models").and_then(Value::as_array))
        .or_else(|| initialized.get("modelCatalog").and_then(Value::as_array));
    let mut models = advertised
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let value = model
                .get("modelId")
                .or_else(|| model.get("id"))
                .or_else(|| model.get("value"))?
                .as_str()?
                .to_owned();
            let metadata = model
                .get("_meta")
                .or_else(|| model.get("meta"))
                .and_then(Value::as_object);
            let effort_levels = metadata
                .and_then(|metadata| metadata.get("reasoningEfforts"))
                .or_else(|| model.get("effortLevels"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            Some(ModelOption {
                display_name: model
                    .get("displayName")
                    .or_else(|| model.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(&value)
                    .to_owned(),
                description: model
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                supports_effort: !effort_levels.is_empty()
                    && metadata
                        .and_then(|metadata| metadata.get("supportsReasoningEffort"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                effort_levels,
                is_default: model.get("isDefault").and_then(Value::as_bool),
                supports_fast_mode: None,
                value,
            })
        })
        .collect::<Vec<_>>();
    match flavor {
        AcpProviderFlavor::Qwen => {
            for model in &mut models {
                model.supports_effort = false;
                model.effort_levels.clear();
            }
            if !models.iter().any(|model| model.value == QWEN_MODEL_ID) {
                models.push(ModelOption {
                    value: QWEN_MODEL_ID.into(),
                    display_name: "Qwen 3.8 27B".into(),
                    description: Some("Local MLX 4-bit, tuned for 48 GB Macs".into()),
                    supports_effort: false,
                    effort_levels: Vec::new(),
                    is_default: None,
                    supports_fast_mode: None,
                });
            }
            models
        }
        _ => models,
    }
}

fn effort_levels(models: &[ModelOption]) -> Vec<String> {
    let mut levels = Vec::new();
    for level in models.iter().flat_map(|model| &model.effort_levels) {
        if !levels.contains(level) {
            levels.push(level.clone());
        }
    }
    levels
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
