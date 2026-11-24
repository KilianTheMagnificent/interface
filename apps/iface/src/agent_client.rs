use std::{collections::HashSet, path::PathBuf, time::Duration};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    agent::AGENT_USAGE,
    agent_protocol::{
        AGENT_MAX_LINE_BYTES, AGENT_MAX_PROMPT_BYTES, AgentCapabilities, AgentErrorCode,
        AgentMethod, AgentProtocolError, AgentRequest, AgentResponsePayload, AgentSnapshot,
        AgentStatus, AgentTaskMetadata, decode_agent_response, encode_agent_request, is_agent_name,
    },
    argv::{LaunchScope, is_engine_id, parse_duration_ms},
    errors::{CLI_EXIT, CommandResult, DeliveryOutcome, exit_code_for_send},
    sanitize::for_display,
};

pub const AGENT_MAILBOX_MAX_PAGE: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentOutput {
    Text,
    Json,
}

const fn default_output(method: AgentMethod) -> AgentOutput {
    match method {
        AgentMethod::List | AgentMethod::Get | AgentMethod::Read => AgentOutput::Text,
        _ => AgentOutput::Json,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
enum AgentArgv {
    Help,
    Command(AgentCommand),
}

/// One parsed `iface agent` verb with every option it may carry.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentCommand {
    verb: AgentMethod,
    output: AgentOutput,
    positionals: Vec<String>,
    lines: u64,
    status: Option<AgentStatus>,
    timeout_ms: Option<u64>,
    after: u64,
    limit: u64,
    engine: Option<String>,
    cwd: Option<PathBuf>,
    model: Option<String>,
    effort: Option<String>,
    scope: Option<LaunchScope>,
    prompt: Option<String>,
    brief: Option<String>,
    task: Option<AgentTaskMetadata>,
    sender: Option<String>,
    message_id: Option<String>,
    reply_to: Option<String>,
    expect_reply: bool,
    clear: bool,
    live: bool,
}

impl AgentCommand {
    /// `verb` with no options given: 80 read lines, a 50-message page.
    fn new(verb: AgentMethod) -> Self {
        Self {
            verb,
            output: default_output(verb),
            positionals: Vec::new(),
            lines: 80,
            status: None,
            timeout_ms: None,
            after: 0,
            limit: 50,
            engine: None,
            cwd: None,
            model: None,
            effort: None,
            scope: None,
            prompt: None,
            brief: None,
            task: None,
            sender: None,
            message_id: None,
            reply_to: None,
            expect_reply: false,
            clear: false,
            live: false,
        }
    }
}

pub type AgentCommandResult = CommandResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentStdinReadError {
    TooLarge,
    InvalidUtf8,
}

pub(crate) fn render_stdin_read_error(
    args: &[String],
    error: AgentStdinReadError,
) -> AgentCommandResult {
    let Ok(AgentArgv::Command(AgentCommand { verb, output, .. })) = parse_agent_argv(args) else {
        return local_error(args, "agent stdin could not be read");
    };
    let operation = verb.verb();
    let message = match error {
        AgentStdinReadError::TooLarge => {
            format!("agent {operation} stdin is limited to 64 KiB of UTF-8")
        }
        AgentStdinReadError::InvalidUtf8 => {
            format!("agent {operation} stdin must be valid UTF-8")
        }
    };
    render_failure(verb, output, None, &AgentProtocolError::invalid(message))
}

pub async fn run_agent_command(
    args: &[String],
    socket_path: &std::path::Path,
    stdin: &str,
) -> AgentCommandResult {
    let parsed = match parse_agent_argv(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            return local_error(args, &error);
        }
    };
    if parsed == AgentArgv::Help {
        return AgentCommandResult {
            stdout: format!("{AGENT_USAGE}\n"),
            stderr: String::new(),
            exit_code: CLI_EXIT.success,
        };
    }
    let AgentArgv::Command(AgentCommand { verb, output, .. }) = &parsed else {
        unreachable!("help returned above")
    };
    let request = match request_for(&parsed, stdin) {
        Ok(request) => request,
        Err(error) => {
            return render_failure(*verb, *output, None, &error);
        }
    };
    match rpc_within(socket_path, &request, reply_deadline(*verb)).await {
        Ok(result) => {
            let rendered = if *output == AgentOutput::Json {
                serde_json::to_string(&result)
                    .map(|value| format!("{value}\n"))
                    .map_err(|_| {
                        AgentProtocolError::invalid("Agent response could not be encoded.")
                    })
            } else {
                render_text(*verb, &result)
            };
            match rendered {
                Ok(stdout) => AgentCommandResult {
                    exit_code: if matches!(verb, AgentMethod::Send | AgentMethod::Steer) {
                        send_exit_code(&result).unwrap_or(CLI_EXIT.generic)
                    } else {
                        CLI_EXIT.success
                    },
                    stdout,
                    stderr: String::new(),
                },
                Err(error) => render_failure(*verb, *output, target_for(&parsed), &error),
            }
        }
        Err(failure) => render_rpc_failure(*verb, *output, target_for(&parsed), failure),
    }
}

/// A failed round trip to the TUI, and how far the request got.
#[derive(Debug)]
enum RpcFailure {
    /// The TUI never received a whole request line: nothing was listening,
    /// or the write failed before its closing newline, which the server
    /// requires before it reads a frame.
    NotSent(AgentProtocolError),
    /// The whole request was written, then the TUI went away without
    /// replying. It may have acted on the request.
    ReplyLost(AgentProtocolError),
    /// Anything else: the TUI's error answer, a malformed answer, a request
    /// that could not be encoded, or a read-only verb's reply deadline.
    Failed(AgentProtocolError),
}

