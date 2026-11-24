use std::collections::{HashMap, HashSet};

use interface_contracts::ipc::{
    OrchestrationAgentSteerRequest, OrchestrationAgentStopRequest, OrchestrationInboxRequest,
    OrchestrationRoleClaimRequest, OrchestrationRoleRelinquishRequest,
    OrchestrationTimelineRequest, OrchestrationTranscriptRequest,
};
use interface_contracts::orchestration::{AgentNodeState, MailboxMessageV1, MessageDeliveryState};
use interface_contracts::orchestration_control::{
    OrchestrationAgentCreateRequestV1, OrchestrationAgentPageV1, OrchestrationAgentSendRequestV1,
    OrchestrationControlAgentV1, OrchestrationControlMethod, OrchestrationControlSnapshotV1,
    OrchestrationEventPageV1, OrchestrationInboxPageV1, OrchestrationTranscriptPageV1,
};
use interface_host::OrchestrationHostControl;

use crate::{
    errors::{CLI_EXIT, CommandResult},
    sanitize::for_display,
};

#[derive(Debug)]
struct UsageError(String);

/// The `iface orchestrate` option grammar: `--name value`, `--name=value`,
/// bare flags and `--`.
#[derive(Default)]
struct ParsedOptions {
    positionals: Vec<String>,
    values: HashMap<String, String>,
    flags: HashSet<String>,
}

fn parse_options(
    args: &[String],
    values: &[&str],
    flags: &[&str],
) -> Result<ParsedOptions, UsageError> {
    let mut result = ParsedOptions::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            result.positionals.extend_from_slice(&args[index + 1..]);
            break;
        }
        if !token.starts_with("--") {
            result.positionals.push(token.clone());
            index += 1;
            continue;
        }
        let (name, inline) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(name, value)| (name, Some(value)));
        if flags.contains(&name) {
            if inline.is_some() {
                return Err(UsageError(format!("{name} does not take a value.")));
            }
            if !result.flags.insert(name.to_owned()) {
                return Err(UsageError(format!("{name} was repeated.")));
            }
            index += 1;
            continue;
        }
        if !values.contains(&name) {
            return Err(UsageError(format!("Unknown option: {name}")));
        }
        if result.values.contains_key(name) {
            return Err(UsageError(format!("{name} was repeated.")));
        }
        let value = if let Some(value) = inline {
            value
        } else {
            index += 1;
            args.get(index).map_or("", String::as_str)
        };
        if value.is_empty() || (inline.is_none() && value.starts_with("--")) {
            return Err(UsageError(format!("{name} requires a value.")));
        }
        result.values.insert(name.to_owned(), value.to_owned());
        index += 1;
    }
    Ok(result)
}

fn required<'a>(options: &'a ParsedOptions, name: &str) -> Result<&'a str, UsageError> {
    options
        .values
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| UsageError(format!("{name} is required.")))
}

fn integer_in_range(
    value: Option<&String>,
    default: usize,
    minimum: usize,
    maximum: usize,
    name: &str,
) -> Result<usize, UsageError> {
    let parsed = value
        .map_or(Ok(default), |value| value.parse::<usize>())
        .map_err(|_| {
            UsageError(format!(
                "{name} must be an integer from {minimum} to {maximum}."
            ))
        })?;
    if (minimum..=maximum).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(UsageError(format!(
            "{name} must be an integer from {minimum} to {maximum}."
        )))
    }
}

pub const ORCHESTRATE_USAGE: &str = "Usage: iface orchestrate <verb> [options]

Verbs:
  status [--json]                       Show the bounded agent summary
  role-claim <agent-id> --role R --responsibility TEXT
  role-relinquish <claim-id>
  worktree-escrow <lease-id> --source AGENT --target AGENT --owner-revision N
                                        Explicitly unavailable in iface
  agents [--after AGENT] [--limit 1..32] [--json]
  inbox <agent-id> [--thread ID] [--before MESSAGE] [--limit 1..4] [--json]
  transcript <agent-id> [--lines N] [--json]
  start-agent --engine ID --name N --cwd DIR --prompt TEXT [--access MODE]
              [--request-id ID]
  send <agent-id> --body TEXT [--sender AGENT] [--expect-reply] [--request-id ID]
  reply <agent-id> --parent MESSAGE --body TEXT [--sender AGENT]
        [--expect-reply] [--request-id ID]
  steer <agent-id> --body TEXT [--request-id ID]
  stop <agent-id>
  events|monitor [--since N] [--limit 1..64] [--json]
                                        Read one stable event page";

