use std::collections::HashMap;

use interface_contracts::engine::{
    ContextCategoryInput, ContextCompactTrigger, ContextUsage, ContextUsageInput, EngineErrorCause,
    EngineEvent, EngineEventKind, FastModeState, NoticeTone, ProviderCommand, RateLimitNotice,
    SessionStatus, normalize_context_usage,
};
use interface_contracts::provider_commands::{
    ProviderCommandCandidate, normalize_provider_commands,
};
use serde_json::{Map, Value};

const MAX_SUMMARY_CHARS: usize = 120;

/// Per-session mapper state. The subagent ledger must never be shared across runs.
#[derive(Debug, Default)]
pub struct ClaudeEventMapper {
    subagents: HashMap<String, String>,
    /// This run's reply was Claude Code's own "not logged in" message
    /// (`"error": "authentication_failed"` on the assistant message), so the
    /// run's failed result is a sign-in problem.
    signed_out: bool,
}

impl ClaudeEventMapper {
    #[must_use]
    pub fn map(&mut self, message: &Value) -> Vec<EngineEvent> {
        let Some(message) = message.as_object() else {
            return Vec::new();
        };
        self.map_object(message)
    }

    #[must_use]
    pub(crate) fn map_object(&mut self, message: &Map<String, Value>) -> Vec<EngineEvent> {
        let agent_key = nonempty_text(message.get("parent_tool_use_id"));
        match nonempty_text(message.get("type")) {
            Some("system") => map_system(message),
            Some("stream_event") => map_stream_event(message.get("event"), agent_key),
            Some("assistant") => self.map_assistant(message, agent_key),
            Some("user") => self.map_user(message, agent_key),
            Some("result") => map_result(message, std::mem::take(&mut self.signed_out)),
            Some("rate_limit_event") => map_rate_limit(message),
            _ => Vec::new(),
        }
    }

    fn map_assistant(
        &mut self,
        message: &Map<String, Value>,
        agent_key: Option<&str>,
    ) -> Vec<EngineEvent> {
        if nonempty_text(message.get("error")) == Some("authentication_failed") {
            self.signed_out = true;
        }
        let mut output = Vec::new();
        for block in content_blocks(message) {
            if nonempty_text(block.get("type")) != Some("tool_use") {
                continue;
            }
            let tool = nonempty_text(block.get("name")).unwrap_or_default();
            let call_id = nonempty_text(block.get("id")).unwrap_or_default();
            let input = block
                .get("input")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let summary = tool_summary(tool, &input);
            let spawned = spawn_details(tool, &input);
            let diff = diff_event(call_id, tool, &input, agent_key);
            output.push(event(EngineEventKind::ToolCall {
                call_id: call_id.to_owned(),
                tool: tool.to_owned(),
                summary,
                input: Value::Object(input),
                agent_key: agent_key.map(str::to_owned),
            }));

            if let Some((subagent_type, description, model)) = spawned {
                self.subagents
                    .insert(call_id.to_owned(), subagent_type.clone());
                output.push(event(EngineEventKind::SubagentStarted {
                    agent_key: call_id.to_owned(),
                    subagent_type,
                    description,
                    model,
                }));
            }

            if let Some(diff) = diff {
                output.push(event(diff));
            }
        }
        output
    }

    fn map_user(
        &mut self,
        message: &Map<String, Value>,
        agent_key: Option<&str>,
    ) -> Vec<EngineEvent> {
        let mut output = Vec::new();
        for block in content_blocks(message) {
            if nonempty_text(block.get("type")) != Some("tool_result") {
                continue;
            }
            let call_id = nonempty_text(block.get("tool_use_id")).unwrap_or_default();
            let ok = block.get("is_error") != Some(&Value::Bool(true));
            let summary = clip_tail(&result_text(block.get("content")), MAX_SUMMARY_CHARS);
            // The summary is copied only for a paired subagent; a plain tool
            // result moves it straight into the event.
            let finished = self.subagents.remove(call_id).map(|subagent_type| {
                event(EngineEventKind::SubagentFinished {
                    agent_key: call_id.to_owned(),
                    ok,
                    summary: if summary.is_empty() {
                        subagent_type
                    } else {
                        summary.clone()
                    },
                })
            });
            output.push(event(EngineEventKind::ToolResult {
                call_id: call_id.to_owned(),
                ok,
                summary,
                agent_key: agent_key.map(str::to_owned),
            }));
            output.extend(finished);
        }
        output
    }
}

