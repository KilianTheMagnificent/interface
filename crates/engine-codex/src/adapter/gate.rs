//! Agent approval modes: what a gated, Auto or Bypass thread asks Codex for,
//! the `config.toml` overrides it sends, and checking the thread's answer.

use std::path::Path;

use interface_contracts::engine::CollaborationMcpStdioDescriptor;
use interface_engine::{EngineError, EngineErrorKind};
use serde_json::{Map, Value, json};

use crate::{
    executable::GATED_DISABLED_FEATURES, protocol::CodexMethod, transport::CodexTransport,
};

use super::{AgentMode, safe_line, session::SessionShared};

/// What a gated thread asks Codex for, and what its response must confirm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GateRequest {
    pub(super) mode: AgentMode,
    /// The thread's working directory, which workspace-write keeps writable on its own.
    pub(super) cwd: String,
    /// Granted folders beyond `cwd`, each requested as an extra writable root.
    pub(super) directories: Vec<String>,
}

/// The gate for a launch, `None` when it is not gated.
pub(super) fn gate_request(
    mode: Option<AgentMode>,
    cwd: &str,
    additional_directories: Option<&[String]>,
) -> Option<GateRequest> {
    mode.map(|mode| GateRequest {
        mode,
        cwd: cwd.to_owned(),
        directories: additional_directories
            .unwrap_or_default()
            .iter()
            .filter(|directory| !same_directory(directory, cwd))
            .cloned()
            .collect(),
    })
}

/// Verifies that the opened thread reports every part of Interface's approval
/// gate. The thread exists by now, so a mismatch is outcome-unknown and the
/// launch prompt is never sent.
pub(super) fn verify_gate(gate: Option<&GateRequest>, response: &Value) -> Result<(), EngineError> {
    let Some(gate) = gate else {
        return Ok(());
    };
    let Some(mismatch) = gate_mismatch(gate, response) else {
        return Ok(());
    };
    let what = match gate.mode {
        AgentMode::Gated => "Interface's approval gate",
        AgentMode::Auto => "the agent's auto-review settings",
        AgentMode::Bypass => "the agent's bypass settings",
    };
    Err(EngineError::outcome_unknown(
        EngineErrorKind::Protocol,
        format!("Codex did not confirm {what}: {mismatch}; the launch prompt was not sent"),
        false,
    ))
}

fn reported(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "no value".to_owned(),
        Some(Value::String(text)) => format!("\"{}\"", safe_line(text, 240)),
        Some(_) => "an unrecognized value".to_owned(),
    }
}

fn gate_mismatch(gate: &GateRequest, response: &Value) -> Option<String> {
    let expected_policy = gate.mode.approval_policy();
    let policy = response.get("approvalPolicy");
    if policy.and_then(Value::as_str) != Some(expected_policy) {
        return Some(format!(
            "it reported approval policy {} instead of \"{expected_policy}\"",
            reported(policy)
        ));
    }
    let Some(expected_reviewer) = gate.mode.approvals_reviewer() else {
        // Bypass: no approvals and no sandbox, so there is nothing else to confirm.
        let kind = response
            .get("sandbox")
            .and_then(Value::as_object)
            .and_then(|sandbox| sandbox.get("type"));
        return (kind.and_then(Value::as_str) != Some("dangerFullAccess")).then(|| {
            format!(
                "it reported sandbox {} instead of \"dangerFullAccess\"",
                reported(kind)
            )
        });
    };
    let reviewer = response.get("approvalsReviewer");
    if reviewer.and_then(Value::as_str) != Some(expected_reviewer) {
        return Some(format!(
            "it reported approvals reviewer {} instead of \"{expected_reviewer}\"",
            reported(reviewer)
        ));
    }
    let sandbox = response.get("sandbox").and_then(Value::as_object);
    let kind = sandbox.and_then(|sandbox| sandbox.get("type"));
    let Some(sandbox) = sandbox.filter(|_| kind.and_then(Value::as_str) == Some("workspaceWrite"))
    else {
        return Some(format!(
            "it reported sandbox {} instead of \"workspaceWrite\"",
            reported(kind)
        ));
    };
    if sandbox.get("networkAccess") != Some(&Value::Bool(true)) {
        return Some("it did not report sandbox network access as enabled".into());
    }
    if sandbox.get("excludeSlashTmp") != Some(&Value::Bool(true)) {
        return Some(
            "it did not report /tmp as excluded from the writable sandbox (excludeSlashTmp)".into(),
        );
    }
    // Workspace-write keeps the thread's own directory writable and does not repeat it in
    // `writableRoots`, so the echoed `cwd` must be the one Interface asked for.
    let cwd = response.get("cwd");
    if !cwd
        .and_then(Value::as_str)
        .is_some_and(|cwd| same_directory(cwd, &gate.cwd))
    {
        return Some(format!(
            "it reported working directory {} instead of {}",
            reported(cwd),
            safe_line(&gate.cwd, 240)
        ));
    }
    let Some(roots) = sandbox.get("writableRoots").and_then(Value::as_array) else {
        return Some("its writable roots were missing or malformed".into());
    };
    if roots.iter().any(|root| {
        root.as_str().is_none_or(|root| {
            !Path::new(root).is_absolute()
                || (!gate
                    .directories
                    .iter()
                    .any(|requested| same_directory(root, requested))
                    && !is_reported_implicit_temp(root, sandbox))
        })
    }) {
        let actual = roots
            .iter()
            .take(18)
            .map(|root| reported(Some(root)))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!(
            "its writable roots included an unrequested or malformed directory (reported: {actual})"
        ));
    }
    gate.directories
        .iter()
        .find(|requested| {
            !roots
                .iter()
                .filter_map(Value::as_str)
                .any(|root| same_directory(root, requested))
        })
        .map(|missing| {
            format!(
                "its writable roots did not include {}",
                safe_line(missing, 240)
            )
        })
}