#[derive(Debug)]
enum CommandError {
    Usage(String),
    Unsent(String),
    Blocked(String),
    Generic(String),
}

impl From<UsageError> for CommandError {
    fn from(error: UsageError) -> Self {
        Self::Usage(error.0)
    }
}

/// Runs the provider-free legacy orchestration surface. Provider-bound verbs
/// remain fail-closed until the long-lived Rust host owns an executor/control
/// transport; no state mutation is claimed when no effect host exists.
#[must_use]
pub async fn run(
    args: &[String],
    control: &OrchestrationHostControl,
    live_provider_host: bool,
    cwd: &std::path::Path,
) -> CommandResult {
    match run_inner(args, control, live_provider_host, cwd).await {
        Ok(result) => result,
        Err(CommandError::Usage(message)) => CommandResult {
            stdout: String::new(),
            stderr: format!("iface: {}\n{ORCHESTRATE_USAGE}\n", for_display(&message)),
            exit_code: CLI_EXIT.validation,
        },
        Err(CommandError::Unsent(message)) => CommandResult {
            stdout: String::new(),
            stderr: format!("iface: {}\n", for_display(&message)),
            exit_code: CLI_EXIT.unsent,
        },
        Err(CommandError::Blocked(message)) => CommandResult {
            stdout: String::new(),
            stderr: format!("iface: orchestration blocked: {}\n", for_display(&message)),
            exit_code: CLI_EXIT.denial,
        },
        Err(CommandError::Generic(message)) => CommandResult {
            stdout: String::new(),
            stderr: format!("iface: {}\n", for_display(&message)),
            exit_code: CLI_EXIT.generic,
        },
    }
}

async fn run_inner(
    args: &[String],
    control: &OrchestrationHostControl,
    live_provider_host: bool,
    cwd: &std::path::Path,
) -> Result<CommandResult, CommandError> {
    let verb = args.first().map_or("status", String::as_str);
    if matches!(verb, "help" | "--help") {
        return Ok(success(format!("{ORCHESTRATE_USAGE}\n")));
    }
    match verb {
        "status" => status_command(control, &args[1..]).await.map(success),
        "agents" => agents_command(control, &args[1..]).await.map(success),
        "inbox" => inbox_command(control, &args[1..]).await.map(success),
        "events" | "monitor" => events_command(control, verb, &args[1..]).await.map(success),
        "worktree-escrow" => worktree_escrow_command(&args[1..], live_provider_host).map(success),
        "transcript" | "start-agent" | "send" | "reply" | "steer" | "stop"
            if !live_provider_host =>
        {
            Err(CommandError::Unsent(
                "Start Interface or the iface TUI before invoking provider orchestration commands."
                    .into(),
            ))
        }
        "role-claim" | "claim-role" => role_claim_command(control, verb, &args[1..]).await,
        "role-relinquish" | "relinquish-role" => {
            role_relinquish_command(control, verb, &args[1..]).await
        }
        "transcript" => transcript_command(control, &args[1..]).await,
        "start-agent" => start_agent_command(control, &args[1..], cwd).await,
        "send" | "reply" => message_command(control, verb, &args[1..]).await,
        "steer" => steer_command(control, &args[1..]).await,
        "stop" => stop_command(control, &args[1..]).await,
        _ => Err(CommandError::Usage(format!(
            "Unknown orchestrate verb: {verb}"
        ))),
    }
}

fn success(stdout: String) -> CommandResult {
    CommandResult {
        stdout,
        stderr: String::new(),
        exit_code: CLI_EXIT.success,
    }
}