#[must_use]
pub fn tool_summary(tool: &str, input: &Map<String, Value>) -> String {
    let summary = ["description", "command", "file_path"]
        .into_iter()
        .find_map(|field| nonempty_text(input.get(field)))
        .unwrap_or(tool);
    clip_head(summary, MAX_SUMMARY_CHARS)
}

#[must_use]
pub fn context_usage_from_sdk(raw: &Value) -> Option<ContextUsage> {
    let snapshot = raw.as_object()?;
    let categories = snapshot
        .get("categories")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_object)
                .map(|row| ContextCategoryInput {
                    name: nonempty_text(row.get("name")).map(str::to_owned),
                    tokens: finite_number(row.get("tokens")),
                })
                .collect()
        });
    normalize_context_usage(ContextUsageInput {
        used: finite_number(snapshot.get("totalTokens")),
        size: finite_number(snapshot.get("maxTokens")),
        percent: finite_number(snapshot.get("percentage")),
        categories,
        auto_compact: snapshot
            .get("isAutoCompactEnabled")
            .and_then(Value::as_bool),
        auto_compact_threshold: finite_number(snapshot.get("autoCompactThreshold")),
        compacting: None,
    })
}

#[allow(clippy::too_many_lines)]
fn map_system(message: &Map<String, Value>) -> Vec<EngineEvent> {
    match nonempty_text(message.get("subtype")) {
        Some("init") => {
            let mut output = vec![event(EngineEventKind::SessionMeta {
                session_id: nonempty_text(message.get("session_id"))
                    .unwrap_or_default()
                    .to_owned(),
                model: nonempty_text(message.get("model"))
                    .unwrap_or_default()
                    .to_owned(),
                cwd: nonempty_text(message.get("cwd"))
                    .unwrap_or_default()
                    .to_owned(),
                title: None,
            })];
            output.extend(fast_mode_events(message));
            output
        }
        Some("api_retry") => {
            let attempt = rounded_nonnegative(message.get("attempt")).max(1);
            let max_attempts = rounded_nonnegative(message.get("max_retries")).max(attempt);
            let delay_ms = rounded_nonnegative(message.get("retry_delay_ms"));
            let wait = (delay_ms > 0).then(|| ((delay_ms + 500) / 1_000).max(1));
            vec![event(EngineEventKind::TurnRetry {
                attempt,
                max_attempts,
                delay_ms: (delay_ms > 0).then_some(delay_ms),
                message: wait.map_or_else(
                    || format!("Retrying ({attempt}/{max_attempts})"),
                    |seconds| format!("Retrying ({attempt}/{max_attempts}) in {seconds}s"),
                ),
            })]
        }
        Some("compact_boundary") => {
            let metadata = message.get("compact_metadata").and_then(Value::as_object);
            let trigger = match metadata.and_then(|value| nonempty_text(value.get("trigger"))) {
                Some("manual") => ContextCompactTrigger::Manual,
                Some("auto") => ContextCompactTrigger::Auto,
                _ => ContextCompactTrigger::Unknown,
            };
            vec![event(EngineEventKind::ContextCompact {
                trigger,
                pre_tokens: metadata.and_then(|value| optional_tokens(value.get("pre_tokens"))),
                post_tokens: metadata.and_then(|value| optional_tokens(value.get("post_tokens"))),
                ok: Some(true),
            })]
        }
        Some("status") => {
            let mut output = Vec::new();
            if nonempty_text(message.get("status")) == Some("compacting") {
                if let Some(usage) = normalize_context_usage(ContextUsageInput {
                    compacting: Some(true),
                    ..ContextUsageInput::default()
                }) {
                    output.push(event(EngineEventKind::ContextUsage { usage }));
                }
            }
            if nonempty_text(message.get("compact_result")) == Some("failed") {
                output.push(event(EngineEventKind::ContextCompact {
                    trigger: ContextCompactTrigger::Unknown,
                    pre_tokens: None,
                    post_tokens: None,
                    ok: Some(false),
                }));
            }
            output
        }
        // The same shared normalizer as initialize and refresh: a missing or
        // non-array list emits nothing, an empty array still emits.
        Some("commands_changed") => message
            .get("commands")
            .and_then(claude_command_list)
            .map(|commands| vec![event(EngineEventKind::CommandsAvailable { commands })])
            .unwrap_or_default(),
        Some("local_command_output") => nonempty_text(message.get("content"))
            .map(|content| {
                vec![
                    event(EngineEventKind::MessageDelta {
                        text: content.to_owned(),
                        agent_key: None,
                    }),
                    event(EngineEventKind::StatusChanged {
                        status: SessionStatus::Idle,
                    }),
                ]
            })
            .unwrap_or_default(),
        Some("informational") => nonempty_text(message.get("content"))
            .map(|content| {
                vec![event(EngineEventKind::Notice {
                    message: content.to_owned(),
                    tone: if nonempty_text(message.get("level")) == Some("warning") {
                        NoticeTone::Warning
                    } else {
                        NoticeTone::Info
                    },
                })]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn map_stream_event(event_value: Option<&Value>, agent_key: Option<&str>) -> Vec<EngineEvent> {
    let Some(stream) = event_value.and_then(Value::as_object) else {
        return Vec::new();
    };
    match nonempty_text(stream.get("type")) {
        Some("content_block_delta") => {
            let delta = stream.get("delta").and_then(Value::as_object);
            if delta.and_then(|value| nonempty_text(value.get("type"))) != Some("text_delta") {
                return Vec::new();
            }
            delta
                .and_then(|value| nonempty_text(value.get("text")))
                .map(|text| {
                    vec![event(EngineEventKind::MessageDelta {
                        text: text.to_owned(),
                        agent_key: agent_key.map(str::to_owned),
                    })]
                })
                .unwrap_or_default()
        }
        Some("content_block_start") => {
            let block_type = stream
                .get("content_block")
                .and_then(Value::as_object)
                .and_then(|value| nonempty_text(value.get("type")));
            block_type
                .map(|block_type| {
                    vec![event(EngineEventKind::ThinkingSignal {
                        active: matches!(block_type, "thinking" | "redacted_thinking"),
                        agent_key: agent_key.map(str::to_owned),
                    })]
                })
                .unwrap_or_default()
        }
        Some("message_stop") => vec![event(EngineEventKind::ThinkingSignal {
            active: false,
            agent_key: agent_key.map(str::to_owned),
        })],
        _ => Vec::new(),
    }
}

fn map_result(message: &Map<String, Value>, signed_out: bool) -> Vec<EngineEvent> {
    let mut output = Vec::new();
    if let Some(usage) = message.get("usage").and_then(Value::as_object) {
        output.push(event(EngineEventKind::UsageUpdated {
            input_tokens: rounded_nonnegative(usage.get("input_tokens")),
            output_tokens: rounded_nonnegative(usage.get("output_tokens")),
            rate_limited: None,
        }));
    }
    output.extend(fast_mode_events(message));
    let subtype = nonempty_text(message.get("subtype"));
    let failed = message.get("is_error") == Some(&Value::Bool(true)) || subtype != Some("success");
    if failed {
        output.push(event(EngineEventKind::Error {
            cause: signed_out.then_some(EngineErrorCause::SignedOut),
            message: result_error_message(message),
            retryable: false,
        }));
    } else {
        output.push(event(EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        }));
    }
    output
}

fn map_rate_limit(message: &Map<String, Value>) -> Vec<EngineEvent> {
    let information = message.get("rate_limit_info").and_then(Value::as_object);
    if information.and_then(|value| nonempty_text(value.get("status"))) != Some("rejected") {
        return Vec::new();
    }
    vec![event(EngineEventKind::UsageUpdated {
        input_tokens: 0,
        output_tokens: 0,
        rate_limited: Some(RateLimitNotice {
            message: "Rate limit reached".into(),
            resets_at: information.and_then(rate_limit_reset),
        }),
    })]
}

/// When a rejected rate limit resets (`rate_limit_info.resetsAt`), in the
/// ISO form usage windows use. The CLI sends Unix seconds; a value large
/// enough to be milliseconds is read as such (the oracle's rule). Absent,
/// zero, negative or not a number: unknown.
fn rate_limit_reset(information: &Map<String, Value>) -> Option<String> {
    let value = information.get("resetsAt")?.as_f64()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let millis = if value < 1e12 { value * 1000.0 } else { value };
    #[allow(clippy::cast_possible_truncation)]
    let millis = millis.round().min(253_402_300_799_000.0) as i64;
    Some(interface_common::time::unix_millis_iso(
        millis,
        interface_common::time::IsoFraction::Millis,
    ))
}

fn fast_mode_events(message: &Map<String, Value>) -> Vec<EngineEvent> {
    let state = match nonempty_text(message.get("fast_mode_state")) {
        Some("off") => FastModeState::Off,
        Some("cooldown") => FastModeState::Cooldown,
        Some("on") => FastModeState::On,
        _ => return Vec::new(),
    };
    vec![event(EngineEventKind::FastmodeChanged {
        state,
        reason: nonempty_text(message.get("fast_mode_disabled_reason")).map(str::to_owned),
    })]
}

fn spawn_details(
    tool: &str,
    input: &Map<String, Value>,
) -> Option<(String, String, Option<String>)> {
    let declared = nonempty_text(input.get("subagent_type"));
    if declared.is_none() && !matches!(tool, "Agent" | "Task") {
        return None;
    }
    let subagent_type = declared.unwrap_or(tool).to_owned();
    let description = ["description", "prompt"]
        .into_iter()
        .find_map(|field| nonempty_text(input.get(field)))
        .map_or_else(|| subagent_type.clone(), str::to_owned);
    Some((
        subagent_type,
        clip_head(&description, MAX_SUMMARY_CHARS),
        nonempty_text(input.get("model")).map(str::to_owned),
    ))
}

fn diff_event(
    call_id: &str,
    tool: &str,
    input: &Map<String, Value>,
    agent_key: Option<&str>,
) -> Option<EngineEventKind> {
    let path = nonempty_text(input.get("file_path"))?.to_owned();
    match tool {
        "Edit" => Some(EngineEventKind::DiffProduced {
            call_id: call_id.to_owned(),
            path,
            before: nonempty_text(input.get("old_string"))
                .unwrap_or_default()
                .to_owned(),
            after: nonempty_text(input.get("new_string"))
                .unwrap_or_default()
                .to_owned(),
            agent_key: agent_key.map(str::to_owned),
        }),
        "Write" => Some(EngineEventKind::DiffProduced {
            call_id: call_id.to_owned(),
            path,
            before: String::new(),
            after: nonempty_text(input.get("content"))
                .unwrap_or_default()
                .to_owned(),
            agent_key: agent_key.map(str::to_owned),
        }),
        _ => None,
    }
}

fn content_blocks(message: &Map<String, Value>) -> impl Iterator<Item = &Map<String, Value>> {
    message
        .get("message")
        .and_then(Value::as_object)
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

/// A tool result's text, borrowed when it is one string: a 100 KB Read or
/// Bash result is only clipped to its last 120 characters.
fn result_text(content: Option<&Value>) -> std::borrow::Cow<'_, str> {
    match content {
        Some(Value::String(text)) => std::borrow::Cow::Borrowed(text.trim()),
        Some(Value::Array(blocks)) => {
            let joined = blocks
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|block| nonempty_text(block.get("text")))
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = joined.trim();
            std::borrow::Cow::Owned(if trimmed.len() == joined.len() {
                joined
            } else {
                trimmed.to_owned()
            })
        }
        _ => std::borrow::Cow::Borrowed(""),
    }
}

fn result_error_message(message: &Map<String, Value>) -> String {
    let errors = message
        .get("errors")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| nonempty_text(Some(value)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !errors.is_empty() {
        return errors.join("; ");
    }
    let subtype = nonempty_text(message.get("subtype"));
    if subtype == Some("success") {
        return nonempty_text(message.get("result"))
            .unwrap_or("turn failed (is_error on success subtype)")
            .to_owned();
    }
    subtype.unwrap_or("error").to_owned()
}

/// Every Claude command list (initialize, refresh, `commands_changed`) goes
/// through the shared provider-command normalizer: caps, the name pattern and
/// the app-owned filter. `None` when `commands` is not an array.
pub(crate) fn claude_command_list(commands: &Value) -> Option<Vec<ProviderCommand>> {
    let candidates = commands
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .map(|command| ProviderCommandCandidate {
            name: command.get("name").cloned().unwrap_or(Value::Null),
            description: command.get("description").cloned().unwrap_or(Value::Null),
            argument_hint: command
                .get("argumentHint")
                .or_else(|| command.get("argument_hint"))
                .cloned()
                .unwrap_or(Value::Null),
            aliases: command.get("aliases").cloned().unwrap_or(Value::Null),
        })
        .collect::<Vec<_>>();
    Some(normalize_provider_commands(&candidates))
}

fn event(event: EngineEventKind) -> EngineEvent {
    EngineEvent { at: None, event }
}

fn nonempty_text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn rounded_nonnegative(value: Option<&Value>) -> u64 {
    optional_tokens(value).unwrap_or(0)
}

fn optional_tokens(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).or_else(|| {
        value
            .and_then(Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
    })
}

fn clip_head(value: &str, limit: usize) -> String {
    match value.char_indices().nth(limit) {
        None => value.to_owned(),
        Some(_) => value
            .chars()
            .take(limit.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect(),
    }
}

/// The last `limit - 1` characters after an ellipsis, or the whole text when
/// it fits. Walks at most `limit` characters from the end, not the whole text.
fn clip_tail(value: &str, limit: usize) -> String {
    if value.char_indices().rev().nth(limit).is_none() {
        return value.to_owned();
    }
    let start = match limit {
        0 | 1 => value.len(),
        _ => value
            .char_indices()
            .rev()
            .nth(limit - 2)
            .map_or(0, |(index, _)| index),
    };
    format!("…{}", &value[start..])
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::{ContextCategory, EngineEventKind, FastModeState};
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_init_text_and_failed_result_without_a_trailing_idle() {
        let mut mapper = ClaudeEventMapper::default();
        assert_eq!(
            mapper.map(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": "s-1",
                "model": "sonnet",
                "cwd": "/workspace",
                "fast_mode_state": "on"
            })),
            vec![
                event(EngineEventKind::SessionMeta {
                    session_id: "s-1".into(),
                    model: "sonnet".into(),
                    cwd: "/workspace".into(),
                    title: None,
                }),
                event(EngineEventKind::FastmodeChanged {
                    state: FastModeState::On,
                    reason: None,
                }),
            ]
        );
        assert_eq!(
            mapper.map(&json!({
                "type": "stream_event",
                "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "hi"}}
            })),
            vec![event(EngineEventKind::MessageDelta {
                text: "hi".into(),
                agent_key: None,
            })]
        );
        let failed = mapper.map(&json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "result": "Authentication failed",
            "usage": {"input_tokens": 3, "output_tokens": 4}
        }));
        assert!(
            matches!(failed.last().map(|event| &event.event), Some(EngineEventKind::Error { message, retryable: false, .. }) if message == "Authentication failed")
        );
        assert!(!failed.iter().any(|event| matches!(
            event.event,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            }
        )));
    }

    #[test]
    fn a_signed_out_claude_turn_fails_with_the_sign_in_cause() {
        // What Claude Code 2.1 streams when it is not logged in.
        let mut mapper = ClaudeEventMapper::default();
        let reply = mapper.map(&json!({
            "type": "assistant",
            "error": "authentication_failed",
            "message": {"model": "<synthetic>", "content": [
                {"type": "text", "text": "Not logged in · Please run /login"}
            ]}
        }));
        assert!(reply.is_empty());
        let failed = mapper.map(&json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "terminal_reason": "api_error",
            "result": "Not logged in · Please run /login"
        }));
        assert!(
            matches!(
                failed.last().map(|event| &event.event),
                Some(EngineEventKind::Error {
                    cause: Some(EngineErrorCause::SignedOut),
                    retryable: false,
                    message,
                }) if message == "Not logged in · Please run /login"
            ),
            "{failed:?}"
        );
        // Only that run: the next failure is an ordinary one.
        let later = mapper.map(&json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "result": "Overloaded"
        }));
        assert!(matches!(
            later.last().map(|event| &event.event),
            Some(EngineEventKind::Error { cause: None, .. })
        ));
    }

    #[test]
    fn maps_tools_diffs_and_subagents_with_run_local_pairing() {
        let mut mapper = ClaudeEventMapper::default();
        let started = mapper.map(&json!({
            "type": "assistant",
            "parent_tool_use_id": "outer",
            "message": {"content": [{
                "type": "tool_use",
                "id": "call-1",
                "name": "Task",
                "input": {"subagent_type": "Explore", "description": "Find it", "model": "opus"}
            }]}
        }));
        assert!(
            matches!(&started[0].event, EngineEventKind::ToolCall { agent_key: Some(key), .. } if key == "outer")
        );
        assert!(
            matches!(&started[1].event, EngineEventKind::SubagentStarted { agent_key, model: Some(model), .. } if agent_key == "call-1" && model == "opus")
        );

        let finished = mapper.map(&json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "call-1",
                "is_error": false,
                "content": "found it"
            }]}
        }));
        assert!(
            matches!(&finished[1].event, EngineEventKind::SubagentFinished { agent_key, ok: true, .. } if agent_key == "call-1")
        );
        let repeated = mapper.map(&json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "call-1", "content": "again"}]}
        }));
        assert_eq!(repeated.len(), 1);

        let edit = mapper.map(&json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "edit-1", "name": "Edit",
                "input": {"file_path": "/tmp/a.rs", "old_string": "a", "new_string": "b"}
            }]}
        }));
        assert!(
            matches!(&edit[1].event, EngineEventKind::DiffProduced { path, before, after, .. } if path == "/tmp/a.rs" && before == "a" && after == "b")
        );
    }

    #[test]
    fn maps_usage_retry_compaction_commands_and_rate_limit() {
        let mut mapper = ClaudeEventMapper::default();
        assert!(matches!(
            &mapper.map(&json!({"type":"system","subtype":"api_retry","attempt":2,"max_retries":4,"retry_delay_ms":1500}))[0].event,
            EngineEventKind::TurnRetry { attempt: 2, max_attempts: 4, delay_ms: Some(1500), .. }
        ));
        assert!(matches!(
            &mapper.map(&json!({"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":180_000,"post_tokens":42_000}}))[0].event,
            EngineEventKind::ContextCompact { trigger: ContextCompactTrigger::Auto, pre_tokens: Some(180_000), post_tokens: Some(42_000), ok: Some(true) }
        ));
        assert!(matches!(
            &mapper.map(&json!({"type":"system","subtype":"commands_changed","commands":[{"name":"/review","description":"Review"}]}))[0].event,
            EngineEventKind::CommandsAvailable { commands } if commands[0].name == "review"
        ));
        assert!(matches!(
            &mapper
                .map(&json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}))
                [0]
            .event,
            EngineEventKind::UsageUpdated {
                rate_limited: Some(_),
                ..
            }
        ));
    }

    /// T3 section 10: a rejected limit keeps the reset the CLI reports
    /// (`rate_limit_info.resetsAt`, Unix seconds, as the recorded replay
    /// fixtures carry it) in the usage windows' ISO form; a limit that
    /// reports none stays without one, and an allowed status is no limit.
    #[test]
    fn a_rejected_rate_limit_keeps_the_reset_the_cli_reports() {
        let mut mapper = ClaudeEventMapper::default();
        let notice = |frame: Value| {
            let mut mapper = ClaudeEventMapper::default();
            match mapper
                .map(&frame)
                .into_iter()
                .next()
                .map(|event| event.event)
            {
                Some(EngineEventKind::UsageUpdated {
                    rate_limited: Some(notice),
                    ..
                }) => notice,
                other => panic!("expected a rate limit, got {other:?}"),
            }
        };
        let kept = notice(json!({"type":"rate_limit_event","rate_limit_info":{
            "status":"rejected","rateLimitType":"five_hour","resetsAt":1_795_273_200}}));
        assert_eq!(kept.message, "Rate limit reached");
        assert_eq!(kept.resets_at.as_deref(), Some("2026-11-21T15:00:00.000Z"));
        let millis = notice(json!({"type":"rate_limit_event","rate_limit_info":{
            "status":"rejected","resetsAt":1_795_273_200_500_u64}}));
        assert_eq!(
            millis.resets_at.as_deref(),
            Some("2026-11-21T15:00:00.500Z")
        );
        for absent in [json!(null), json!(0), json!(-5), json!("soon")] {
            let none = notice(json!({"type":"rate_limit_event","rate_limit_info":{
                "status":"rejected","resetsAt":absent}}));
            assert_eq!(none.resets_at, None, "{absent}");
        }
        let none =
            notice(json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}));
        assert_eq!(none.resets_at, None);
        assert_eq!(
            serde_json::to_value(&none).unwrap(),
            json!({"message":"Rate limit reached"})
        );
        assert!(
            mapper
                .map(&json!({"type":"rate_limit_event","rate_limit_info":{
                    "status":"allowed","resetsAt":1_795_273_200}}))
                .is_empty()
        );
    }

    #[test]
    fn commands_changed_goes_through_the_shared_normalizer() {
        // Ported from the 0.1.169 oracle (claude/mapper.test.ts).
        let mut mapper = ClaudeEventMapper::default();
        let events = mapper.map(
            &json!({"type":"system","subtype":"commands_changed","commands":[
                {"name":"/review","description":"Review","aliases":["/check"]},
                {"name":"/model","description":"app-owned"},
                {"name":"bad name","description":"invalid"},
            ]}),
        );
        assert_eq!(events.len(), 1);
        let EngineEventKind::CommandsAvailable { commands } = &events[0].event else {
            panic!("expected commands, got {:?}", events[0].event);
        };
        assert_eq!(
            commands,
            &vec![ProviderCommand {
                name: "review".into(),
                description: "Review".into(),
                argument_hint: None,
                aliases: Some(vec!["check".into()]),
            }]
        );
        assert!(matches!(
            &mapper.map(&json!({"type":"system","subtype":"commands_changed","commands":[]}))[..],
            [EngineEvent { event: EngineEventKind::CommandsAvailable { commands }, .. }]
                if commands.is_empty()
        ));
        assert!(
            mapper
                .map(&json!({"type":"system","subtype":"commands_changed"}))
                .is_empty()
        );
    }

    #[test]
    fn map_object_matches_map_without_cloning_the_record() {
        let record = json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "write-1",
                "name": "Write",
                "input": {"file_path": "/tmp/a.rs", "content": "fn main() {}"}
            }]}
        });
        let mut via_value = ClaudeEventMapper::default();
        let mut via_object = ClaudeEventMapper::default();
        let mapped = via_value.map(&record);
        let object = record.as_object().expect("object record");
        assert_eq!(via_object.map_object(object), mapped);
        assert!(matches!(
            &mapped[0].event,
            EngineEventKind::ToolCall { input, .. }
                if input == &json!({"file_path": "/tmp/a.rs", "content": "fn main() {}"})
        ));
        assert!(matches!(
            &mapped[1].event,
            EngineEventKind::DiffProduced { path, after, .. }
                if path == "/tmp/a.rs" && after == "fn main() {}"
        ));
    }

    #[test]
    fn clip_head_and_clip_tail_count_unicode_scalars() {
        assert_eq!(clip_head("ééé", 3), "ééé");
        assert_eq!(clip_head("éééé", 3), "éé…");
        assert_eq!(clip_tail("éééé", 3), "…éé");
        assert_eq!(clip_head("hello", 0), "…");
        assert_eq!(clip_tail("hello", 0), "…");
        assert_eq!(clip_head("", 0), "");
        assert_eq!(clip_tail("abc", 3), "abc");
        assert_eq!(clip_head("abc", 3), "abc");
        assert_eq!(
            result_text(Some(&json!([{"text": "  hello"}, {"text": "world  "}]))),
            "hello\nworld"
        );
        let mut mapper = ClaudeEventMapper::default();
        let long = "x".repeat(200);
        let finished = mapper.map(&json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "call-1",
                "content": long
            }]}
        }));
        let EngineEventKind::ToolResult { summary, .. } = &finished[0].event else {
            panic!("expected tool result");
        };
        assert_eq!(summary.chars().count(), 120);
        assert!(summary.starts_with('…'));
        assert!(summary.ends_with('x'));
    }

    #[test]
    fn context_usage_keeps_only_safe_window_categories() {
        assert_eq!(
            context_usage_from_sdk(&json!({
                "totalTokens": 80_000,
                "maxTokens": 200_000,
                "percentage": 41,
                "categories": [
                    {"name": "System prompt", "tokens": 12_000, "path": "/secret"},
                    {"name": "", "tokens": 5}
                ],
                "isAutoCompactEnabled": true,
                "autoCompactThreshold": 180_000,
                "memoryFiles": [{"path": "/secret/MEMORY.md"}]
            })),
            Some(ContextUsage {
                used: Some(80_000),
                size: Some(200_000),
                percent: Some(41.0),
                categories: Some(vec![ContextCategory {
                    name: "System prompt".into(),
                    tokens: 12_000,
                }]),
                auto_compact: Some(true),
                auto_compact_threshold: Some(180_000),
                compacting: None,
            })
        );
    }
}
