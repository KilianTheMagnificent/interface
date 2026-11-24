//! Opening a session: the handle, the app-server child, the collaboration
//! fence and source, `thread/start` or `thread/resume`, and verifying the
//! opened thread before it is published ready.

use std::{
    collections::{HashMap, VecDeque},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use interface_common::sync::lock;
use interface_contracts::engine::{
    EngineEvent, EngineEventKind, FastModeState, LaunchOptions, ResumeOptions, SandboxMode,
    collaboration_source_instruction, collaboration_source_session_key,
};
use interface_engine::{
    EngineError, EngineErrorKind, SessionHandle, bounded_event_stream_with_budget,
    event_budget::PROVIDER_EVENT_BYTES,
    provider_search_path,
    session_core::{ReadyState, SessionCore, value_text},
};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::{
    executable::{codex_app_server_process_with_collaboration, codex_gated_feature_args},
    protocol::{
        CodexMethod, CodexSessionState, InitializeParams, ThreadResumeParams, ThreadStartParams,
        host_context_block, thread_id_from_response,
    },
    transport::{CodexConnection, CodexTransport, CodexTransportLimits},
};

use super::{
    AgentMode, CodexAdapter, CodexCollaborationRuntime, PERMISSION_MODES,
    commands::{
        CommandRefreshState, SessionCommands, publish_command_snapshot, spawn_command_refresh,
    },
    gate::{GateRequest, agent_overrides, gate_request, verify_gate},
    launch::{
        emit_ignored_launch_scoping, opening_agent_mode, opening_collaboration_depth, opening_kind,
        validate_session_config,
    },
    models::fast_tier_for,
    safe_line,
    session::{
        CodexSessionHandle, ServiceTierOverride, SessionConfig, SessionShared,
        dispose_session_collaboration, session_inbound_loop,
    },
    turns::send_prompt,
};

static NEXT_PROVISIONAL_ID: AtomicU64 = AtomicU64::new(1);

impl CodexAdapter {
    #[allow(clippy::too_many_lines)]
    pub(super) fn make_handle(
        &self,
        opening: Opening,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        if self.collaboration.adapter_disposed.load(Ordering::Acquire) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex adapter is disposed",
                false,
            ));
        }
        tokio::runtime::Handle::try_current().map_err(|_| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex sessions require a running Tokio runtime",
                false,
            )
        })?;
        let capacity = self.config.event_capacity;
        let (events_tx, events_rx) =
            bounded_event_stream_with_budget(capacity, PROVIDER_EVENT_BYTES);
        let provisional_id = match &opening {
            Opening::Create { .. } => format!(
                "pending-codex-{}",
                NEXT_PROVISIONAL_ID.fetch_add(1, Ordering::Relaxed)
            ),
            Opening::Resume { session_id, .. } => session_id.clone(),
        };
        let (command_refresh, _) = watch::channel(CommandRefreshState::default());
        let cwd = match &opening {
            Opening::Create { options } => options.cwd.clone(),
            Opening::Resume { cwd, .. } => cwd.clone(),
        };
        let config = match &opening {
            Opening::Create { options } => SessionConfig {
                model: options.model.clone(),
                resolved_model: None,
                effort: options.effort.clone(),
                service_tier: ServiceTierOverride::Inherit,
                approval_override: None,
                approval_display: options.permission_mode.clone(),
            },
            Opening::Resume { options, .. } => SessionConfig {
                model: options.model.clone(),
                resolved_model: None,
                effort: options.effort.clone(),
                service_tier: ServiceTierOverride::Inherit,
                approval_override: None,
                approval_display: None,
            },
        };
        validate_session_config(&config)?;
        let collaboration_depth = opening_collaboration_depth(&opening)?;
        let agent_tools = self
            .config
            .agent_tools
            .as_ref()
            .filter(|_| opening_agent_mode(&opening).is_some())
            .and_then(|tools| match &opening {
                Opening::Create { .. } => tools.descriptor_for(Path::new(&cwd)),
                Opening::Resume { session_id, .. } => tools.descriptor_for_resume(
                    Path::new(&cwd),
                    &interface_contracts::engine::session_key(&self.id, session_id),
                ),
            });
        if opening_agent_mode(&opening).is_some()
            && self.config.agent_tools.is_some()
            && agent_tools.is_none()
        {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Unavailable,
                "The scoped Agent task tool endpoint is unavailable; no provider was started",
                false,
            ));
        }
        let (core, ready_rx) = SessionCore::new(events_tx);
        let shared = Arc::new(SessionShared {
            core,
            engine_id: self.id.clone(),
            session_id: Mutex::new(provisional_id),
            protocol: Mutex::new(CodexSessionState::default()),
            config: Mutex::new(config),
            cwd,
            commands: Mutex::new(SessionCommands::default()),
            command_refresh,
            pending_approvals: Mutex::new(HashMap::new()),
            fast_tiers: Arc::clone(&self.fast_tiers),
            effect_lock: AsyncMutex::new(()),
            collaboration_depth,
            collaboration_lease: Mutex::new(None),
            collaboration_source_key: Mutex::new(None),
            pending_host_context: Mutex::new(None),
            agent_tools,
            team_tools: self.config.team_tools.clone().filter(|_| {
                interface_contracts::engine::team_tools_eligible(
                    interface_contracts::engine::session_kind_of(opening_kind(&opening)),
                    false,
                    opening_agent_mode(&opening).is_some(),
                    collaboration_depth,
                )
            }),
            team_binding: Mutex::new(None),
            approval_gate: opening_agent_mode(&opening) == Some(AgentMode::Gated),
            agent_mode: opening_agent_mode(&opening),
            file_change_paths: Mutex::new(VecDeque::new()),
            max_frame_bytes: self.config.transport.max_frame_bytes,
        });
        emit_ignored_launch_scoping(&shared, &opening);
        let handle = CodexSessionHandle {
            shared: Arc::clone(&shared),
            ready: ready_rx,
            events: Some(events_rx),
        };
        {
            let mut sessions = lock(&self.sessions);
            sessions.retain(|session| session.strong_count() > 0);
            sessions.push(Arc::downgrade(&shared));
        }
        let executable = self.config.executable_path.get();
        let client_version = self.config.client_version.clone();
        let limits = self.config.transport.clone();
        let collaboration = self.collaboration.for_session(shared.agent_mode.is_some());
        let environment = self.config.environment.clone();
        tokio::spawn(async move {
            open_session(
                shared,
                opening,
                executable,
                client_version,
                limits,
                collaboration,
                environment,
            )
            .await;
        });
        Ok(Box::new(handle))
    }
}