async fn role_claim_command(
    control: &OrchestrationHostControl,
    verb: &str,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(args, &["--role", "--responsibility"], &[])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(format!(
            "{verb} requires one agent id."
        )));
    }
    let snapshot: OrchestrationControlSnapshotV1 = control
        .invoke(
            OrchestrationControlMethod::RoleClaim,
            &OrchestrationRoleClaimRequest {
                agent_id: parsed.positionals[0].clone(),
                role: required(&parsed, "--role")?.to_owned(),
                responsibility: required(&parsed, "--responsibility")?.to_owned(),
            },
        )
        .await
        .map_err(generic)?;
    Ok(success(json_envelope("snapshot", &snapshot)?))
}

async fn role_relinquish_command(
    control: &OrchestrationHostControl,
    verb: &str,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(args, &[], &[])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(format!(
            "{verb} requires one claim id."
        )));
    }
    let snapshot: OrchestrationControlSnapshotV1 = control
        .invoke(
            OrchestrationControlMethod::RoleRelinquish,
            &OrchestrationRoleRelinquishRequest {
                claim_id: parsed.positionals[0].clone(),
            },
        )
        .await
        .map_err(generic)?;
    Ok(success(json_envelope("snapshot", &snapshot)?))
}

fn worktree_escrow_command(
    args: &[String],
    live_provider_host: bool,
) -> Result<String, CommandError> {
    if !live_provider_host {
        return Err(CommandError::Unsent(
            "Start Interface or the iface TUI before invoking provider orchestration commands."
                .into(),
        ));
    }
    let parsed = parse_options(args, &["--source", "--target", "--owner-revision"], &[])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(
            "worktree-escrow requires one lease id.".into(),
        ));
    }
    required(&parsed, "--source")?;
    required(&parsed, "--target")?;
    required(&parsed, "--owner-revision")?;
    non_negative(parsed.values.get("--owner-revision"), 0, "--owner-revision")?;
    Err(CommandError::Blocked(
        "iface has no managed-worktree executor; escrow was not attempted.".into(),
    ))
}

async fn transcript_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(args, &["--lines"], &["--json"])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(
            "orchestrate transcript requires one agent id.".into(),
        ));
    }
    let lines = integer_in_range(parsed.values.get("--lines"), 200, 1, 2_000, "--lines")?;
    let page: OrchestrationTranscriptPageV1 = control
        .invoke(
            OrchestrationControlMethod::AgentTranscript,
            &OrchestrationTranscriptRequest {
                agent_id: parsed.positionals[0].clone(),
                lines: lines as u64,
            },
        )
        .await
        .map_err(generic)?;
    let stdout = if parsed.flags.contains("--json") {
        json_envelope("page", &page)?
    } else {
        let mut rows = page
            .lines
            .iter()
            .map(|line| for_display(line))
            .collect::<Vec<_>>();
        if page.truncated {
            rows.push(format!(
                "(transcript bounded to {} public lines)",
                page.lines.len()
            ));
        }
        format!("{}\n", rows.join("\n"))
    };
    Ok(success(stdout))
}

async fn start_agent_command(
    control: &OrchestrationHostControl,
    args: &[String],
    cwd: &std::path::Path,
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(
        args,
        &[
            "--engine",
            "--name",
            "--cwd",
            "--prompt",
            "--access",
            "--model",
            "--effort",
            "--parent",
            "--role",
            "--request-id",
        ],
        &[],
    )?;
    if !parsed.positionals.is_empty() {
        return Err(CommandError::Usage(
            "orchestrate start-agent takes options only.".into(),
        ));
    }
    let access_mode = parsed
        .values
        .get("--access")
        .cloned()
        .unwrap_or_else(|| "project".into());
    if access_mode == "isolated" {
        return Err(CommandError::Blocked(
            "iface cannot start an isolated agent because its executor has no durable worktree lease."
                .into(),
        ));
    }
    let engine_id = required(&parsed, "--engine")?.to_owned();
    let name = required(&parsed, "--name")?.to_owned();
    let prompt = required(&parsed, "--prompt")?.to_owned();
    let requested_cwd = std::path::Path::new(required(&parsed, "--cwd")?);
    let canonical_cwd = if requested_cwd.is_absolute() {
        requested_cwd.to_owned()
    } else {
        cwd.join(requested_cwd)
    };
    let agent: OrchestrationControlAgentV1 = control
        .invoke(
            OrchestrationControlMethod::AgentCreate,
            &OrchestrationAgentCreateRequestV1 {
                agent_id: None,
                engine_id,
                name,
                prompt,
                canonical_cwd: canonical_cwd.to_string_lossy().into_owned(),
                access_mode,
                model: parsed.values.get("--model").cloned(),
                effort: parsed.values.get("--effort").cloned(),
                parent_agent_id: parsed.values.get("--parent").cloned(),
                role: parsed.values.get("--role").cloned(),
                required_capabilities: None,
                client_request_id: request_id(&parsed),
            },
        )
        .await
        .map_err(generic)?;
    agent_result(&agent, Some(CLI_EXIT.unsent))
}