impl RpcFailure {
    fn into_error(self) -> AgentProtocolError {
        match self {
            Self::NotSent(error) | Self::ReplyLost(error) | Self::Failed(error) => error,
        }
    }
}

/// Renders a failed round trip. Read-only verbs (and the other mutating
/// verbs) render as before; for a send, steer or start, a TUI that never
/// got the request is definitely-unsent and one that went away after
/// getting it is outcome-unknown, which must not be retried blindly.
fn render_rpc_failure(
    verb: AgentMethod,
    output: AgentOutput,
    target: Option<&str>,
    failure: RpcFailure,
) -> AgentCommandResult {
    let delivers = matches!(
        verb,
        AgentMethod::Send | AgentMethod::Steer | AgentMethod::Start
    );
    match failure {
        RpcFailure::NotSent(error) if delivers => render_failure_as(
            verb,
            output,
            target,
            &error,
            FailureDisposition {
                exit_code: CLI_EXIT.unsent,
                retryable: true,
                hint: error_hint(error.code),
            },
        ),
        RpcFailure::ReplyLost(error) if delivers => render_failure_as(
            verb,
            output,
            target,
            &error,
            FailureDisposition {
                exit_code: CLI_EXIT.unknown,
                retryable: false,
                hint: if verb == AgentMethod::Start {
                    "The TUI may have started it; check `iface agent list` before starting it again."
                } else {
                    "The TUI may have acted on it; check `iface agent inbox` or `iface agent get` before resending, or resend with the same --message-id."
                },
            },
        ),
        failure => render_failure(verb, output, target, &failure.into_error()),
    }
}

/// How long a read-only verb waits for the TUI's reply before reporting a
/// retryable timeout instead of hanging on a wedged TUI.
const AGENT_READ_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// The reply deadline for `verb`. Only verbs that change nothing get one:
/// cutting a mutating verb short would leave its outcome unknown, and
/// `monitor` and `wait` carry their own `--timeout`.
const fn reply_deadline(verb: AgentMethod) -> Option<Duration> {
    match verb {
        AgentMethod::Commands
        | AgentMethod::List
        | AgentMethod::Get
        | AgentMethod::Read
        | AgentMethod::Inbox => Some(AGENT_READ_REPLY_TIMEOUT),
        _ => None,
    }
}

async fn rpc_within(
    socket_path: &std::path::Path,
    request: &AgentRequest,
    deadline: Option<Duration>,
) -> Result<Value, RpcFailure> {
    let Some(deadline) = deadline else {
        return round_trip(socket_path, request).await;
    };
    tokio::time::timeout(deadline, round_trip(socket_path, request))
        .await
        .unwrap_or_else(|_| {
            Err(RpcFailure::Failed(AgentProtocolError::new(
                AgentErrorCode::Timeout,
                "The iface TUI did not reply in time.",
            )))
        })
}

/// Sends one request to the running iface TUI over its private Unix socket.
///
/// # Errors
///
/// Returns a structured protocol error when the TUI is unavailable, the
/// request or response is invalid, or the server returns an agent error.
pub async fn agent_rpc(
    socket_path: &std::path::Path,
    request: &AgentRequest,
) -> Result<Value, AgentProtocolError> {
    round_trip(socket_path, request)
        .await
        .map_err(RpcFailure::into_error)
}

async fn round_trip(
    socket_path: &std::path::Path,
    request: &AgentRequest,
) -> Result<Value, RpcFailure> {
    let not_running =
        || AgentProtocolError::new(AgentErrorCode::NotRunning, "The iface TUI is not running.");
    let stopped = || {
        RpcFailure::ReplyLost(AgentProtocolError::new(
            AgentErrorCode::NotRunning,
            "The iface TUI stopped before replying.",
        ))
    };
    let mut socket = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|_| RpcFailure::NotSent(not_running()))?;
    let request_line = encode_agent_request(request).map_err(RpcFailure::Failed)?;
    socket
        .write_all(&request_line)
        .await
        .map_err(|_| RpcFailure::NotSent(not_running()))?;
    // From here the whole line, newline included, is in the TUI's socket.
    socket.shutdown().await.map_err(|_| stopped())?;
    let mut bytes = Vec::new();
    socket
        .take((AGENT_MAX_LINE_BYTES + 2) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| stopped())?;
    if bytes.is_empty() {
        return Err(stopped());
    }
    if bytes.len() > AGENT_MAX_LINE_BYTES + 1 {
        return Err(RpcFailure::Failed(AgentProtocolError::invalid(
            "Agent response exceeds the 1 MiB limit.",
        )));
    }
    let response = decode_agent_response(&bytes).map_err(RpcFailure::Failed)?;
    if response.id != request.id {
        return Err(RpcFailure::Failed(AgentProtocolError::invalid(
            "Agent response id does not match the request.",
        )));
    }
    match response.payload {
        AgentResponsePayload::Result(result) => Ok(result),
        AgentResponsePayload::Error(error) => Err(RpcFailure::Failed(AgentProtocolError {
            code: error.code,
            message: error.message,
            data: error.data,
        })),
    }
}

#[must_use]
pub fn agent_command_uses_stdin(args: &[String]) -> bool {
    parse_agent_argv(args).is_ok_and(|parsed| match parsed {
        AgentArgv::Command(AgentCommand {
            verb: AgentMethod::Send | AgentMethod::Steer,
            ref positionals,
            ..
        }) => positionals.get(1).is_some_and(|value| value == "-"),
        AgentArgv::Command(AgentCommand {
            verb: AgentMethod::Start,
            ref prompt,
            ref brief,
            ..
        }) => prompt.as_deref() == Some("-") || brief.as_deref() == Some("-"),
        _ => false,
    })
}