pub(super) enum Opening {
    Create {
        options: LaunchOptions,
    },
    Resume {
        session_id: String,
        cwd: String,
        options: ResumeOptions,
    },
}

async fn fail_open_and_revoke(
    shared: &Arc<SessionShared>,
    transport: Option<&Arc<CodexTransport>>,
    error: EngineError,
) {
    shared.core.fail_open(error);
    if let Some(transport) = transport {
        transport.shutdown();
    }
    dispose_session_collaboration(shared).await;
}

async fn enter_codex_depth_fence(
    shared: &Arc<SessionShared>,
    collaboration: &CodexCollaborationRuntime,
) -> Result<(), EngineError> {
    if shared.collaboration_depth != 1 || collaboration.descriptor.is_none() {
        return Ok(());
    }
    let entrant = collaboration.depth_fence.as_ref().ok_or_else(|| {
        EngineError::definitely_unsent(
            EngineErrorKind::Internal,
            "Codex collaboration depth fence is not configured",
            false,
        )
    })?;
    let lease = entrant.enter().await.map_err(|_| {
        EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            "Codex collaboration depth fence is unavailable",
            false,
        )
    })?;
    if shared.core.is_closed() || collaboration.adapter_disposed.load(Ordering::Acquire) {
        let _ = lease.dispose().await;
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "Codex session closed before the collaboration depth fence entered",
            false,
        ));
    }
    *lock(&shared.collaboration_lease) = Some(lease);
    Ok(())
}