// Codex 0.156 resumes can explicitly echo the already-permitted per-user TMPDIR.
// This is the exact inherited directory only, never shared /tmp, /, an ancestor,
// or a new root supplied by the response. Missing exclusion metadata is not proof.
fn is_reported_implicit_temp(root: &str, sandbox: &Map<String, Value>) -> bool {
    std::env::var_os("TMPDIR")
        .is_some_and(|temp| matches_reported_implicit_temp(root, sandbox, &temp.to_string_lossy()))
}

pub(super) fn matches_reported_implicit_temp(
    root: &str,
    sandbox: &Map<String, Value>,
    temp: &str,
) -> bool {
    sandbox.get("excludeTmpdirEnvVar") == Some(&Value::Bool(false))
        && Path::new(temp).is_absolute()
        && !same_directory(temp, "/")
        && !same_directory(temp, "/tmp")
        && same_directory(root, temp)
}

/// Whether two absolute paths name the same directory, as written or once resolved, so
/// `/tmp` and `/private/tmp` agree.
fn same_directory(left: &str, right: &str) -> bool {
    Path::new(left) == Path::new(right)
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

/// `config.toml` overrides for a gated thread: the sandbox's extra writable roots, and every
/// MCP server Codex would start switched off.
///
/// Codex raises no approval request for an MCP tool call, so a gated thread runs without MCP
/// servers. An empty `mcp_servers` table is merged into the owner's configuration and changes
/// nothing (probed against Codex 0.154.0 on 2026-09-15); `enabled = false` on each configured
/// server keeps it from starting on thread start, its turns and resume. The thread response
/// reports no MCP state to verify this against; live probe P2 confirms it. A server the owner
/// adds between this read and the thread start is not covered.
///
/// # Errors
/// Nothing has been opened yet, so a failure to list the servers is definitely unsent.
async fn gate_config(transport: &CodexTransport, gate: &GateRequest) -> Result<Value, EngineError> {
    let unlisted = || {
        EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "Codex did not list its configured MCP servers, so Interface could not turn them off for this agent; the agent was NOT started",
            false,
        )
    };
    let response = transport
        .request(
            CodexMethod::CONFIG_READ,
            &json!({"cwd": gate.cwd, "includeLayers": false}),
        )
        .await
        .into_result()
        .map_err(|_| unlisted())?;
    verify_gated_features(&response)?;
    let servers = configured_mcp_servers(&response).ok_or_else(unlisted)?;
    Ok(gate_overrides(gate, &servers))
}