#[allow(clippy::too_many_lines)]
fn parse_agent_argv(args: &[String]) -> Result<AgentArgv, String> {
    let first = args
        .first()
        .ok_or_else(|| "iface agent requires a verb".to_owned())?;
    if first == "--help" {
        return Ok(AgentArgv::Help);
    }
    let verb =
        AgentMethod::from_verb(first).ok_or_else(|| format!("Unknown agent verb: {first}"))?;
    let mut command = AgentCommand::new(verb);
    let mut seen = HashSet::new();
    let mut task = AgentTaskMetadata::default();
    let mut help = false;
    let mut index = 1;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            command.positionals.extend_from_slice(&args[index + 1..]);
            break;
        }
        if token == "-" || !token.starts_with('-') {
            command.positionals.push(token.clone());
            index += 1;
            continue;
        }
        if !seen.insert(token.clone()) {
            return Err(format!("{token} may only be specified once"));
        }
        match token.as_str() {
            "--help" => help = true,
            "--clear" => command.clear = true,
            "--expect-reply" => command.expect_reply = true,
            "--live" => command.live = true,
            "--output" => {
                command.output = match option_value(args, index, token)?.as_str() {
                    "text" => AgentOutput::Text,
                    "json" => AgentOutput::Json,
                    _ => return Err("--output must be text or json".into()),
                };
                index += 1;
            }
            "--lines" => {
                command.lines = positive_integer(&option_value(args, index, token)?, "--lines")?;
                index += 1;
            }
            "--status" => {
                command.status = Some(
                    AgentStatus::parse_waitable(&option_value(args, index, token)?)
                        .ok_or("--status must be idle, working, or blocked")?,
                );
                index += 1;
            }
            "--timeout" => {
                command.timeout_ms = Some(
                    parse_duration_ms(&option_value(args, index, token)?)
                        .map_err(|error| error.to_string())?,
                );
                index += 1;
            }
            "--after" => {
                let value = option_value(args, index, token)?;
                command.after = value
                    .parse::<u64>()
                    .map_err(|_| "--after must be a non-negative integer".to_owned())?;
                index += 1;
            }
            "--limit" => {
                command.limit = positive_integer(&option_value(args, index, token)?, "--limit")?;
                if command.limit > AGENT_MAILBOX_MAX_PAGE {
                    return Err(format!(
                        "--limit must be an integer from 1 to {AGENT_MAILBOX_MAX_PAGE}"
                    ));
                }
                index += 1;
            }
            "--engine" => {
                let value = option_value(args, index, token)?;
                if !is_engine_id(&value) {
                    return Err("--engine must be claude, codex, or acp:<slug>".into());
                }
                command.engine = Some(value);
                index += 1;
            }
            "--cwd" => {
                command.cwd = Some(
                    std::path::absolute(option_value(args, index, token)?)
                        .map_err(|error| error.to_string())?,
                );
                index += 1;
            }
            "--model" => {
                command.model = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--effort" => {
                command.effort = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--scope" => {
                command.scope = Some(
                    LaunchScope::parse(&option_value(args, index, token)?)
                        .ok_or("--scope must be all, project, or isolated")?,
                );
                index += 1;
            }
            "--prompt" => {
                command.prompt = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--brief" => {
                command.brief = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--plan-id" => {
                task.plan_id = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--task-id" => {
                task.task_id = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--parent-session" => {
                task.parent_session_key = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--role" => {
                task.role = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--task-scope" => {
                task.scope = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--brief-hash" => {
                task.brief_hash = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--sender" => {
                command.sender = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--message-id" => {
                command.message_id = Some(option_value(args, index, token)?);
                index += 1;
            }
            "--reply-to" => {
                command.reply_to = Some(option_value(args, index, token)?);
                index += 1;
            }
            _ => return Err(format!("Unknown option: {token}")),
        }
        index += 1;
    }
    if help {
        return Ok(AgentArgv::Help);
    }
    command.task = (task != AgentTaskMetadata::default()).then_some(task);
    validate_agent_options(&command, &seen)?;
    validate_agent_positionals(&command)?;
    let positionals = &command.positionals;
    if verb == AgentMethod::Start && !is_agent_name(&positionals[0]) {
        return Err(format!("invalid agent name: {}", positionals[0]));
    }
    if verb == AgentMethod::Rename && !command.clear && !is_agent_name(&positionals[1]) {
        return Err(format!("invalid agent name: {}", positionals[1]));
    }
    Ok(AgentArgv::Command(command))
}