async fn register_codex_source(
    shared: &Arc<SessionShared>,
    collaboration: &CodexCollaborationRuntime,
    provider_id: &str,
    collaboration_enabled: bool,
) -> Result<Option<String>, EngineError> {
    if shared.collaboration_depth == 1
        || !collaboration_enabled
        || collaboration.descriptor.is_none()
    {
        return Ok(None);
    }
    let source_key =
        collaboration_source_session_key(&shared.engine_id, provider_id).map_err(|_| {
            EngineError::definitely_unsent(
                EngineErrorKind::Protocol,
                "Codex collaboration source identity is invalid",
                false,
            )
        })?;
    let registrar = collaboration.registrar.as_ref().ok_or_else(|| {
        EngineError::definitely_unsent(
            EngineErrorKind::Internal,
            "Codex collaboration source registrar is not configured",
            false,
        )
    })?;
    let Ok(lease) = registrar.register(source_key.clone()).await else {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Notice {
                message: "Interface collaboration is unavailable for this session; Codex continued without a bound source.".into(),
                tone: interface_contracts::engine::NoticeTone::Warning,
            },
        });
        return Ok(None);
    };
    if shared.core.is_closed() || collaboration.adapter_disposed.load(Ordering::Acquire) {
        let _ = lease.dispose().await;
        return Ok(None);
    }
    let Ok(instruction) = collaboration_source_instruction(&source_key) else {
        let _ = lease.dispose().await;
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Protocol,
            "Codex collaboration source instruction is invalid",
            false,
        ));
    };
    *lock(&shared.collaboration_source_key) = Some(source_key);
    *lock(&shared.collaboration_lease) = Some(lease);
    Ok(Some(instruction))
}

/// Host-owned environment overlay for the app-server child.
///
/// engine-claude augments its child's PATH the same way, in the same order:
/// the resolved executable's own directory (which an explicit configuration
/// override already decided) leads, then the known install locations, then the
/// inherited PATH. A packaged launch cannot rely on the PATH the desktop shell
/// happened to inherit.
pub(super) fn codex_child_environment(
    executable: &Path,
    current_path: Option<&OsStr>,
    home: Option<&Path>,
) -> Result<Vec<(OsString, OsString)>, EngineError> {
    let search_path = provider_search_path(executable, current_path, home).map_err(|error| {
        EngineError::definitely_unsent(EngineErrorKind::InvalidInput, error.to_string(), false)
    })?;
    Ok(vec![(OsString::from("PATH"), search_path)])
}

/// `account_environment` is an added provider account's `CODEX_HOME`, set
/// after the search path; empty for the Default account.
pub(super) async fn initialized_session_connection(
    executable: &Path,
    client_version: &str,
    limits: &CodexTransportLimits,
    collaboration: &CodexCollaborationRuntime,
    agent_mode: Option<AgentMode>,
    account_environment: &[(OsString, OsString)],
) -> Result<(CodexConnection, bool), EngineError> {
    let mut environment = codex_child_environment(
        executable,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )?;
    environment.extend(account_environment.iter().cloned());
    let mut collaboration_enabled =
        collaboration.descriptor.is_some() && collaboration.process_enabled.load(Ordering::Acquire);
    loop {
        let descriptor = collaboration_enabled
            .then_some(collaboration.descriptor.as_ref())
            .flatten();
        let mut spec = codex_app_server_process_with_collaboration(executable, descriptor)
            .map_err(|error| {
                EngineError::definitely_unsent(
                    EngineErrorKind::InvalidInput,
                    error.to_string(),
                    false,
                )
            })?;
        // A gated or Auto agent runs this app-server alone, so disabling the acting
        // features per process (never for other sessions) keeps them off for
        // every thread, turn and resume it serves.
        if agent_mode.is_some_and(AgentMode::restricts) {
            spec.args.extend(codex_gated_feature_args());
        }
        let connection = CodexTransport::spawn_with_environment(&spec, limits, &environment)?;
        let mut initialization = InitializeParams::interface(client_version);
        if agent_mode.is_some() {
            initialization.capabilities = Some(json!({"experimentalApi":true}));
        }
        let initialized = connection
            .transport
            .request(CodexMethod::INITIALIZE, &initialization)
            .await
            .into_result();
        match initialized {
            Ok(_) => return Ok((connection, collaboration_enabled)),
            Err(_) if collaboration_enabled => {
                connection.transport.shutdown();
                collaboration
                    .process_enabled
                    .store(false, Ordering::Release);
                collaboration
                    .startup_unavailable
                    .store(true, Ordering::Release);
                collaboration_enabled = false;
            }
            Err(error) => {
                connection.transport.shutdown();
                return Err(error);
            }
        }
    }
}