async fn message_command(
    control: &OrchestrationHostControl,
    verb: &str,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(
        args,
        &["--body", "--parent", "--request-id", "--sender"],
        &["--expect-reply"],
    )?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(format!(
            "orchestrate {verb} requires one agent id."
        )));
    }
    let message: MailboxMessageV1 = control
        .invoke(
            if verb == "send" {
                OrchestrationControlMethod::AgentSend
            } else {
                OrchestrationControlMethod::AgentReply
            },
            &OrchestrationAgentSendRequestV1 {
                recipient_agent_id: parsed.positionals[0].clone(),
                body: required(&parsed, "--body")?.to_owned(),
                expect_reply: parsed.flags.contains("--expect-reply"),
                client_request_id: request_id(&parsed),
                sender_agent_id: parsed.values.get("--sender").cloned(),
                thread_id: None,
                parent_message_id: if verb == "reply" {
                    Some(required(&parsed, "--parent")?.to_owned())
                } else {
                    None
                },
            },
        )
        .await
        .map_err(generic)?;
    Ok(CommandResult {
        stdout: json_envelope("message", &message)?,
        stderr: String::new(),
        exit_code: match message.receipts.last().map(|receipt| receipt.state) {
            Some(MessageDeliveryState::Failed) => CLI_EXIT.unsent,
            Some(MessageDeliveryState::Unknown) => CLI_EXIT.unknown,
            _ => CLI_EXIT.success,
        },
    })
}

async fn steer_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(args, &["--body", "--request-id"], &[])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(
            "orchestrate steer requires one agent id.".into(),
        ));
    }
    let agent: OrchestrationControlAgentV1 = control
        .invoke(
            OrchestrationControlMethod::AgentSteer,
            &OrchestrationAgentSteerRequest {
                agent_id: parsed.positionals[0].clone(),
                body: required(&parsed, "--body")?.to_owned(),
                client_request_id: request_id(&parsed),
            },
        )
        .await
        .map_err(generic)?;
    agent_result(&agent, None)
}

async fn stop_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<CommandResult, CommandError> {
    let parsed = parse_options(args, &[], &[])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(
            "orchestrate stop requires one agent id.".into(),
        ));
    }
    let agent: OrchestrationControlAgentV1 = control
        .invoke(
            OrchestrationControlMethod::AgentStop,
            &OrchestrationAgentStopRequest {
                agent_id: parsed.positionals[0].clone(),
            },
        )
        .await
        .map_err(generic)?;
    agent_result(&agent, None)
}