/// Refuses an option the verb does not take. `seen` holds every option
/// given, which is how `--lines`, `--after` and `--limit` (always set to a
/// default) show they were given.
fn validate_agent_options(command: &AgentCommand, seen: &HashSet<String>) -> Result<(), String> {
    let AgentCommand {
        verb,
        status,
        timeout_ms: timeout,
        ref engine,
        ref cwd,
        ref model,
        ref effort,
        scope,
        ref prompt,
        ref brief,
        ref task,
        ref sender,
        ref message_id,
        ref reply_to,
        expect_reply,
        clear,
        live,
        ..
    } = *command;
    if seen.contains("--lines") && verb != AgentMethod::Read {
        return Err("--lines is only valid with read".into());
    }
    if status.is_some() && verb != AgentMethod::Wait {
        return Err("--status is only valid with wait".into());
    }
    if status.is_none() && verb == AgentMethod::Wait {
        return Err("wait requires --status".into());
    }
    if timeout.is_some() && !matches!(verb, AgentMethod::Wait | AgentMethod::Monitor) {
        return Err("--timeout is only valid with wait or monitor".into());
    }
    if (seen.contains("--after") || seen.contains("--limit"))
        && !matches!(verb, AgentMethod::Inbox | AgentMethod::Monitor)
    {
        return Err("--after and --limit are only valid with inbox or monitor".into());
    }
    if (sender.is_some() || message_id.is_some() || reply_to.is_some() || expect_reply)
        && !matches!(verb, AgentMethod::Send | AgentMethod::Steer)
    {
        return Err("message correlation options are only valid with send or steer".into());
    }
    if (reply_to.is_some() || expect_reply) && sender.is_none() {
        return Err("--sender is required with --reply-to or --expect-reply".into());
    }
    if (engine.is_some()
        || cwd.is_some()
        || model.is_some()
        || effort.is_some()
        || scope.is_some()
        || prompt.is_some()
        || brief.is_some()
        || task.is_some())
        && verb != AgentMethod::Start
    {
        return Err("agent launch and task metadata options are only valid with start".into());
    }
    if prompt.is_some() && brief.is_some() {
        return Err("start cannot combine --prompt with --brief".into());
    }
    if clear && verb != AgentMethod::Rename {
        return Err("--clear is only valid with rename".into());
    }
    if live && verb != AgentMethod::List {
        return Err("--live is only valid with list".into());
    }
    Ok(())
}

fn validate_agent_positionals(command: &AgentCommand) -> Result<(), String> {
    let AgentCommand {
        verb,
        ref positionals,
        clear,
        ..
    } = *command;
    match verb {
        AgentMethod::Commands | AgentMethod::List if !positionals.is_empty() => {
            Err(format!("Unexpected argument: {}", positionals[0]))
        }
        AgentMethod::Commands | AgentMethod::List => Ok(()),
        AgentMethod::Get
        | AgentMethod::Resume
        | AgentMethod::Focus
        | AgentMethod::Stop
        | AgentMethod::Read
        | AgentMethod::Inbox
        | AgentMethod::Monitor
        | AgentMethod::Wait => {
            if positionals.is_empty() {
                return Err(format!("{} requires a target", verb.verb()));
            }
            if positionals.len() > 1 {
                return Err(format!("Unexpected argument: {}", positionals[1]));
            }
            Ok(())
        }
        AgentMethod::Send | AgentMethod::Steer => {
            let name = verb.verb();
            if positionals.is_empty() {
                return Err(format!("{name} requires a target"));
            }
            if positionals.len() == 1 {
                return Err(format!("{name} requires text or - for stdin"));
            }
            Ok(())
        }
        AgentMethod::Start => {
            if positionals.is_empty() {
                return Err("start requires an agent name".into());
            }
            if positionals.len() > 1 {
                return Err(format!("Unexpected argument: {}", positionals[1]));
            }
            Ok(())
        }
        AgentMethod::Rename => {
            if positionals.is_empty() {
                return Err("rename requires a target".into());
            }
            if clear && positionals.len() > 1 {
                return Err("rename cannot combine a name with --clear".into());
            }
            if !clear && positionals.len() < 2 {
                return Err("rename requires a name or --clear".into());
            }
            if positionals.len() > 2 {
                return Err(format!("Unexpected argument: {}", positionals[2]));
            }
            Ok(())
        }
    }
}

#[allow(clippy::too_many_lines)]
fn request_for(parsed: &AgentArgv, stdin: &str) -> Result<AgentRequest, AgentProtocolError> {
    let AgentArgv::Command(AgentCommand {
        verb,
        positionals,
        lines,
        status,
        timeout_ms,
        after,
        limit,
        engine,
        cwd,
        model,
        effort,
        scope,
        prompt,
        brief,
        task,
        sender,
        message_id,
        reply_to,
        expect_reply,
        clear,
        live,
        ..
    }) = parsed
    else {
        unreachable!("help has no request")
    };
    let mut params = Map::new();
    match verb {
        AgentMethod::Commands => {}
        // Sent only when asked, so a plain list still reaches a TUI that
        // predates the parameter.
        AgentMethod::List => {
            if *live {
                params.insert("live".into(), Value::Bool(true));
            }
        }
        AgentMethod::Get | AgentMethod::Stop | AgentMethod::Resume | AgentMethod::Focus => {
            params.insert("target".into(), json!(positionals[0]));
        }
        AgentMethod::Read => {
            params.insert("target".into(), json!(positionals[0]));
            params.insert("lines".into(), json!(lines));
        }
        AgentMethod::Send | AgentMethod::Steer => {
            params.insert("target".into(), json!(positionals[0]));
            let text = positionals[1..].join(" ");
            params.insert("text".into(), json!(stdin_value(&text, stdin, *verb)?));
            insert_option(&mut params, "sender", sender.as_ref());
            insert_option(&mut params, "messageId", message_id.as_ref());
            insert_option(&mut params, "replyTo", reply_to.as_ref());
            if *expect_reply {
                params.insert("expectReply".into(), Value::Bool(true));
            }
        }
        AgentMethod::Inbox | AgentMethod::Monitor => {
            params.insert("target".into(), json!(positionals[0]));
            params.insert("after".into(), json!(after));
            params.insert("limit".into(), json!(limit));
            if let Some(timeout) = timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout));
            }
        }
        AgentMethod::Wait => {
            params.insert("target".into(), json!(positionals[0]));
            params.insert(
                "status".into(),
                serde_json::to_value(status.expect("wait status validated"))
                    .map_err(|_| AgentProtocolError::invalid("invalid wait status"))?,
            );
            if let Some(timeout) = timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout));
            }
        }
        AgentMethod::Start => {
            params.insert("name".into(), json!(positionals[0]));
            if let Some(value) = prompt {
                params.insert("prompt".into(), json!(stdin_value(value, stdin, *verb)?));
            }
            if let Some(value) = brief {
                params.insert("brief".into(), json!(stdin_value(value, stdin, *verb)?));
            }
            insert_option(&mut params, "engine", engine.as_ref());
            if let Some(value) = cwd {
                params.insert("cwd".into(), json!(value.to_string_lossy()));
            }
            insert_option(&mut params, "model", model.as_ref());
            insert_option(&mut params, "effort", effort.as_ref());
            if let Some(value) = scope {
                params.insert("scope".into(), json!(value.as_str()));
            }
            if let Some(value) = task {
                params.insert(
                    "task".into(),
                    serde_json::to_value(value)
                        .map_err(|_| AgentProtocolError::invalid("invalid task metadata"))?,
                );
            }
        }
        AgentMethod::Rename => {
            params.insert("target".into(), json!(positionals[0]));
            params.insert("clear".into(), json!(clear));
            if !clear {
                params.insert("name".into(), json!(positionals[1]));
            }
        }
    }
    Ok(AgentRequest {
        id: Uuid::new_v4().to_string(),
        method: *verb,
        params,
    })
}