/// The thread config an agent launch sends: Gated and Auto switch off the owner's
/// MCP servers after `config/read`, and every agent mode attaches Interface's task
/// tools. Bypass without task tools sends no config at all.
///
/// # Errors
/// A gated or Auto launch whose MCP servers cannot be listed is definitely unsent.
pub(super) async fn agent_overrides(
    transport: &CodexTransport,
    gate: Option<&GateRequest>,
    shared: &SessionShared,
) -> Result<Option<Value>, EngineError> {
    let Some(gate) = gate else {
        return Ok(None);
    };
    let mut config = if gate.mode.restricts() {
        gate_config(transport, gate).await?
    } else {
        json!({})
    };
    if let Some(tools) = &shared.agent_tools {
        config["mcp_servers"][&tools.name] = agent_tool_config(tools);
    }
    Ok((config != json!({})).then_some(config))
}

/// Fails closed if `config/read` still reports one of [`GATED_DISABLED_FEATURES`]
/// enabled after the app-server was spawned with each turned off, so a build
/// that ignored the `-c features.X=false` override never opens a gated thread.
/// Codex 0.154.0 omits the `features` table from this response, and the spawn
/// argv (asserted by `codex_gated_feature_args`) is then the authoritative
/// disable; a missing or non-object table is not treated as a failure.
fn verify_gated_features(response: &Value) -> Result<(), EngineError> {
    let Some(features) = response
        .get("config")
        .and_then(|config| config.get("features"))
        .and_then(Value::as_object)
    else {
        return Ok(());
    };
    let still_on: Vec<&str> = GATED_DISABLED_FEATURES
        .iter()
        .copied()
        .filter(|feature| features.get(*feature).is_some_and(feature_is_enabled))
        .collect();
    if still_on.is_empty() {
        return Ok(());
    }
    Err(EngineError::definitely_unsent(
        EngineErrorKind::Unsupported,
        format!(
            "Codex reported these acting features still enabled for a gated agent, so it was NOT started: {}",
            still_on.join(", ")
        ),
        false,
    ))
}

/// Whether a `features` entry reports the feature on. Codex reports either a
/// bare boolean or a table with an `enabled` field; anything else is read as on,
/// failing closed.
fn feature_is_enabled(value: &Value) -> bool {
    match value {
        Value::Bool(enabled) => *enabled,
        Value::Object(table) => table
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        Value::Null => false,
        _ => true,
    }
}

/// The MCP server names in a `config/read` response; `None` unless it reports the table.
pub(super) fn configured_mcp_servers(response: &Value) -> Option<Vec<String>> {
    response
        .get("config")?
        .get("mcp_servers")?
        .as_object()
        .map(|servers| servers.keys().cloned().collect())
}

// Only this host-supplied endpoint owns these tools. Its backend validates every
// request against the current task, budgets and exact owner grants before effects.
pub(super) fn agent_tool_config(tools: &CollaborationMcpStdioDescriptor) -> Value {
    json!({"command":tools.command,"args":tools.args,"env":{},"enabled":true,
        "required":true,"startup_timeout_sec":10,"tool_timeout_sec":310,
        "default_tools_approval_mode":"approve",
        "enabled_tools":interface_contracts::agent_work::AGENT_TOOL_NAMES,
        "tools":interface_contracts::agent_work::AGENT_TOOL_NAMES.iter().map(|name| ((*name).to_owned(), json!({"approval_mode":"approve"}))).collect::<Map<String, Value>>()})
}

/// The `iface_team` server's thread config (team tools, protocol 57): not
/// required (a thread starts without it rather than not at all), and a tool
/// timeout past `wait_thread`'s longest wait (600 s).
pub(super) fn team_tool_config(tools: &CollaborationMcpStdioDescriptor) -> Value {
    json!({"command":tools.command,"args":tools.args,"env":{},"enabled":true,
        "required":false,"startup_timeout_sec":10,"tool_timeout_sec":660,
        "enabled_tools":interface_contracts::engine::TEAM_TOOL_NAMES})
}

pub(super) fn gate_overrides(gate: &GateRequest, mcp_servers: &[String]) -> Value {
    let mut config = json!({
        "notify": [],
        "sandbox_workspace_write": {
            "writable_roots": gate.directories,
            "network_access": true,
            // Shared /tmp is not scratch; the per-user $TMPDIR stays writable.
            "exclude_slash_tmp": true,
        }
    });
    if !mcp_servers.is_empty() {
        config["mcp_servers"] = mcp_servers
            .iter()
            .map(|name| (name.clone(), json!({"enabled": false})))
            .collect::<Map<String, Value>>()
            .into();
    }
    config
}