/// The caller's `--request-id`, or a fresh one for this request.
fn request_id(parsed: &ParsedOptions) -> String {
    parsed
        .values
        .get("--request-id")
        .cloned()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

/// The `agent` envelope a start, steer or stop prints. An agent in an unknown
/// state exits outcome-unknown; a failed one exits `failed_exit` when given
/// (a start that never ran sent nothing), and success otherwise.
fn agent_result(
    agent: &OrchestrationControlAgentV1,
    failed_exit: Option<u8>,
) -> Result<CommandResult, CommandError> {
    let exit_code = match (agent.state, failed_exit) {
        (AgentNodeState::Unknown, _) => CLI_EXIT.unknown,
        (AgentNodeState::Failed, Some(code)) => code,
        _ => CLI_EXIT.success,
    };
    Ok(CommandResult {
        stdout: json_envelope("agent", agent)?,
        stderr: String::new(),
        exit_code,
    })
}

async fn status_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<String, CommandError> {
    let parsed = parse_options(args, &[], &["--json"])?;
    if !parsed.positionals.is_empty() {
        return Err(CommandError::Usage(
            "orchestrate status takes no arguments.".into(),
        ));
    }
    let projected: OrchestrationControlSnapshotV1 = control
        .invoke(
            OrchestrationControlMethod::OrchestrationSnapshot,
            &serde_json::json!({}),
        )
        .await
        .map_err(generic)?;
    if parsed.flags.contains("--json") {
        json_envelope("snapshot", &projected)
    } else {
        Ok(format!("{}\n", status_lines(&projected).join("\n")))
    }
}

async fn agents_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<String, CommandError> {
    let parsed = parse_options(args, &["--after", "--limit"], &["--json"])?;
    if !parsed.positionals.is_empty() {
        return Err(CommandError::Usage(
            "orchestrate agents takes no arguments.".into(),
        ));
    }
    let limit = integer_in_range(parsed.values.get("--limit"), 32, 1, 32, "--limit")?;
    let page: OrchestrationAgentPageV1 = control
        .invoke(
            OrchestrationControlMethod::AgentList,
            &serde_json::json!({
                "afterAgentId": parsed.values.get("--after"),
                "limit": limit,
            }),
        )
        .await
        .map_err(generic)?;
    if parsed.flags.contains("--json") {
        return json_envelope("page", &page);
    }
    let mut lines = agent_lines(&page.agents);
    if page.has_more {
        lines.push(format!(
            "more agents: --after {}",
            page.next_after_agent_id.as_deref().unwrap_or("")
        ));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

async fn inbox_command(
    control: &OrchestrationHostControl,
    args: &[String],
) -> Result<String, CommandError> {
    let parsed = parse_options(args, &["--thread", "--before", "--limit"], &["--json"])?;
    if parsed.positionals.len() != 1 {
        return Err(CommandError::Usage(
            "orchestrate inbox requires one agent id.".into(),
        ));
    }
    let limit = integer_in_range(parsed.values.get("--limit"), 4, 1, 4, "--limit")?;
    let page: OrchestrationInboxPageV1 = control
        .invoke(
            OrchestrationControlMethod::AgentInbox,
            &OrchestrationInboxRequest {
                agent_id: parsed.positionals[0].clone(),
                thread_id: parsed.values.get("--thread").cloned(),
                before_message_id: parsed.values.get("--before").cloned(),
                limit: Some(limit as u64),
            },
        )
        .await
        .map_err(generic)?;
    if parsed.flags.contains("--json") {
        return json_envelope("page", &page);
    }
    let mut lines = message_lines(&page.messages);
    if page.has_more {
        lines.push(format!(
            "more messages: --before {}",
            page.next_before_message_id.as_deref().unwrap_or("")
        ));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

async fn events_command(
    control: &OrchestrationHostControl,
    verb: &str,
    args: &[String],
) -> Result<String, CommandError> {
    let parsed = parse_options(args, &["--since", "--limit"], &["--json"])?;
    if !parsed.positionals.is_empty() {
        return Err(CommandError::Usage(format!(
            "orchestrate {verb} takes options only."
        )));
    }
    let sequence = non_negative(parsed.values.get("--since"), 0, "--since")?;
    let limit = integer_in_range(parsed.values.get("--limit"), 64, 1, 64, "--limit")?;
    let page: OrchestrationEventPageV1 = control
        .invoke(
            OrchestrationControlMethod::EventsSince,
            &OrchestrationTimelineRequest {
                sequence: sequence as u64,
                limit: Some(limit as u64),
            },
        )
        .await
        .map_err(generic)?;
    if parsed.flags.contains("--json") {
        return json_envelope("page", &page);
    }
    let mut lines = Vec::new();
    if page.truncated {
        lines.push(format!(
            "(history compacted through sequence {})",
            page.floor_sequence
        ));
    }
    if page.events.is_empty() {
        lines.push("(no events)".into());
    } else {
        lines.extend(page.events.iter().map(|event| {
            format!(
                "{}\t{}\t{}\t{}",
                event.sequence,
                event.kind,
                event.entity_id,
                for_display(&event.summary)
            )
        }));
    }
    if page.has_more {
        lines.push(format!("more events: --since {}", page.next_sequence));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn status_lines(snapshot: &OrchestrationControlSnapshotV1) -> Vec<String> {
    let bounded = if snapshot.projection.truncated {
        " · bounded projection"
    } else {
        ""
    };
    let mut lines = vec![format!("revision {}{bounded}", snapshot.revision)];
    if snapshot.agents.is_empty() {
        lines.push("agents: (none)".into());
    }
    lines.extend(snapshot.agents.iter().map(|agent| {
        format!(
            "agent {} {} {}{}",
            agent.agent_id,
            json_enum(agent.state),
            agent.engine_id,
            agent
                .role
                .as_ref()
                .map_or_else(String::new, |role| format!(" {}", for_display(role)))
        )
    }));
    lines
}

fn agent_lines(agents: &[OrchestrationControlAgentV1]) -> Vec<String> {
    if agents.is_empty() {
        return vec!["(no orchestrated agents)".into()];
    }
    agents
        .iter()
        .map(|agent| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                agent.agent_id,
                json_enum(agent.state),
                agent.engine_id,
                for_display(agent.role.as_deref().unwrap_or("-")),
                for_display(agent.session_key.as_deref().unwrap_or("-"))
            )
        })
        .collect()
}

fn message_lines(messages: &[MailboxMessageV1]) -> Vec<String> {
    if messages.is_empty() {
        return vec!["(inbox empty)".into()];
    }
    messages
        .iter()
        .map(|message| {
            let receipt = message
                .receipts
                .last()
                .map_or("requested".to_owned(), |receipt| json_enum(receipt.state));
            format!(
                "{}\t{}\t{}\t{}",
                message.message_id,
                receipt,
                message.thread_id,
                for_display(message.body.as_deref().unwrap_or("(body pruned)"))
            )
        })
        .collect()
}

fn non_negative(value: Option<&String>, default: u64, name: &str) -> Result<u64, CommandError> {
    value
        .map_or(Ok(default), |value| value.parse::<u64>())
        .map_err(|_| CommandError::Usage(format!("{name} must be a non-negative integer.")))
}

fn json_envelope<T: serde::Serialize>(key: &str, value: &T) -> Result<String, CommandError> {
    Ok(format!(
        "{{\"schemaVersion\":1,\"{key}\":{}}}\n",
        serde_json::to_string(value).map_err(generic)?
    ))
}

fn json_enum<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn generic(error: impl std::fmt::Display) -> CommandError {
    CommandError::Generic(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use interface_host::{OrchestrationHostControl, prepare_one_shot_orchestration_host};
    use interface_runtime::SessionRuntime;
    use tempfile::TempDir;

    use super::*;

    async fn fixture() -> (TempDir, OrchestrationHostControl, SessionRuntime) {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let data = temp.path().join("data");
        let user_data = temp.path().join("Interface");
        let local_bin = home.join(".local/bin");
        fs::create_dir_all(&local_bin).expect("home bin");
        let codex = local_bin.join("codex");
        fs::write(
            &codex,
            br#"#!/usr/bin/env python3
import json, sys
if sys.argv[1:] == ["--version"]:
    print("codex-cli test")
    raise SystemExit(0)
for line in sys.stdin:
    message = json.loads(line)
    if message.get("id") is None:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {"serverInfo":{"version":"test"}}
    elif method == "model/list":
        result = {"data":[]}
    elif method == "account/read":
        result = {"account":{"type":"test"}}
    else:
        raise AssertionError("unexpected method " + str(method))
    print(json.dumps({"id":message["id"], "result":result}), flush=True)
"#,
        )
        .expect("codex fixture");
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).expect("codex fixture mode");
        let paths = crate::paths::resolve_cli_paths(Some(data.into_os_string()), &home);
        let prepared = prepare_one_shot_orchestration_host(&user_data)
            .await
            .expect("prepare host");
        assert!(prepared.owns_stores());
        let epoch = prepared.runtime_epoch().to_owned();
        let runtime = crate::app::build_runtime(
            &paths,
            &home,
            Some((&user_data, &epoch)),
            None,
            &crate::app::CliExecutableLookup::for_runtime(),
        )
        .expect("runtime");
        let control = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("host");
        (temp, control, runtime)
    }

    #[tokio::test]
    async fn empty_status_agents_inbox_and_events_match_legacy_text() {
        let (temp, control, runtime) = fixture().await;
        let status = run(&["status".into()], &control, false, temp.path()).await;
        assert_eq!(status.exit_code, 0, "{}", status.stderr);
        assert_eq!(status.stdout, "revision 0\nagents: (none)\n");
        let agents = run(&["agents".into()], &control, false, temp.path()).await;
        assert_eq!(agents.stdout, "(no orchestrated agents)\n");
        let inbox = run(
            &["inbox".into(), "agent_1".into()],
            &control,
            false,
            temp.path(),
        )
        .await;
        assert_eq!(inbox.stdout, "(inbox empty)\n");
        let events = run(&["events".into()], &control, false, temp.path()).await;
        assert_eq!(events.stdout, "(no events)\n");
        control.close().await.expect("close");
        drop(runtime);
    }

    #[tokio::test]
    async fn json_envelope_and_fail_closed_provider_exit_are_stable() {
        let (temp, control, runtime) = fixture().await;
        let status = run(
            &["status".into(), "--json".into()],
            &control,
            false,
            temp.path(),
        )
        .await;
        assert_eq!(status.exit_code, 0);
        assert!(
            status
                .stdout
                .starts_with("{\"schemaVersion\":1,\"snapshot\":{")
        );
        let stop = run(
            &["stop".into(), "agent_1".into()],
            &control,
            false,
            temp.path(),
        )
        .await;
        assert_eq!(stop.exit_code, 3);
        assert!(stop.stderr.contains("Start Interface or the iface TUI"));
        let escrow = run(&["worktree-escrow".into()], &control, false, temp.path()).await;
        assert_eq!(escrow.exit_code, CLI_EXIT.unsent);
        control.close().await.expect("close");
        drop(runtime);
    }

    #[tokio::test]
    async fn every_live_verb_keeps_its_legacy_preflight() {
        let (temp, control, runtime) = fixture().await;
        let project = temp.path().join("project");
        fs::create_dir(&project).expect("project");

        for (args, expected) in [
            (vec!["start-agent"], "--engine is required."),
            (vec!["send", "agent_1"], "--body is required."),
            (
                vec!["reply", "agent_1", "--body", "hello"],
                "--parent is required.",
            ),
            (vec!["steer", "agent_1"], "--body is required."),
            (vec!["stop"], "orchestrate stop requires one agent id."),
            (vec!["role-claim"], "role-claim requires one agent id."),
            (
                vec!["role-relinquish"],
                "role-relinquish requires one claim id.",
            ),
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let result = run(&args, &control, true, &project).await;
            assert_eq!(result.exit_code, CLI_EXIT.validation, "{args:?}");
            assert!(
                result.stderr.contains(expected),
                "{args:?}: {}",
                result.stderr
            );
        }
        let escrow = run(
            &[
                "worktree-escrow".into(),
                "lease_1".into(),
                "--source".into(),
                "agent_1".into(),
                "--target".into(),
                "agent_2".into(),
                "--owner-revision".into(),
                "0".into(),
            ],
            &control,
            true,
            &project,
        )
        .await;
        assert_eq!(escrow.exit_code, CLI_EXIT.denial);
        control.close().await.expect("close");
        drop(runtime);
    }
}