async fn open_session(
    shared: Arc<SessionShared>,
    opening: Opening,
    executable: PathBuf,
    client_version: String,
    limits: CodexTransportLimits,
    collaboration: CodexCollaborationRuntime,
    environment: Vec<(OsString, OsString)>,
) {
    if let Err(error) = enter_codex_depth_fence(&shared, &collaboration).await {
        fail_open_and_revoke(&shared, None, error).await;
        return;
    }
    let (connection, collaboration_enabled) = match initialized_session_connection(
        &executable,
        &client_version,
        &limits,
        &collaboration,
        shared.agent_mode,
        &environment,
    )
    .await
    {
        Ok(connection) => connection,
        Err(error) => {
            fail_open_and_revoke(&shared, None, error).await;
            return;
        }
    };
    let transport = Arc::clone(&connection.transport);
    shared.core.attach_transport(&transport);
    tokio::spawn(session_inbound_loop(
        Arc::clone(&shared),
        connection.inbound,
        Arc::clone(&transport),
    ));
    let opened = open_on_transport(
        &shared,
        &opening,
        &transport,
        &collaboration,
        collaboration_enabled,
    )
    .await;
    match opened {
        Ok(()) => {}
        Err(Some(error)) => fail_open_and_revoke(&shared, Some(&transport), error).await,
        Err(None) => {
            transport.shutdown();
            dispose_session_collaboration(&shared).await;
        }
    }
}

/// Opens the thread once its app-server is initialized, sending the launch
/// prompt only after every check passed. `Err(None)` means the session closed
/// meanwhile, so the transport is shut down quietly; `Err(Some(error))` fails
/// the open and revokes its collaboration.
async fn open_on_transport(
    shared: &Arc<SessionShared>,
    opening: &Opening,
    transport: &Arc<CodexTransport>,
    collaboration: &CodexCollaborationRuntime,
    collaboration_enabled: bool,
) -> Result<(), Option<EngineError>> {
    if shared.core.is_closed() {
        return Err(None);
    }
    if shared.collaboration_depth == 0
        && collaboration.descriptor.is_some()
        && collaboration.startup_unavailable.load(Ordering::Acquire)
        && !collaboration_enabled
    {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Notice {
                message: "Interface collaboration is unavailable for this session; Codex restarted without the broker.".into(),
                tone: interface_contracts::engine::NoticeTone::Warning,
            },
        });
    }

    let opened = match opening {
        Opening::Create { options } => start_thread(shared, options, transport).await?,
        Opening::Resume {
            session_id,
            cwd,
            options,
        } => {
            resume_thread(
                shared,
                transport,
                collaboration,
                collaboration_enabled,
                session_id,
                cwd,
                options,
            )
            .await?
        }
    };
    let response_id = verify_opened(&opened)?;
    if opened.expected_id.is_none() {
        confirm_team_tools(shared, &response_id).await;
    }
    if opened.expected_id.is_none() {
        // A fresh thread has no id until `thread.start` answers, so the source
        // label cannot ride `developer_instructions` the way resume sends it.
        // Owe it to the first turn instead; until it is delivered the bound
        // source is unverifiable.
        let instruction =
            register_codex_source(shared, collaboration, &response_id, collaboration_enabled)
                .await?;
        *lock(&shared.pending_host_context) = instruction.as_deref().map(host_context_block);
    }
    if shared.core.is_closed() {
        return Err(None);
    }
    lock(&shared.protocol).adopt_thread_response(&opened.response)?;
    lock(&shared.session_id).clone_from(&response_id);
    let OpenedThread {
        response,
        cwd,
        title,
        initial_prompt,
        ..
    } = opened;
    publish_ready(shared, &response, response_id, cwd, title)?;
    if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.is_empty()) {
        let outcome = send_prompt(shared, prompt, true).await;
        if let Some(error) = outcome.error() {
            let _ = shared.core.emit(EngineEvent {
                at: None,
                event: EngineEventKind::Error {
                    cause: None,
                    message: error.message.clone(),
                    retryable: error.retryable,
                },
            });
        }
    }
    Ok(())
}

