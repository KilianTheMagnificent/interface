//! Launch and resume admission: every check that refuses a session before a
//! provider process is started.

use std::path::Path;

use interface_contracts::{
    engine::{
        AgentAccess, EngineEvent, EngineEventKind, LaunchOptions, NoticeTone, ResumeOptions,
        SandboxMode, SessionKind, normalize_collaboration_depth, session_kind_of,
        validate_agent_access,
    },
    provider_commands::app_owned_provider_command_error,
};
use interface_engine::{EngineError, EngineErrorKind, session_core::validate_cwd};

use super::{
    AgentMode, GATE_APPROVAL_POLICY, MAX_PROMPT_BYTES, PERMISSION_MODES, agent_mode,
    codex_chat_refusal,
    open::Opening,
    safe_line,
    session::{SessionConfig, SessionShared},
};

pub(super) fn validate_launch(options: &LaunchOptions) -> Result<(), EngineError> {
    if options.kind == Some(SessionKind::Chat) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            codex_chat_refusal(),
            false,
        ));
    }
    validate_cwd("Codex", &options.cwd)?;
    let mode = checked_agent_mode(options.agent_access, options.approval_gate, options.kind)?;
    validate_gate_directories(mode, options.additional_directories.as_deref())?;
    if let Some(mode) = mode.filter(|mode| *mode != AgentMode::Gated) {
        if options.sandbox_mode.is_some() || options.permission_mode.is_some() {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                format!(
                    "This agent's Codex approval policy and sandbox come from its {} setting, so this session was NOT started",
                    if mode == AgentMode::Auto {
                        "Auto"
                    } else {
                        "Bypass"
                    }
                ),
                false,
            ));
        }
    }
    if options.approval_gate == Some(true) {
        let sandbox = match options.sandbox_mode {
            Some(SandboxMode::ReadOnly) => Some("read-only"),
            Some(SandboxMode::DangerFullAccess) => Some("danger-full-access"),
            Some(SandboxMode::WorkspaceWrite) | None => None,
        };
        if let Some(sandbox) = sandbox {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                format!(
                    "Codex enforces Interface's approval gate with the workspace-write sandbox, so this {sandbox} session was NOT started"
                ),
                false,
            ));
        }
        if let Some(mode) = options
            .permission_mode
            .as_deref()
            .filter(|mode| *mode != GATE_APPROVAL_POLICY)
        {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                format!(
                    "Codex enforces Interface's approval gate with the untrusted approval policy, so this {} session was NOT started",
                    safe_line(mode, 64)
                ),
                false,
            ));
        }
    }
    // Protocol v22 permits creating an idle thread so a durable controller can
    // record its identity before dispatching any project work. open_session
    // already omits turn/start for an empty prompt; ordinary sends stay nonempty.
    if options.prompt.len() > MAX_PROMPT_BYTES {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            "Codex launch prompt must be bounded",
            false,
        ));
    }
    if let Some(message) = app_owned_provider_command_error(&options.prompt) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            message,
            false,
        ));
    }
    let refused = [
        options
            .allowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
            .then_some("allowedTools"),
        options
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
            .then_some("disallowedTools"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !refused.is_empty() {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            format!(
                "Codex cannot scope tools by name, so this session was NOT started: the codex app-server protocol has no {} field. Clear {} to launch it on Codex.",
                refused.join(" or "),
                refused.join(" and ")
            ),
            false,
        ));
    }
    Ok(())
}

pub(super) fn ignored_launch_scoping(options: &LaunchOptions) -> Vec<&'static str> {
    [
        options
            .setting_sources
            .is_some()
            .then_some("settingSources"),
        options.allowed_tools.is_some().then_some("allowedTools"),
        options
            .disallowed_tools
            .is_some()
            .then_some("disallowedTools"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) fn emit_ignored_launch_scoping(shared: &SessionShared, opening: &Opening) {
    let Opening::Create { options } = opening else {
        return;
    };
    let ignored = ignored_launch_scoping(options);
    if ignored.is_empty() {
        return;
    }
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::Notice {
            message: format!(
                "Codex ignored {}: the codex app-server protocol has no equivalent, so this session runs with whatever tools and settings your own Codex configuration allows.",
                ignored.join(", ")
            ),
            tone: NoticeTone::Warning,
        },
    });
}

pub(super) fn validate_resume(
    session_id: &str,
    cwd: &str,
    options: &ResumeOptions,
) -> Result<(), EngineError> {
    if options.kind == Some(SessionKind::Chat) {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            codex_chat_refusal(),
            false,
        ));
    }
    if session_id.trim().is_empty() || session_id.len() > 512 {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            "Codex session id must be bounded non-empty text",
            false,
        ));
    }
    validate_cwd("Codex", cwd)?;
    let mode = checked_agent_mode(options.agent_access, options.approval_gate, options.kind)?;
    validate_gate_directories(mode, options.additional_directories.as_deref())
}

/// The agent mode a launch or resume asks for; refused unsent with the gate or
/// outside an agent session.
fn checked_agent_mode(
    access: Option<AgentAccess>,
    approval_gate: Option<bool>,
    kind: Option<SessionKind>,
) -> Result<Option<AgentMode>, EngineError> {
    validate_agent_access(access, approval_gate, session_kind_of(kind)).map_err(|message| {
        EngineError::definitely_unsent(EngineErrorKind::InvalidInput, message, false)
    })?;
    Ok(agent_mode(approval_gate, access))
}

/// Extra folders become Codex writable roots, which only an agent thread
/// requests and verifies (Bypass has no sandbox for them to narrow).
fn validate_gate_directories(
    mode: Option<AgentMode>,
    additional_directories: Option<&[String]>,
) -> Result<(), EngineError> {
    let directories = additional_directories.unwrap_or_default();
    for directory in directories {
        if directory.is_empty() || directory.len() > 16_384 || !Path::new(directory).is_absolute() {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "Codex additional directories must be bounded absolute paths",
                false,
            ));
        }
    }
    if !directories.is_empty() && mode.is_none() {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "Codex grants additional directories only behind Interface's approval gate, so this session was NOT started",
            false,
        ));
    }
    Ok(())
}

pub(super) fn validate_session_config(config: &SessionConfig) -> Result<(), EngineError> {
    if config
        .approval_display
        .as_deref()
        .is_some_and(|mode| !PERMISSION_MODES.contains(&mode))
    {
        return Err(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            "unsupported Codex permission mode",
            false,
        ));
    }
    Ok(())
}

pub(super) fn opening_agent_mode(opening: &Opening) -> Option<AgentMode> {
    match opening {
        Opening::Create { options } => agent_mode(options.approval_gate, options.agent_access),
        Opening::Resume { options, .. } => agent_mode(options.approval_gate, options.agent_access),
    }
}

/// The kind of session an opening asks for (absent: code).
pub(super) const fn opening_kind(opening: &Opening) -> Option<SessionKind> {
    match opening {
        Opening::Create { options } => options.kind,
        Opening::Resume { options, .. } => options.kind,
    }
}

pub(super) fn opening_collaboration_depth(opening: &Opening) -> Result<u8, EngineError> {
    let depth = match opening {
        Opening::Create { options } => options.collaboration_depth,
        Opening::Resume { options, .. } => options.collaboration_depth,
    };
    normalize_collaboration_depth(depth.map(u64::from)).map_err(|error| {
        EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            format!("invalid Codex collaboration options: {error}"),
            false,
        )
    })
}