fn render_text(verb: AgentMethod, result: &Value) -> Result<String, AgentProtocolError> {
    match verb {
        AgentMethod::Commands => Ok(format!("{}\n", for_display(&result.to_string()))),
        AgentMethod::List => {
            let rows = result.as_array().ok_or_else(|| {
                AgentProtocolError::invalid("Agent list response must be an array.")
            })?;
            if rows.is_empty() {
                return Ok(String::new());
            }
            let rendered = rows
                .iter()
                .map(snapshot_from)
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .map(snapshot_text)
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!("{rendered}\n"))
        }
        AgentMethod::Read => {
            let lines = result
                .get("lines")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AgentProtocolError::invalid("Agent read response has an invalid shape.")
                })?;
            let lines = lines
                .iter()
                .map(|line| {
                    line.as_str().map(for_display).ok_or_else(|| {
                        AgentProtocolError::invalid(
                            "Agent read response lines have an invalid shape.",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            })
        }
        AgentMethod::Send | AgentMethod::Steer => {
            let snapshot = result.get("snapshot").ok_or_else(|| {
                AgentProtocolError::invalid("Agent send response has an invalid shape.")
            })?;
            let snapshot = snapshot_from(snapshot)?;
            let prefix = if result.get("queued") == Some(&Value::Bool(true)) {
                "queued"
            } else {
                "sent"
            };
            Ok(format!("{prefix}\t{}\n", snapshot_text(&snapshot)))
        }
        AgentMethod::Inbox | AgentMethod::Monitor => {
            let messages = result
                .get("messages")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AgentProtocolError::invalid("Agent inbox response has an invalid shape.")
                })?;
            if messages.is_empty() {
                Ok(String::new())
            } else {
                Ok(format!(
                    "{}\n",
                    messages
                        .iter()
                        .map(|message| for_display(&message.to_string()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        }
        _ => Ok(format!("{}\n", snapshot_text(&snapshot_from(result)?))),
    }
}

fn snapshot_from(value: &Value) -> Result<AgentSnapshot, AgentProtocolError> {
    let object = value
        .as_object()
        .ok_or_else(|| AgentProtocolError::invalid("Agent response did not contain a snapshot."))?;
    for required in [
        "name",
        "key",
        "engine",
        "sessionId",
        "status",
        "cwd",
        "title",
        "live",
    ] {
        if !object.contains_key(required) {
            return Err(AgentProtocolError::invalid(
                "Agent response did not contain a snapshot.",
            ));
        }
    }
    let invalid = || AgentProtocolError::invalid("Agent response snapshot has an invalid shape.");
    let name = match object.get("name") {
        Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        _ => return Err(invalid()),
    };
    let string = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let status =
        serde_json::from_value::<AgentStatus>(object.get("status").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let live = object
        .get("live")
        .and_then(Value::as_bool)
        .ok_or_else(invalid)?;
    let model = match object.get("model") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(invalid()),
    };
    let task = object.get("task").map(task_metadata_from).transpose()?;
    let capabilities = object.get("capabilities").map_or_else(
        || Ok(AgentCapabilities::defaults(live, status)),
        capabilities_from,
    )?;
    Ok(AgentSnapshot {
        name,
        key: string("key")?,
        engine: string("engine")?,
        session_id: string("sessionId")?,
        status,
        cwd: string("cwd")?,
        title: string("title")?,
        model,
        task,
        capabilities,
        live,
    })
}

fn capabilities_from(value: &Value) -> Result<AgentCapabilities, AgentProtocolError> {
    let invalid =
        || AgentProtocolError::invalid("Agent response capabilities have an invalid shape.");
    // Borrow the response value instead of deep-cloning the subtree into from_value.
    let capabilities = AgentCapabilities::deserialize(value).map_err(|_| invalid())?;
    if !capabilities.reference || !capabilities.inbox || !capabilities.monitor {
        return Err(invalid());
    }
    Ok(capabilities)
}

fn task_metadata_from(value: &Value) -> Result<AgentTaskMetadata, AgentProtocolError> {
    let invalid =
        || AgentProtocolError::invalid("Agent response task metadata has an invalid shape.");
    let object = value.as_object().ok_or_else(invalid)?;
    if object
        .values()
        .any(|field| field.as_str().is_none_or(str::is_empty))
    {
        return Err(invalid());
    }
    AgentTaskMetadata::deserialize(value).map_err(|_| invalid())
}

fn snapshot_text(snapshot: &AgentSnapshot) -> String {
    let session_id = for_display(&snapshot.session_id);
    [
        snapshot.name.as_deref().unwrap_or("-").to_owned(),
        snapshot.status.as_str().to_owned(),
        snapshot.engine.clone(),
        session_id.chars().take(8).collect(),
        for_display(&snapshot.cwd),
        for_display(&snapshot.title),
    ]
    .join("\t")
}

fn send_exit_code(result: &Value) -> Option<u8> {
    match result.get("delivery")?.get("kind")?.as_str()? {
        "adapter-accepted" => Some(exit_code_for_send(DeliveryOutcome::AdapterAccepted)),
        "definitely-unsent" => Some(exit_code_for_send(DeliveryOutcome::DefinitelyUnsent)),
        "outcome-unknown" => Some(exit_code_for_send(DeliveryOutcome::OutcomeUnknown)),
        _ => None,
    }
}

/// The exit code, `retryable` flag and hint a failure renders with.
#[derive(Clone, Copy)]
struct FailureDisposition {
    exit_code: u8,
    retryable: bool,
    hint: &'static str,
}

fn render_failure(
    verb: AgentMethod,
    output: AgentOutput,
    target: Option<&str>,
    error: &AgentProtocolError,
) -> AgentCommandResult {
    let disposition = FailureDisposition {
        exit_code: match error.code {
            AgentErrorCode::Ambiguous | AgentErrorCode::InvalidRequest => CLI_EXIT.validation,
            AgentErrorCode::Timeout => CLI_EXIT.timeout,
            _ => CLI_EXIT.generic,
        },
        retryable: matches!(
            error.code,
            AgentErrorCode::NotRunning | AgentErrorCode::Timeout
        ),
        hint: error_hint(error.code),
    };
    render_failure_as(verb, output, target, error, disposition)
}

fn render_failure_as(
    verb: AgentMethod,
    output: AgentOutput,
    target: Option<&str>,
    error: &AgentProtocolError,
    disposition: FailureDisposition,
) -> AgentCommandResult {
    let FailureDisposition {
        exit_code,
        retryable,
        hint,
    } = disposition;
    let stderr = if output == AgentOutput::Json {
        let kind = match error.code {
            AgentErrorCode::Ambiguous | AgentErrorCode::InvalidRequest => "usage",
            AgentErrorCode::NotRunning | AgentErrorCode::Timeout => "runtime",
            _ => "session",
        };
        let mut failure = Map::new();
        failure.insert("kind".into(), json!(kind));
        failure.insert("operation".into(), json!(verb.wire_name()));
        if let Some(target) = target {
            failure.insert("target".into(), json!(target));
        }
        failure.insert("hint".into(), json!(hint));
        failure.insert("retryable".into(), json!(retryable));
        failure.insert("message".into(), json!(for_display(&error.message)));
        format!("{}\n", Value::Object(failure))
    } else {
        let mut text = format!("iface: {}\n", for_display(&error.message));
        if error.code == AgentErrorCode::Ambiguous {
            if let Some(candidates) = error
                .data
                .as_ref()
                .and_then(|data| data.get("candidates"))
                .and_then(Value::as_array)
            {
                for candidate in candidates.iter().filter_map(Value::as_str) {
                    text.push_str(&for_display(candidate));
                    text.push('\n');
                }
            }
        }
        // An unknown outcome must reach the user with what to check.
        if exit_code == CLI_EXIT.unknown {
            text.push_str(hint);
            text.push('\n');
        }
        text
    };
    AgentCommandResult {
        stdout: String::new(),
        stderr,
        exit_code,
    }
}

fn local_error(args: &[String], message: &str) -> AgentCommandResult {
    let json_output = args
        .windows(2)
        .take_while(|pair| pair[0] != "--")
        .any(|pair| pair == ["--output", "json"]);
    if json_output {
        let operation = args.first().map_or("agent".to_owned(), |verb| {
            if verb.starts_with('-') {
                "agent".to_owned()
            } else {
                format!("agent.{verb}")
            }
        });
        AgentCommandResult {
            stdout: String::new(),
            stderr: format!(
                "{}\n",
                json!({
                    "kind": "usage",
                    "operation": operation,
                    "hint": "Correct the command arguments and retry.",
                    "retryable": false,
                    "message": for_display(message),
                })
            ),
            exit_code: CLI_EXIT.validation,
        }
    } else {
        AgentCommandResult {
            stdout: String::new(),
            stderr: format!("iface: {}\n", for_display(message)),
            exit_code: CLI_EXIT.validation,
        }
    }
}

fn option_value(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index + 1)
        .filter(|value| !value.starts_with("--") && !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn positive_integer(value: &str, flag: &str) -> Result<u64, String> {
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("{flag} must be a positive integer"));
    }
    value
        .parse()
        .map_err(|_| format!("{flag} must be a positive integer"))
}

fn stdin_value(value: &str, stdin: &str, verb: AgentMethod) -> Result<String, AgentProtocolError> {
    if value != "-" {
        return Ok(value.to_owned());
    }
    if stdin.len() > AGENT_MAX_PROMPT_BYTES {
        return Err(AgentProtocolError::invalid(format!(
            "agent {} stdin is limited to 64 KiB of UTF-8",
            verb.verb()
        )));
    }
    Ok(stdin.to_owned())
}

fn insert_option(map: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        map.insert(key.into(), Value::String(value.clone()));
    }
}

fn target_for(parsed: &AgentArgv) -> Option<&str> {
    let AgentArgv::Command(AgentCommand {
        verb, positionals, ..
    }) = parsed
    else {
        return None;
    };
    match verb {
        AgentMethod::Commands | AgentMethod::List => None,
        _ => positionals.first().map(String::as_str),
    }
}

const fn error_hint(code: AgentErrorCode) -> &'static str {
    match code {
        AgentErrorCode::Ambiguous => "Use a durable name or a longer unique session-id prefix.",
        AgentErrorCode::NotRunning => "Start the iface TUI, then retry the command.",
        AgentErrorCode::NotLive => "Resume the session explicitly before using this verb.",
        AgentErrorCode::Blocked => "Resolve the pending approval in the TUI before sending.",
        AgentErrorCode::Failed => "Inspect the failed session in the TUI.",
        AgentErrorCode::NameTaken => "Choose another durable agent name.",
        AgentErrorCode::Unsupported => {
            "Inspect the agent capability facets and choose a supported operation."
        }
        AgentErrorCode::Timeout => "Retry with a longer timeout or inspect the session.",
        AgentErrorCode::NotFound => "Check `iface agent list` and use a listed name or id prefix.",
        AgentErrorCode::InvalidRequest => "Correct the command arguments and retry.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[tokio::test]
    async fn only_verbs_that_change_nothing_time_out_on_a_silent_tui() {
        for verb in [
            AgentMethod::Commands,
            AgentMethod::List,
            AgentMethod::Get,
            AgentMethod::Read,
            AgentMethod::Inbox,
        ] {
            assert_eq!(reply_deadline(verb), Some(AGENT_READ_REPLY_TIMEOUT));
        }
        for verb in [
            AgentMethod::Send,
            AgentMethod::Steer,
            AgentMethod::Stop,
            AgentMethod::Resume,
            AgentMethod::Start,
            AgentMethod::Rename,
            AgentMethod::Focus,
            AgentMethod::Monitor,
            AgentMethod::Wait,
        ] {
            assert_eq!(reply_deadline(verb), None);
        }
        let temp = tempfile::tempdir().expect("temp");
        let socket = temp.path().join("iface.sock");
        // Bound but never answered, like a wedged TUI.
        let _listener = tokio::net::UnixListener::bind(&socket).expect("bind");
        let request = AgentRequest {
            id: "silent".into(),
            method: AgentMethod::List,
            params: Map::new(),
        };
        let error = rpc_within(&socket, &request, Some(Duration::from_millis(50)))
            .await
            .expect_err("timed out")
            .into_error();
        assert_eq!(error.code, AgentErrorCode::Timeout);
        assert_eq!(error.message, "The iface TUI did not reply in time.");
    }

    /// A TUI that takes the whole request line, then exits without replying.
    fn tui_that_exits_after_reading(socket: &std::path::Path) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(socket).expect("bind");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.expect("request");
                assert!(request.ends_with(b"\n"), "the whole line arrived");
            }
        })
    }

    fn json_failure(result: &AgentCommandResult) -> Value {
        serde_json::from_str(result.stderr.trim()).expect("JSON failure")
    }

    #[tokio::test]
    async fn a_send_whose_reply_is_lost_after_the_write_is_outcome_unknown_not_retryable() {
        let temp = tempfile::tempdir().expect("temp");
        let socket = temp.path().join("iface.sock");
        let tui = tui_that_exits_after_reading(&socket);

        for (verb_args, hint) in [
            (&["send", "bob", "hi"][..], "--message-id"),
            (&["steer", "bob", "hi"][..], "--message-id"),
            (&["start", "bob"][..], "iface agent list"),
        ] {
            let mut argv = args(verb_args);
            argv.extend(args(&["--output", "json"]));
            let result = run_agent_command(&argv, &socket, "").await;
            assert_eq!(result.exit_code, CLI_EXIT.unknown, "{verb_args:?}");
            let failure = json_failure(&result);
            assert_eq!(failure["retryable"], json!(false), "{verb_args:?}");
            assert!(
                failure["hint"].as_str().expect("hint").contains(hint),
                "{verb_args:?}: {failure}"
            );
        }
        let text = run_agent_command(
            &args(&["send", "bob", "hi", "--output", "text"]),
            &socket,
            "",
        )
        .await;
        assert_eq!(text.exit_code, CLI_EXIT.unknown);
        assert!(
            text.stderr.contains("check `iface agent inbox`"),
            "{}",
            text.stderr
        );

        // Read-only verbs keep the old retryable runtime failure.
        let list = run_agent_command(&args(&["list", "--output", "json"]), &socket, "").await;
        assert_eq!(list.exit_code, CLI_EXIT.generic);
        assert_eq!(json_failure(&list)["retryable"], json!(true));
        tui.abort();
    }

    #[tokio::test]
    async fn a_send_with_no_tui_listening_is_definitely_unsent_and_retryable() {
        let temp = tempfile::tempdir().expect("temp");
        let socket = temp.path().join("iface.sock");

        for verb_args in [
            &["send", "bob", "hi"][..],
            &["steer", "bob", "hi"][..],
            &["start", "bob"][..],
        ] {
            let mut argv = args(verb_args);
            argv.extend(args(&["--output", "json"]));
            let result = run_agent_command(&argv, &socket, "").await;
            assert_eq!(result.exit_code, CLI_EXIT.unsent, "{verb_args:?}");
            let failure = json_failure(&result);
            assert_eq!(failure["retryable"], json!(true), "{verb_args:?}");
            assert_eq!(failure["kind"], json!("runtime"));
        }
        let list = run_agent_command(&args(&["list", "--output", "json"]), &socket, "").await;
        assert_eq!(list.exit_code, CLI_EXIT.generic);
        assert_eq!(json_failure(&list)["retryable"], json!(true));
    }

    #[test]
    fn parses_exact_defaults_and_correlated_send() {
        let parsed = parse_agent_argv(&args(&["read", "reviewer"])).expect("read");
        assert!(matches!(
            parsed,
            AgentArgv::Command(AgentCommand {
                verb: AgentMethod::Read,
                lines: 80,
                output: AgentOutput::Text,
                ..
            })
        ));
        let parsed = parse_agent_argv(&args(&[
            "send",
            "reviewer",
            "hello",
            "there",
            "--sender",
            "primary",
            "--message-id",
            "request-1",
            "--expect-reply",
        ]))
        .expect("send");
        let request = request_for(&parsed, "").expect("request");
        assert_eq!(request.method, AgentMethod::Send);
        assert_eq!(request.params["text"], "hello there");
        assert_eq!(request.params["expectReply"], true);
    }

    #[test]
    fn rejects_cross_verb_options_and_invalid_names() {
        for values in [
            vec!["list", "--lines", "2"],
            vec!["wait", "reviewer"],
            vec!["start", "Reviewer"],
            vec!["send", "reviewer", "text", "--expect-reply"],
            vec!["rename", "reviewer"],
        ] {
            assert!(parse_agent_argv(&args(&values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn list_sends_live_only_when_asked() {
        let request = |values: &[&str]| {
            request_for(&parse_agent_argv(&args(values)).expect("list"), "").expect("request")
        };
        assert!(request(&["list"]).params.is_empty());
        assert_eq!(request(&["list", "--live"]).params["live"], true);
        assert_eq!(
            parse_agent_argv(&args(&["get", "reviewer", "--live"])),
            Err("--live is only valid with list".into())
        );
    }

    #[test]
    fn stdin_detection_is_parse_aware() {
        assert!(agent_command_uses_stdin(&args(&["send", "reviewer", "-"])));
        assert!(agent_command_uses_stdin(&args(&[
            "start", "reviewer", "--brief", "-"
        ])));
        assert!(!agent_command_uses_stdin(&args(&[
            "send", "reviewer", "text"
        ])));
    }

    #[test]
    fn stdin_validation_uses_legacy_text_and_json_usage_envelopes() {
        let text = render_stdin_read_error(
            &args(&["send", "reviewer", "-", "--output", "text"]),
            AgentStdinReadError::TooLarge,
        );
        assert_eq!(text.exit_code, CLI_EXIT.validation);
        assert_eq!(
            text.stderr,
            "iface: agent send stdin is limited to 64 KiB of UTF-8\n"
        );

        let json = render_stdin_read_error(
            &args(&["start", "reviewer", "--brief", "-", "--output", "json"]),
            AgentStdinReadError::InvalidUtf8,
        );
        assert_eq!(json.exit_code, CLI_EXIT.validation);
        let error: Value = serde_json::from_str(json.stderr.trim()).expect("json error");
        assert_eq!(error["kind"], "usage");
        assert_eq!(error["operation"], "agent.start");
        assert_eq!(error["message"], "agent start stdin must be valid UTF-8");
    }

    #[test]
    fn prompt_limit_applies_only_to_stdin_not_literal_argv_text() {
        let literal = "x".repeat(AGENT_MAX_PROMPT_BYTES + 1);
        let parsed = AgentArgv::Command(AgentCommand {
            verb: AgentMethod::Send,
            output: AgentOutput::Json,
            positionals: vec!["reviewer".into(), literal.clone()],
            lines: 80,
            status: None,
            timeout_ms: None,
            after: 0,
            limit: 100,
            engine: None,
            cwd: None,
            model: None,
            effort: None,
            scope: None,
            prompt: None,
            brief: None,
            task: None,
            sender: None,
            message_id: None,
            reply_to: None,
            expect_reply: false,
            clear: false,
            live: false,
        });
        let request = request_for(&parsed, "").expect("literal request");
        assert_eq!(request.params["text"], literal);
    }

    #[test]
    fn text_render_is_bounded_and_terminal_safe() {
        let snapshot = json!({
            "name": "reviewer",
            "key": "claude#abcdefghi",
            "engine": "claude",
            "sessionId": "abcdefghi",
            "status": "idle",
            "cwd": "/repo\u{001b}[31m",
            "title": "Review",
            "capabilities": {"reference": true, "transcript": true, "message": true,
                "steer": false, "stop": true, "inbox": true, "monitor": true},
            "live": true
        });
        assert_eq!(
            render_text(AgentMethod::Get, &snapshot).expect("render"),
            "reviewer\tidle\tclaude\tabcdefgh\t/repo\tReview\n"
        );
    }

    #[test]
    fn snapshot_parser_matches_legacy_optional_capabilities_and_shape_errors() {
        let value = json!({
            "name": null,
            "key": "claude#one",
            "engine": "claude",
            "sessionId": "one",
            "status": "idle",
            "cwd": "/repo",
            "title": "One",
            "live": true,
            "futureField": "ignored"
        });

        let snapshot = snapshot_from(&value).expect("legacy snapshot");

        assert!(snapshot.capabilities.reference);
        assert!(snapshot.capabilities.transcript);
        assert!(snapshot.capabilities.message);
        assert!(!snapshot.capabilities.steer);
        assert!(snapshot.capabilities.stop);
        assert_eq!(
            snapshot_from(&json!({"key": "claude#one"}))
                .expect_err("missing fields")
                .message,
            "Agent response did not contain a snapshot."
        );
        let mut invalid_capabilities = value.clone();
        invalid_capabilities["capabilities"] = json!({"reference": false});
        assert_eq!(
            snapshot_from(&invalid_capabilities)
                .expect_err("invalid capabilities")
                .message,
            "Agent response capabilities have an invalid shape."
        );
    }
}