/// Team tools (protocol 57): asks the host for this thread's `iface_team`
/// binding, names the app-server process serving it (the host answers only
/// that process and the ones it started), and adds the server to the
/// thread's config. A resumed thread's key is known, so its binding is
/// confirmed at once; a new thread's is confirmed once `thread/start` names
/// it. Without a binding the config is left as it was.
async fn with_team_tools(
    shared: &Arc<SessionShared>,
    transport: &CodexTransport,
    config: Option<Value>,
    provisional: &str,
    resumed: bool,
) -> Option<Value> {
    use interface_engine::team_tools::{TEAM_TOOLS_UNAVAILABLE, TeamSetup, create_team_binding};
    let Some(factory) = shared.team_tools.clone() else {
        return config;
    };
    let binding = match create_team_binding(&factory, &shared.engine_id, provisional, resumed).await
    {
        TeamSetup::None => return config,
        TeamSetup::Ready(binding)
            if !resumed || binding.confirm(&shared.engine_id, provisional).await =>
        {
            binding
        }
        TeamSetup::Ready(_) | TeamSetup::Unavailable => {
            team_tools_unavailable(shared, TEAM_TOOLS_UNAVAILABLE);
            return config;
        }
    };
    binding.provider_started(transport.process_id()).await;
    let mut config = config.unwrap_or_else(|| json!({}));
    config["mcp_servers"][interface_contracts::engine::TEAM_TOOLS_SERVER] =
        super::gate::team_tool_config(binding.descriptor());
    *lock(&shared.team_binding) = Some(Arc::new(binding));
    Some(config)
}

/// Confirms a new thread's team tools binding for the id Codex gave it; a
/// refusal revokes it (the thread's tools then answer nothing).
async fn confirm_team_tools(shared: &Arc<SessionShared>, provider_id: &str) {
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
            tone: interface_contracts::engine::NoticeTone::Warning,
        },
    });
}

/// A `thread/start` or `thread/resume` answer, with what the open asked for so
/// [`verify_opened`] can hold the answer to it.
struct OpenedThread {
    response: Value,
    /// The thread a resume asked for; a fresh thread has none.
    expected_id: Option<String>,
    cwd: String,
    title: Option<String>,
    initial_prompt: Option<String>,
    requested_sandbox: Option<SandboxMode>,
    requested_service_tier: Option<String>,
    requested_gate: Option<GateRequest>,
}

/// Resolves the Fast tier before the gate's `config/read`, then starts the
/// thread.
async fn start_thread(
    shared: &Arc<SessionShared>,
    options: &LaunchOptions,
    transport: &Arc<CodexTransport>,
) -> Result<OpenedThread, Option<EngineError>> {
    let launch_service_tier = if options.fast_mode == Some(true) {
        match fast_tier_for(transport, &shared.fast_tiers, options.model.as_deref()).await? {
            Some(tier) => Some(tier),
            None => {
                return Err(Some(EngineError::definitely_unsent(
                    EngineErrorKind::Unsupported,
                    format!(
                        "Codex model {} does not advertise a Fast service tier.",
                        options.model.as_deref().unwrap_or("(provider default)")
                    ),
                    false,
                )));
            }
        }
    } else {
        None
    };
    if let Some(tier) = &launch_service_tier {
        lock(&shared.config).service_tier = ServiceTierOverride::Exact(tier.clone());
    }
    let gate = gate_request(
        shared.agent_mode,
        &options.cwd,
        options.additional_directories.as_deref(),
    );
    let gate_overrides = agent_overrides(transport, gate.as_ref(), shared).await?;
    let provisional = lock(&shared.session_id).clone();
    let gate_overrides =
        with_team_tools(shared, transport, gate_overrides, &provisional, false).await;
    let response = transport
        .request(
            CodexMethod::THREAD_START,
            &ThreadStartParams {
                cwd: options.cwd.clone(),
                model: options.model.clone(),
                service_tier: launch_service_tier.clone(),
                approval_policy: match &gate {
                    Some(gate) => Some(gate.mode.approval_policy().into()),
                    None => options.permission_mode.clone(),
                },
                approvals_reviewer: gate
                    .as_ref()
                    .and_then(|gate| gate.mode.approvals_reviewer())
                    .map(Into::into),
                sandbox: match &gate {
                    Some(gate) => Some(gate.mode.sandbox()),
                    None => options.sandbox_mode,
                },
                config: gate_overrides,
            },
        )
        .await
        .into_result()?;
    Ok(OpenedThread {
        response,
        expected_id: None,
        cwd: options.cwd.clone(),
        title: options.name.clone(),
        initial_prompt: Some(options.prompt.clone()),
        requested_sandbox: options.sandbox_mode,
        requested_service_tier: launch_service_tier,
        requested_gate: gate,
    })
}

/// Resumes the thread with its collaboration source label, and once without
/// it if Codex refused the labelled resume.
async fn resume_thread(
    shared: &Arc<SessionShared>,
    transport: &Arc<CodexTransport>,
    collaboration: &CodexCollaborationRuntime,
    collaboration_enabled: bool,
    session_id: &str,
    cwd: &str,
    options: &ResumeOptions,
) -> Result<OpenedThread, Option<EngineError>> {
    let gate = gate_request(
        shared.agent_mode,
        cwd,
        options.additional_directories.as_deref(),
    );
    let gate_overrides = agent_overrides(transport, gate.as_ref(), shared).await?;
    let gate_overrides = with_team_tools(shared, transport, gate_overrides, session_id, true).await;
    // Outside an agent, the approval policy the owner chose for this thread,
    // which the host keeps across restarts; without one Codex picks its own.
    let owner_policy = options
        .permission_mode
        .as_deref()
        .filter(|mode| PERMISSION_MODES.contains(mode));
    let resume_params = |developer_instructions: Option<String>| ThreadResumeParams {
        thread_id: session_id.to_owned(),
        cwd: cwd.to_owned(),
        developer_instructions,
        runtime_workspace_roots: gate.as_ref().map(|_| Vec::new()),
        approval_policy: match &gate {
            Some(gate) => Some(gate.mode.approval_policy().into()),
            None => owner_policy.map(Into::into),
        },
        approvals_reviewer: gate
            .as_ref()
            .and_then(|gate| gate.mode.approvals_reviewer())
            .map(Into::into),
        sandbox: gate.as_ref().map(|gate| gate.mode.sandbox()),
        config: gate_overrides.clone(),
    };
    let developer_instructions =
        register_codex_source(shared, collaboration, session_id, collaboration_enabled).await?;
    if shared.core.is_closed() {
        return Err(None);
    }
    let first = transport
        .request(
            CodexMethod::THREAD_RESUME,
            &resume_params(developer_instructions.clone()),
        )
        .await
        .into_result();
    let response = if developer_instructions.is_some() && first.is_err() {
        dispose_session_collaboration(shared).await;
        if shared.core.is_closed() {
            return Err(None);
        }
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::Notice {
                message: "Interface collaboration is unavailable for this session; Codex continued without a bound source.".into(),
                tone: interface_contracts::engine::NoticeTone::Warning,
            },
        });
        transport
            .request(CodexMethod::THREAD_RESUME, &resume_params(None))
            .await
            .into_result()
    } else {
        first
    }?;
    Ok(OpenedThread {
        response,
        expected_id: Some(session_id.to_owned()),
        cwd: cwd.to_owned(),
        title: None,
        initial_prompt: None,
        requested_sandbox: None,
        requested_service_tier: None,
        requested_gate: gate,
    })
}

/// Checks an opened thread in a fixed order: the requested sandbox, the gate,
/// the Fast tier, an exact provider id, then the id a resume asked for. The
/// first failing check is the error reported; the thread's id is returned.
fn verify_opened(opened: &OpenedThread) -> Result<String, EngineError> {
    let response = &opened.response;
    verify_requested_sandbox(opened.requested_sandbox, response)?;
    verify_gate(opened.requested_gate.as_ref(), response)?;
    verify_requested_service_tier(opened.requested_service_tier.as_deref(), response)?;
    let Some(response_id) = thread_id_from_response(response) else {
        return Err(EngineError::definitely_rejected(
            EngineErrorKind::Protocol,
            "Codex opened a thread without an exact provider id",
            false,
        ));
    };
    if opened
        .expected_id
        .as_deref()
        .is_some_and(|id| id != response_id)
    {
        return Err(EngineError::definitely_rejected(
            EngineErrorKind::Protocol,
            "Codex resumed a different thread id than requested",
            false,
        ));
    }
    Ok(response_id.to_owned())
}

/// Adopts the settings the verified thread reports, marks the session ready and
/// publishes it. `Err(None)` means the session closed first.
fn publish_ready(
    shared: &Arc<SessionShared>,
    response: &Value,
    response_id: String,
    cwd: String,
    title: Option<String>,
) -> Result<(), Option<EngineError>> {
    let (model, access_mode, fast_state) = {
        let mut config = lock(&shared.config);
        config.resolved_model = value_text(response, &["model"]).map(ToOwned::to_owned);
        if let Some(service_tier) = response.get("serviceTier") {
            config.service_tier = if service_tier.is_null() {
                ServiceTierOverride::Standard
            } else {
                service_tier
                    .as_str()
                    .filter(|tier| !tier.is_empty())
                    .map_or(ServiceTierOverride::Inherit, |tier| {
                        ServiceTierOverride::Exact(tier.to_owned())
                    })
            };
        }
        if let Some(permission) = value_text(response, &["approvalPolicy"])
            .filter(|permission| PERMISSION_MODES.contains(permission))
        {
            config.approval_display = Some(permission.to_owned());
        }
        let model = config
            .resolved_model
            .clone()
            .or_else(|| config.model.clone())
            .unwrap_or_default();
        let access_mode = config.approval_display.clone();
        let fast_state = match &config.service_tier {
            ServiceTierOverride::Standard => Some(FastModeState::Off),
            ServiceTierOverride::Exact(current) => {
                let catalogue = lock(&shared.fast_tiers);
                catalogue.loaded.then(|| {
                    if catalogue.tier_for(Some(model.as_str())).as_deref() == Some(current) {
                        FastModeState::On
                    } else {
                        FastModeState::Off
                    }
                })
            }
            ServiceTierOverride::Inherit => None,
        };
        (model, access_mode, fast_state)
    };
    let marked_ready = lock(&shared.core.lifecycle).mark_ready();
    if marked_ready.is_err() {
        return Err(None);
    }
    if let Some(mode) = access_mode {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::AccessChanged { mode },
        });
    }
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::SessionMeta {
            session_id: response_id.clone(),
            model,
            cwd,
            title,
        },
    });
    if let Some(state) = fast_state {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::FastmodeChanged {
                state,
                reason: None,
            },
        });
    }
    publish_command_snapshot(shared, Vec::new(), true);
    spawn_command_refresh(Arc::clone(shared), false);
    let _ = shared.core.ready.send(ReadyState::Ready(response_id));
    Ok(())
}

pub(super) fn verify_requested_sandbox(
    requested: Option<SandboxMode>,
    response: &Value,
) -> Result<(), EngineError> {
    let (expected_kind, label) = match requested {
        Some(SandboxMode::ReadOnly) => ("readOnly", "read-only"),
        Some(SandboxMode::WorkspaceWrite) => ("workspaceWrite", "workspace-write"),
        _ => return Ok(()),
    };
    let policy = response.get("sandbox").and_then(Value::as_object);
    let kind = policy
        .and_then(|policy| policy.get("type"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let network_access = policy.and_then(|policy| policy.get("networkAccess"));
    if kind == Some(expected_kind) && network_access.is_none_or(Value::is_boolean) {
        return Ok(());
    }
    let message = match kind {
        None => format!(
            "Codex did not report the effective sandbox policy required to verify a requested {label} launch; the launch prompt was not sent"
        ),
        Some(reported) if reported == expected_kind => format!(
            "Codex reported a malformed {label} sandbox policy; the launch prompt was not sent"
        ),
        Some(reported) => format!(
            "Codex reported effective sandbox policy \"{}\" after Interface requested {label}; the launch prompt was not sent",
            safe_line(reported, 240)
        ),
    };
    Err(EngineError::outcome_unknown(
        EngineErrorKind::Protocol,
        message,
        false,
    ))
}

fn verify_requested_service_tier(
    requested: Option<&str>,
    response: &Value,
) -> Result<(), EngineError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let Some(response) = response.as_object() else {
        return Ok(());
    };
    let Some(raw_reported) = response.get("serviceTier") else {
        return Ok(());
    };
    let reported = raw_reported.as_str().filter(|tier| !tier.is_empty());
    if reported == Some(requested) {
        return Ok(());
    }
    Err(EngineError::outcome_unknown(
        EngineErrorKind::Protocol,
        format!(
            "Codex did not report the requested Fast service tier ({requested}); it reported {}.",
            reported.unwrap_or("Standard")
        ),
        false,
    ))
}
