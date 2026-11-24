use std::{collections::HashSet, fmt::Write as _};

use interface_contracts::engine::{
    ContextUsageInput, EngineErrorCause, EngineEvent, EngineEventKind, EngineId, EngineUsage,
    RateLimitNotice, SessionStatus, UsageWindow, normalize_context_usage,
};
use interface_contracts::ipc::HistoryPage;
use interface_contracts::prompt_images::{
    PROMPT_IMAGE_LIMITS, PromptImageReference, is_prompt_image_preview_url, prompt_image_basename,
    replayed_turn_images,
};
use interface_contracts::retention::{clip_label, clip_output};
use interface_engine::{ForkBoundary, adapter::FORK_BOUNDARY_NOT_FOUND};
use serde_json::{Map, Value};

use crate::protocol::is_collaboration_host_context;

const MAX_SUMMARY: f64 = 120.0;

#[derive(Default)]
pub struct CodexEventMapper {
    open_tools: HashSet<String>,
    open_subagents: HashSet<String>,
}

impl CodexEventMapper {
    #[cfg(test)]
    #[must_use]
    pub fn map(&mut self, method: &str, params: Option<&Value>) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        self.map_into(&mut events, method, params);
        events
    }

    pub(crate) fn map_into(
        &mut self,
        out: &mut Vec<EngineEvent>,
        method: &str,
        params: Option<&Value>,
    ) {
        let params = params.and_then(Value::as_object);
        match method {
            "thread/started" => out.extend(map_thread_started(params)),
            "thread/status/changed" => out.extend(map_thread_status(params)),
            "turn/started" => out.push(kind(EngineEventKind::StatusChanged {
                status: SessionStatus::Working,
            })),
            "turn/completed" => out.extend(map_turn_completed(params)),
            "item/agentMessage/delta" => {
                if let Some(text) = text(params, "delta") {
                    out.push(kind(EngineEventKind::MessageDelta {
                        text: text.to_owned(),
                        agent_key: None,
                    }));
                }
            }
            "item/reasoning/textDelta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded" => {
                out.push(kind(EngineEventKind::ThinkingSignal {
                    active: true,
                    agent_key: None,
                }));
            }
            "item/started" => {
                out.extend(self.map_item_started(params.and_then(|p| object(p, "item"))));
            }
            "item/completed" => {
                out.extend(self.map_item_completed(params.and_then(|p| object(p, "item"))));
            }
            "item/fileChange/patchUpdated" => out.extend(diff_events(
                text(params, "itemId").unwrap_or_default(),
                params.and_then(|p| p.get("changes")),
            )),
            "error" => out.extend(map_error(params)),
            "account/rateLimits/updated" => {
                out.extend(map_rate_limits(
                    params.and_then(|p| object(p, "rateLimits")),
                ));
            }
            "thread/tokenUsage/updated" => {
                out.extend(map_token_usage(
                    params.and_then(|p| object(p, "tokenUsage")),
                ));
            }
            _ => {}
        }
    }

    fn map_item_started(&mut self, item: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
        let Some(item) = item else {
            return Vec::new();
        };
        let item_type = text(Some(item), "type").unwrap_or_default();
        if item_type == "agentMessage" {
            return vec![kind(EngineEventKind::ThinkingSignal {
                active: false,
                agent_key: None,
            })];
        }
        if !is_tool_item(item_type) {
            return Vec::new();
        }
        let call_id = text(Some(item), "id").unwrap_or_default().to_owned();
        if call_id.is_empty() {
            return Vec::new();
        }
        let tool = tool_name(item_type, item);
        self.open_tools.insert(call_id.clone());
        let mut events = vec![kind(EngineEventKind::ToolCall {
            call_id: call_id.clone(),
            tool,
            summary: tool_summary(item_type, item),
            input: Value::Object(item.clone()),
            agent_key: None,
        })];
        if item_type == "fileChange" {
            events.extend(diff_events(&call_id, item.get("changes")));
        }
        events
    }

    fn map_item_completed(&mut self, item: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
        let Some(item) = item else {
            return Vec::new();
        };
        let item_type = text(Some(item), "type").unwrap_or_default();
        if item_type == "reasoning" {
            return vec![kind(EngineEventKind::ThinkingSignal {
                active: false,
                agent_key: None,
            })];
        }
        if matches!(item_type, "subAgentActivity" | "collabAgentToolCall") {
            return self.map_subagent(item);
        }
        if !is_tool_item(item_type) {
            return Vec::new();
        }
        let call_id = text(Some(item), "id").unwrap_or_default().to_owned();
        if call_id.is_empty() {
            return Vec::new();
        }
        self.open_tools.remove(&call_id);
        vec![kind(EngineEventKind::ToolResult {
            call_id,
            ok: item_ok(item_type, item),
            summary: result_summary(item_type, item),
            agent_key: None,
        })]
    }

    fn map_subagent(&mut self, item: &Map<String, Value>) -> Vec<EngineEvent> {
        let call_id = text(Some(item), "id").unwrap_or_default().to_owned();
        if call_id.is_empty() {
            return Vec::new();
        }
        let activity_kind = text(Some(item), "kind").unwrap_or_default();
        let child = text(Some(item), "agentThreadId")
            .or_else(|| {
                item.get("receiverThreadIds")
                    .and_then(Value::as_array)
                    .and_then(|ids| ids.first())
                    .and_then(Value::as_str)
            })
            .unwrap_or(&call_id)
            .to_owned();
        if activity_kind == "started" || text(Some(item), "tool") == Some("spawnAgent") {
            if !self.open_subagents.insert(child.clone()) {
                return Vec::new();
            }
            let description = text(Some(item), "prompt")
                .or_else(|| text(Some(item), "agentPath"))
                .unwrap_or("Codex subagent")
                .to_owned();
            return vec![
                kind(EngineEventKind::ToolCall {
                    call_id: child.clone(),
                    tool: "spawnAgent".into(),
                    summary: one_line(&description),
                    input: Value::Object(item.clone()),
                    agent_key: None,
                }),
                kind(EngineEventKind::SubagentStarted {
                    agent_key: child,
                    subagent_type: "Agent".into(),
                    description,
                    model: text(Some(item), "model").map(ToOwned::to_owned),
                }),
            ];
        }
        if matches!(activity_kind, "interrupted" | "completed")
            && self.open_subagents.remove(&child)
        {
            return vec![
                kind(EngineEventKind::ToolResult {
                    call_id: child.clone(),
                    ok: activity_kind == "completed",
                    summary: activity_kind.to_owned(),
                    agent_key: None,
                }),
                kind(EngineEventKind::SubagentFinished {
                    agent_key: child,
                    ok: activity_kind == "completed",
                    summary: activity_kind.to_owned(),
                }),
            ];
        }
        Vec::new()
    }

    #[must_use]
    pub fn settle_open(&mut self, summary: &str) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        for call_id in self.open_tools.drain() {
            events.push(kind(EngineEventKind::ToolResult {
                call_id,
                ok: false,
                summary: summary.to_owned(),
                agent_key: None,
            }));
        }
        for agent_key in self.open_subagents.drain() {
            events.push(kind(EngineEventKind::ToolResult {
                call_id: agent_key.clone(),
                ok: false,
                summary: summary.to_owned(),
                agent_key: None,
            }));
            events.push(kind(EngineEventKind::SubagentFinished {
                agent_key,
                ok: false,
                summary: summary.to_owned(),
            }));
        }
        events
    }
}

fn kind(event: EngineEventKind) -> EngineEvent {
    EngineEvent { at: None, event }
}

fn object<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key)?.as_object()
}

fn text<'a>(value: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    value?.get(key)?.as_str().filter(|value| !value.is_empty())
}

fn number(value: Option<&Map<String, Value>>, key: &str) -> u64 {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn map_thread_started(params: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let Some(thread) = params.and_then(|params| object(params, "thread")) else {
        return Vec::new();
    };
    vec![kind(EngineEventKind::SessionMeta {
        session_id: text(Some(thread), "id").unwrap_or_default().to_owned(),
        model: String::new(),
        cwd: text(Some(thread), "cwd").unwrap_or_default().to_owned(),
        title: text(Some(thread), "name")
            .or_else(|| text(Some(thread), "preview"))
            .map(one_line),
    })]
}

fn map_thread_status(params: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let status = params
        .and_then(|params| object(params, "status"))
        .and_then(|status| text(Some(status), "type"));
    let status = match status {
        Some("active") => SessionStatus::Working,
        Some("idle") => SessionStatus::Idle,
        Some("systemError") => SessionStatus::Failed,
        _ => return Vec::new(),
    };
    vec![kind(EngineEventKind::StatusChanged { status })]
}

fn map_turn_completed(params: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let Some(turn) = params.and_then(|params| object(params, "turn")) else {
        return Vec::new();
    };
    if text(Some(turn), "status") == Some("inProgress") {
        return Vec::new();
    }
    let mut events = vec![kind(EngineEventKind::ThinkingSignal {
        active: false,
        agent_key: None,
    })];
    if text(Some(turn), "status") == Some("failed") {
        let error = object(turn, "error");
        let message = turn_error_message(error);
        events.push(kind(EngineEventKind::Error {
            cause: turn_error_cause(error),
            message,
            retryable: false,
        }));
    }
    events
}

fn map_error(params: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let error = params.and_then(|params| object(params, "error"));
    vec![kind(EngineEventKind::Error {
        cause: turn_error_cause(error),
        message: turn_error_message(error),
        retryable: params
            .and_then(|params| params.get("willRetry"))
            .and_then(Value::as_bool)
            == Some(true),
    })]
}

/// Codex says `codexErrorInfo: "unauthorized"` when its CLI is not signed in
/// (or its sign-in expired): the app offers Sign in for it.
fn turn_error_cause(error: Option<&Map<String, Value>>) -> Option<EngineErrorCause> {
    (text(error, "codexErrorInfo") == Some("unauthorized")).then_some(EngineErrorCause::SignedOut)
}

fn turn_error_message(error: Option<&Map<String, Value>>) -> String {
    let message = text(error, "message").unwrap_or("Codex turn failed");
    text(error, "additionalDetails").map_or_else(
        || message.to_owned(),
        |detail| format!("{message}: {detail}"),
    )
}

fn limit_reached_notice(snapshot: &Map<String, Value>) -> Option<String> {
    if let Some(reached) = text(Some(snapshot), "rateLimitReachedType") {
        return Some(format!("Rate limit reached ({reached})"));
    }
    (snapshot.get("spendControlReached").and_then(Value::as_bool) == Some(true))
        .then(|| "Spend limit reached".to_owned())
}

fn map_rate_limits(rate_limits: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let Some(rate_limits) = rate_limits else {
        return Vec::new();
    };
    let Some(mut message) = limit_reached_notice(rate_limits) else {
        return Vec::new();
    };
    let reset = object(rate_limits, "primary")
        .or_else(|| object(rate_limits, "secondary"))
        .map_or(0, |window| number(Some(window), "resetsAt"));
    // T3 section 10: the reset is carried as its own field too, so the host
    // can offer to resume the thread then.
    let resets_at = (reset > 0).then(|| {
        let seconds = i64::try_from(reset).unwrap_or(i64::MAX);
        super::adapter::unix_timestamp(seconds)
    });
    if let Some(iso) = &resets_at {
        let _ = write!(message, "; resets {iso}");
    }
    vec![kind(EngineEventKind::UsageUpdated {
        input_tokens: 0,
        output_tokens: 0,
        rate_limited: Some(RateLimitNotice { message, resets_at }),
    })]
}

fn map_token_usage(usage: Option<&Map<String, Value>>) -> Vec<EngineEvent> {
    let Some(usage) = usage else {
        return Vec::new();
    };
    let Some(total) = object(usage, "total") else {
        return Vec::new();
    };
    let mut events = vec![kind(EngineEventKind::UsageUpdated {
        input_tokens: number(Some(total), "inputTokens"),
        output_tokens: number(Some(total), "outputTokens"),
        rate_limited: None,
    })];
    let last = object(usage, "last");
    let context = normalize_context_usage(ContextUsageInput {
        used: last
            .and_then(|last| last.get("totalTokens"))
            .and_then(Value::as_f64),
        size: usage.get("modelContextWindow").and_then(Value::as_f64),
        ..ContextUsageInput::default()
    });
    if let Some(usage) = context {
        events.push(kind(EngineEventKind::ContextUsage { usage }));
    }
    events
}

fn is_tool_item(item_type: &str) -> bool {
    matches!(
        item_type,
        "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
    )
}

fn tool_name(item_type: &str, item: &Map<String, Value>) -> String {
    match item_type {
        "mcpToolCall" => {
            let tool = text(Some(item), "tool").unwrap_or("tool");
            text(Some(item), "server")
                .map_or_else(|| tool.to_owned(), |server| format!("{server}/{tool}"))
        }
        "dynamicToolCall" => {
            let tool = text(Some(item), "tool").unwrap_or("tool");
            text(Some(item), "namespace").map_or_else(
                || tool.to_owned(),
                |namespace| format!("{namespace}/{tool}"),
            )
        }
        _ => item_type.to_owned(),
    }
}

fn tool_summary(item_type: &str, item: &Map<String, Value>) -> String {
    match item_type {
        "commandExecution" => one_line(text(Some(item), "command").unwrap_or("command")),
        "fileChange" => one_line(&change_list(item.get("changes"))),
        _ => tool_name(item_type, item),
    }
}

fn item_ok(item_type: &str, item: &Map<String, Value>) -> bool {
    if text(Some(item), "status") != Some("completed") {
        return false;
    }
    if item_type == "mcpToolCall" && item.get("error").and_then(Value::as_object).is_some() {
        return false;
    }
    item_type != "dynamicToolCall" || item.get("success").and_then(Value::as_bool) != Some(false)
}

fn result_summary(item_type: &str, item: &Map<String, Value>) -> String {
    match item_type {
        "commandExecution" => {
            let exit = item
                .get("exitCode")
                .filter(|value| !value.is_null())
                .map(|value| format!("exit {}", json_number(value)));
            let output = text(Some(item), "aggregatedOutput").map(collapse_whitespace);
            let parts = exit.into_iter().chain(output).collect::<Vec<_>>();
            if parts.is_empty() {
                text(Some(item), "status").unwrap_or_default().to_owned()
            } else {
                clip_output(&parts.join(" — "), MAX_SUMMARY)
            }
        }
        "fileChange" => {
            let status = text(Some(item), "status");
            if status.is_some_and(|status| status != "completed") {
                status.unwrap_or_default().to_owned()
            } else {
                one_line(&change_list(item.get("changes")))
            }
        }
        "mcpToolCall" => {
            if let Some(error) = item.get("error").and_then(Value::as_object) {
                let message = collapse_whitespace(
                    text(Some(error), "message").unwrap_or("MCP tool call failed"),
                );
                clip_output(&message, MAX_SUMMARY)
            } else {
                format!(
                    "{} {}",
                    tool_name(item_type, item),
                    text(Some(item), "status").unwrap_or_default()
                )
                .trim()
                .to_owned()
            }
        }
        _ => text(Some(item), "status").unwrap_or_default().to_owned(),
    }
}

fn change_list(changes: Option<&Value>) -> String {
    let entries = changes
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|change| {
            let kind = change
                .get("kind")
                .and_then(Value::as_object)
                .and_then(|kind| text(Some(kind), "type"));
            let path = text(Some(change), "path").unwrap_or_default();
            let entry = kind.map_or_else(|| path.to_owned(), |kind| format!("{kind} {path}"));
            (!entry.is_empty()).then_some(entry)
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        "file change".into()
    } else {
        entries.join(", ")
    }
}

fn json_number(value: &Value) -> String {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or_else(
            || "0".into(),
            |value| {
                if value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    value.to_string()
                }
            },
        )
}

fn diff_events(call_id: &str, changes: Option<&Value>) -> Vec<EngineEvent> {
    changes
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let change = change.as_object()?;
            let path = text(Some(change), "path")?.to_owned();
            let (before, after) =
                unified_diff_to_before_after(text(Some(change), "diff").unwrap_or_default());
            Some(kind(EngineEventKind::DiffProduced {
                call_id: call_id.to_owned(),
                path,
                before,
                after,
                agent_key: None,
            }))
        })
        .collect()
}

#[must_use]
pub(crate) fn unified_diff_to_before_after(diff: &str) -> (String, String) {
    if diff.is_empty() {
        return (String::new(), String::new());
    }
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut saw_hunk = false;
    let mut in_hunk = false;
    for line in diff.split('\n') {
        if line.starts_with("@@") {
            saw_hunk = true;
            in_hunk = true;
            continue;
        }
        if line.starts_with("diff --git") {
            in_hunk = false;
            continue;
        }
        if !in_hunk
            && (line.starts_with("index ") || line.starts_with("--- ") || line.starts_with("+++ "))
        {
            continue;
        }
        if line.starts_with("\\ ") {
            continue;
        }
        if let Some(line) = line.strip_prefix('+') {
            after.push(line);
        } else if let Some(line) = line.strip_prefix('-') {
            before.push(line);
        } else if let Some(line) = line.strip_prefix(' ') {
            before.push(line);
            after.push(line);
        } else if in_hunk {
            before.push(line);
            after.push(line);
        }
    }
    if !saw_hunk && before.is_empty() && after.is_empty() {
        return (String::new(), diff.to_owned());
    }
    (before.join("\n"), after.join("\n"))
}

/// Replay one app-server `ThreadItem` through the same event vocabulary used
/// for live notifications. Stored user input is the only special case because
/// the live UI already owns the prompt it just sent.
#[must_use]
pub fn map_stored_item(item: &Map<String, Value>) -> Vec<EngineEvent> {
    let item_type = text(Some(item), "type").unwrap_or_default();
    if item_type == "userMessage" {
        let (text, images, image_references) = user_message_content(item);
        if text.is_none() && images.is_empty() {
            return Vec::new();
        }
        return vec![kind(EngineEventKind::UserMessage {
            text: text.unwrap_or_default(),
            agent_key: None,
            images: (!images.is_empty()).then_some(images),
            image_references: (!image_references.is_empty()).then_some(image_references),
        })];
    }

    let mut output = Vec::new();
    if item_type == "agentMessage" {
        if let Some(message) = text(Some(item), "text") {
            output.push(kind(EngineEventKind::MessageDelta {
                text: message.to_owned(),
                agent_key: None,
            }));
        }
    }
    let mut mapper = CodexEventMapper::default();
    output.extend(mapper.map_item_started(Some(item)));
    output.extend(mapper.map_item_completed(Some(item)));
    output
}

/// Whether a stored item is an owner prompt exactly as
/// [`stored_history_page`] shows one: a `userMessage` with text or an image
/// left once host context is removed.
fn is_stored_prompt(item: &Map<String, Value>) -> bool {
    if text(Some(item), "type") != Some("userMessage") {
        return false;
    }
    let (text, images, _) = user_message_content(item);
    text.is_some() || !images.is_empty()
}

/// The provider id of the turn `boundary` names in a `thread/read` turn
/// list, for `thread/fork`'s inclusive `lastTurnId`. Each turn holding an
/// owner prompt (as the stored history shows prompts) is known by its first
/// prompt — a prompt steered into a running turn belongs to that turn, just
/// as the thread host records one turn per prompt that started one — and
/// the turns are walked from the newest, so prompts the host never recorded
/// (history from before the thread, a terminal's) cannot shift the cut.
/// Turns with no owner prompt (host context only) are skipped. `Err`
/// explains why no such turn can be forked after.
pub(crate) fn fork_turn_boundary(
    turns: &[Value],
    boundary: &ForkBoundary,
) -> Result<String, String> {
    let mut unloaded = false;
    let prompts = turns
        .iter()
        .rev()
        .filter_map(Value::as_object)
        .map_while(|turn| {
            if text(Some(turn), "itemsView").unwrap_or("full") != "full" {
                unloaded = true;
                return None;
            }
            Some(turn)
        })
        .filter_map(|turn| {
            let prompt = turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|item| is_stored_prompt(item))?;
            Some((turn, user_message_content(prompt).0.unwrap_or_default()))
        });
    let Some(turn) = boundary.find_newest_first(prompts) else {
        return Err(if unloaded {
            "Codex did not load every earlier turn, so Interface cannot find where that turn ends."
                .into()
        } else {
            FORK_BOUNDARY_NOT_FOUND.into()
        });
    };
    if text(Some(turn), "status") == Some("inProgress") {
        return Err("That turn is still running. Fork after it finishes.".into());
    }
    text(Some(turn), "id")
        .map(ToOwned::to_owned)
        .ok_or_else(|| "Codex did not name that turn.".into())
}

enum StoredEntry<'a> {
    Item(&'a Map<String, Value>),
    Gap { view: String, turns: u64 },
}

/// Tail-bound a `thread/read` response by item count while reporting message
/// counts separately. A provider-declared incomplete turn is represented by a
/// retryable transcript notice, never silently replayed as complete history.
#[must_use]
pub fn stored_history_page(turns: &[Value], limit: usize) -> HistoryPage {
    let mut entries = Vec::new();
    let mut total = 0_usize;
    for turn in turns.iter().filter_map(Value::as_object) {
        let view = text(Some(turn), "itemsView").unwrap_or("full");
        if view != "full" {
            match entries.last_mut() {
                Some(StoredEntry::Gap {
                    view: previous,
                    turns,
                }) if previous == view => *turns += 1,
                _ => entries.push(StoredEntry::Gap {
                    view: view.to_owned(),
                    turns: 1,
                }),
            }
        }
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            entries.push(StoredEntry::Item(item));
            total += 1;
        }
    }

    let kept = if limit == 0 { total } else { total.min(limit) };
    let cut = total.saturating_sub(kept);
    let mut dropped = 0_usize;
    let mut messages = 0_u64;
    let mut older_messages = 0_u64;
    let mut events = Vec::new();
    for entry in entries {
        match entry {
            StoredEntry::Item(item) => {
                let message = matches!(
                    text(Some(item), "type"),
                    Some("userMessage" | "agentMessage")
                );
                if dropped < cut {
                    dropped += 1;
                    older_messages += u64::from(message);
                    continue;
                }
                messages += u64::from(message);
                events.extend(map_stored_item(item));
            }
            StoredEntry::Gap { view, turns } => {
                if dropped >= cut {
                    events.push(kind(EngineEventKind::Error {
                        cause: None,
                        message: gap_notice(&view, turns),
                        retryable: true,
                    }));
                }
            }
        }
    }
    HistoryPage {
        availability: None,
        reason: None,
        events,
        messages: Some(messages),
        older_messages,
    }
}

fn gap_notice(view: &str, turns: u64) -> String {
    let count = if turns == 1 {
        "1 turn".to_owned()
    } else {
        format!("{turns} turns")
    };
    let they = if turns == 1 { "it is" } else { "they are" };
    match view {
        "notLoaded" => format!(
            "Codex did not load {count} of this conversation, so {they} not shown here (thread/read reported itemsView \"notLoaded\")."
        ),
        "summary" => format!(
            "Codex returned {count} as a display summary rather than the full exchange, so what follows is that summary and not everything that was said (itemsView \"summary\")."
        ),
        _ => format!(
            "Codex reported {count} as itemsView \"{view}\", which this build does not recognise as a complete transcript — what follows may be incomplete."
        ),
    }
}

fn user_message_content(
    item: &Map<String, Value>,
) -> (
    Option<String>,
    Vec<interface_contracts::prompt_images::PromptTurnImage>,
    Vec<PromptImageReference>,
) {
    let mut parts = Vec::new();
    let mut urls = Vec::new();
    let mut references = Vec::new();
    let mut next_text_line = 0_u64;
    for part in item
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        let part_type = text(Some(part), "type");
        if part_type == Some("image") && urls.len() < PROMPT_IMAGE_LIMITS.max_turn_images {
            let url = part
                .get("url")
                .or_else(|| part.get("image_url"))
                .and_then(Value::as_str);
            if let Some(url) = url.filter(|url| is_prompt_image_preview_url(url)) {
                urls.push(url.to_owned());
                continue;
            }
        }
        let Some(rendered) = user_input_text(part) else {
            continue;
        };
        if is_collaboration_host_context(&rendered) {
            continue;
        }
        if matches!(part_type, Some("image" | "localImage")) {
            let path = (part_type == Some("localImage"))
                .then(|| text(Some(part), "path"))
                .flatten();
            references.push(PromptImageReference {
                text_line: next_text_line,
                text_line_count: u64::try_from(rendered.split('\n').count()).unwrap_or(u64::MAX),
                label: prompt_image_basename(path),
            });
        }
        next_text_line = next_text_line
            .saturating_add(u64::try_from(rendered.split('\n').count()).unwrap_or(u64::MAX));
        parts.push(rendered);
    }
    (
        (!parts.is_empty()).then(|| parts.join("\n")),
        replayed_turn_images(&urls),
        references,
    )
}

fn user_input_text(part: &Map<String, Value>) -> Option<String> {
    let part_type = text(Some(part), "type")?;
    let name = || {
        text(Some(part), "name")
            .or_else(|| text(Some(part), "path"))
            .map(ToOwned::to_owned)
    };
    Some(match part_type {
        "text" => text(Some(part), "text")?.to_owned(),
        "mention" => name().map_or_else(|| "[mention]".into(), |name| format!("@{name}")),
        "skill" => name().map_or_else(|| "[skill]".into(), |name| format!("[skill: {name}]")),
        "image" => "[image]".into(),
        "audio" => "[audio]".into(),
        "localImage" => text(Some(part), "path")
            .map_or_else(|| "[image]".into(), |path| format!("[image: {path}]")),
        "localAudio" => text(Some(part), "path")
            .map_or_else(|| "[audio]".into(), |path| format!("[audio: {path}]")),
        other => format!("[{other}]"),
    })
}

/// Pure mapper for `account/rateLimits/read` plus the optional
/// `account/usage/read` enrichment.
///
/// # Panics
///
/// Panics only if the compile-time built-in `codex` engine identifier stops
/// satisfying the shared identifier contract.
#[must_use]
pub fn map_engine_usage(limits: &Value, token_usage: Option<&Value>) -> EngineUsage {
    let response = limits.as_object();
    let top = response
        .and_then(|value| value.get("rateLimits"))
        .and_then(Value::as_object);
    let mut snapshots = Vec::new();
    if let Some(by_id) = response
        .and_then(|value| value.get("rateLimitsByLimitId"))
        .and_then(Value::as_object)
    {
        let default_id = top.and_then(|value| text(Some(value), "limitId"));
        let mut entries = by_id
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_object()
                    .map(|snapshot| (text(Some(snapshot), "limitId").unwrap_or(key), snapshot))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(id, _)| usize::from(Some(*id) != default_id));
        snapshots.extend(entries.into_iter().map(|(_, snapshot)| snapshot));
    }
    if snapshots.is_empty() {
        snapshots.extend(top);
    }
    let qualify = snapshots.len() > 1;
    let mut windows = Vec::new();
    for snapshot in &snapshots {
        let qualifier = qualify.then(|| {
            text(Some(snapshot), "limitName")
                .or_else(|| text(Some(snapshot), "limitId"))
                .unwrap_or_default()
        });
        for slot in ["primary", "secondary"] {
            let Some(window) = snapshot.get(slot).and_then(Value::as_object) else {
                continue;
            };
            let base = window_label(window.get("windowDurationMins"), slot);
            windows.push(UsageWindow {
                label: qualifier
                    .filter(|value| !value.is_empty())
                    .map_or(base.clone(), |name| format!("{name} {base}")),
                utilization: percentage(window.get("usedPercent")),
                resets_at: reset_iso(window.get("resetsAt")),
            });
        }
        if let Some(spend) = snapshot.get("individualLimit").and_then(Value::as_object) {
            let remaining = percentage(spend.get("remainingPercent"));
            windows.push(UsageWindow {
                label: qualifier
                    .filter(|value| !value.is_empty())
                    .map_or_else(|| "spend".into(), |name| format!("{name} spend")),
                utilization: remaining.map(|value| 100.0 - value),
                resets_at: reset_iso(spend.get("resetsAt")),
            });
        }
    }
    let plan = top
        .and_then(|value| text(Some(value), "planType"))
        .or_else(|| {
            snapshots
                .first()
                .and_then(|value| text(Some(value), "planType"))
        })
        .map(ToOwned::to_owned);
    let note = usage_note(response, top, token_usage, !windows.is_empty());
    EngineUsage {
        engine: EngineId::new("codex").expect("built-in Codex id"),
        available: !windows.is_empty(),
        plan,
        windows,
        cost_usd: None,
        note,
        account: None,
        account_label: None,
    }
}

fn window_label(value: Option<&Value>, slot: &str) -> String {
    let Some(minutes) = value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return slot.to_owned();
    };
    if (minutes / 1_440.0).fract() == 0.0 {
        format!("{:.0}-day", minutes / 1_440.0)
    } else if (minutes / 60.0).fract() == 0.0 {
        format!("{:.0}-hour", minutes / 60.0)
    } else {
        format!("{minutes}-minute")
    }
}

fn percentage(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .map(|value| value.round().clamp(0.0, 100.0))
}

fn reset_iso(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .map(super::adapter::unix_timestamp)
}

fn usage_note(
    response: Option<&Map<String, Value>>,
    top: Option<&Map<String, Value>>,
    token_usage: Option<&Value>,
    have_windows: bool,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(notice) = top.and_then(limit_reached_notice) {
        parts.push(notice);
    }
    if let Some(credits) = top
        .and_then(|value| value.get("credits"))
        .and_then(Value::as_object)
    {
        if credits.get("unlimited").and_then(Value::as_bool) == Some(true) {
            parts.push("unlimited credits".into());
        } else if credits.get("hasCredits").and_then(Value::as_bool) == Some(true) {
            parts.push(text(Some(credits), "balance").map_or_else(
                || "credits available".into(),
                |balance| format!("credits {balance}"),
            ));
        } else {
            parts.push("no credits".into());
        }
    }
    let resets = response
        .and_then(|value| value.get("rateLimitResetCredits"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("availableCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if resets > 0 {
        parts.push(format!(
            "{resets} limit-reset credit{} available",
            if resets == 1 { "" } else { "s" }
        ));
    }
    if let Some(lifetime) = token_usage
        .and_then(Value::as_object)
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("lifetimeTokens"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        parts.push(format!("{} tokens all-time", compact_count(lifetime)));
    }
    let latest = token_usage
        .and_then(Value::as_object)
        .and_then(|value| value.get("dailyUsageBuckets"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|bucket| {
            Some((
                text(Some(bucket), "startDate")?,
                bucket.get("tokens")?.as_f64()?,
            ))
        })
        .filter(|(_, tokens)| tokens.is_finite())
        .max_by_key(|(date, _)| *date);
    if let Some((date, tokens)) = latest {
        parts.push(format!("{} on {date}", compact_count(tokens)));
    }
    if !parts.is_empty() {
        Some(parts.join(" · "))
    } else if have_windows {
        None
    } else {
        Some("The Codex engine reported no plan limits for this account.".into())
    }
}

fn compact_count(value: f64) -> String {
    let absolute = value.abs();
    if absolute >= 1_000_000_000.0 {
        format!("{:.1}B", value / 1_000_000_000.0)
    } else if absolute >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if absolute >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

pub(crate) fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::new();
    for word in value.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

fn one_line(value: &str) -> String {
    clip_label(&collapse_whitespace(value), MAX_SUMMARY)
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::EngineEventKind;
    use serde_json::json;

    use super::*;

    #[test]
    fn map_into_reuses_the_caller_buffer_for_deltas() {
        let mut mapper = CodexEventMapper::default();
        let mut events = Vec::new();
        mapper.map_into(
            &mut events,
            "item/agentMessage/delta",
            Some(&json!({"delta": "a"})),
        );
        assert_eq!(events.len(), 1);
        events.clear();
        mapper.map_into(
            &mut events,
            "item/agentMessage/delta",
            Some(&json!({"delta": "b"})),
        );
        assert_eq!(
            events.as_slice(),
            [EngineEvent {
                at: None,
                event: EngineEventKind::MessageDelta {
                    text: "b".into(),
                    agent_key: None,
                },
            }]
        );
        events.clear();
        mapper.map_into(
            &mut events,
            "item/agentMessage/delta",
            Some(&json!({"delta": ""})),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn an_unauthorized_codex_turn_fails_with_the_sign_in_cause() {
        let mut mapper = CodexEventMapper::default();
        let failed = mapper.map(
            "turn/completed",
            Some(&json!({"turn": {"id": "t", "status": "failed", "error": {
                "message": "unexpected status 401 Unauthorized",
                "codexErrorInfo": "unauthorized"
            }}})),
        );
        assert!(matches!(
            failed.last().map(|event| &event.event),
            Some(EngineEventKind::Error {
                cause: Some(EngineErrorCause::SignedOut),
                retryable: false,
                ..
            })
        ));
        let notified = mapper.map(
            "error",
            Some(
                &json!({"error": {"message": "401", "codexErrorInfo": "unauthorized"},
                "willRetry": false}),
            ),
        );
        assert!(matches!(
            notified.last().map(|event| &event.event),
            Some(EngineEventKind::Error {
                cause: Some(EngineErrorCause::SignedOut),
                ..
            })
        ));
        let other = mapper.map(
            "error",
            Some(
                &json!({"error": {"message": "slow down", "codexErrorInfo": "usageLimitExceeded"}}),
            ),
        );
        assert!(matches!(
            other.last().map(|event| &event.event),
            Some(EngineEventKind::Error { cause: None, .. })
        ));
    }

    #[test]
    fn collapse_whitespace_skips_the_throwaway_word_vec() {
        assert_eq!(collapse_whitespace("  a\n\tb  "), "a b");
        assert_eq!(collapse_whitespace(""), "");
    }

    #[test]
    fn stored_tool_item_replays_started_and_completed_without_an_envelope() {
        let item = json!({
            "type": "commandExecution",
            "id": "exec-1",
            "command": "echo",
            "status": "completed",
            "exitCode": 0,
            "aggregatedOutput": "ok"
        });
        assert_eq!(
            serde_json::to_value(map_stored_item(item.as_object().unwrap())).unwrap(),
            json!([
                {"kind":"tool.call", "callId":"exec-1", "tool":"commandExecution",
                    "summary":"echo", "input": item},
                {"kind":"tool.result", "callId":"exec-1", "ok":true, "summary":"exit 0 — ok"}
            ])
        );
    }

    #[test]
    fn private_reasoning_is_only_an_activity_signal() {
        let mut mapper = CodexEventMapper::default();
        let events = mapper.map(
            "item/reasoning/textDelta",
            Some(&json!({"delta": "private text"})),
        );
        assert!(matches!(
            events.as_slice(),
            [EngineEvent {
                event: EngineEventKind::ThinkingSignal { active: true, .. },
                ..
            }]
        ));
    }

    #[test]
    fn usage_and_hard_rate_limit_are_distinct() {
        let mut mapper = CodexEventMapper::default();
        let usage = mapper.map(
            "thread/tokenUsage/updated",
            Some(&json!({"tokenUsage": {
                "total": {"inputTokens": 12, "outputTokens": 3},
                "last": {"totalTokens": 15}, "modelContextWindow": 100
            }})),
        );
        assert_eq!(usage.len(), 2);
        let ordinary_limit = mapper.map(
            "account/rateLimits/updated",
            Some(&json!({"rateLimits": {"primary": {"usedPercent": 100}}})),
        );
        assert!(ordinary_limit.is_empty());
        let blocked = mapper.map(
            "account/rateLimits/updated",
            Some(&json!({"rateLimits": {"rateLimitReachedType": "rate_limit_reached"}})),
        );
        assert!(matches!(
            blocked[0].event,
            EngineEventKind::UsageUpdated {
                rate_limited: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn patch_reconstructs_both_changed_sides() {
        assert_eq!(
            unified_diff_to_before_after("@@ -1,2 +1,2 @@\n-old\n+new\n same"),
            ("old\nsame".into(), "new\nsame".into())
        );
    }

    #[test]
    fn tool_events_match_the_frozen_app_server_oracle() {
        let mut mapper = CodexEventMapper::default();
        let command = json!({
            "type":"commandExecution", "id":"exec-1", "command":"rg --files\nsrc",
            "status":"inProgress"
        });
        assert_eq!(
            serde_json::to_value(
                mapper.map("item/started", Some(&json!({"item":command.clone()})))
            )
            .unwrap(),
            json!([{"kind":"tool.call", "callId":"exec-1",
                "tool":"commandExecution", "summary":"rg --files src", "input":command}])
        );
        assert_eq!(
            serde_json::to_value(mapper.map(
                "item/completed",
                Some(&json!({"item":{
                    "type":"commandExecution", "id":"exec-1", "command":"rg --files\nsrc",
                    "status":"completed", "exitCode":0,
                    "aggregatedOutput":"src/a.ts\nsrc/b.ts"
                }}))
            ))
            .unwrap(),
            json!([{"kind":"tool.result", "callId":"exec-1", "ok":true,
                "summary":"exit 0 — src/a.ts src/b.ts"}])
        );

        let mcp = json!({"type":"mcpToolCall", "id":"mcp-1", "server":"proof",
            "tool":"prepare", "arguments":{}, "status":"inProgress"});
        assert_eq!(
            serde_json::to_value(mapper.map("item/started", Some(&json!({"item":mcp.clone()}))))
                .unwrap(),
            json!([{"kind":"tool.call", "callId":"mcp-1", "tool":"proof/prepare",
                "summary":"proof/prepare", "input":mcp}])
        );
        assert_eq!(
            serde_json::to_value(mapper.map(
                "item/completed",
                Some(&json!({"item":{"type":"mcpToolCall", "id":"mcp-1",
                    "server":"proof", "tool":"prepare", "status":"completed",
                    "error":{"message":"unsupported fields"}}}))
            ))
            .unwrap(),
            json!([{"kind":"tool.result", "callId":"mcp-1", "ok":false,
                "summary":"unsupported fields"}])
        );
    }

    #[test]
    fn failures_and_rate_reset_keep_exact_provider_details() {
        let mut mapper = CodexEventMapper::default();
        assert_eq!(
            serde_json::to_value(mapper.map(
                "turn/completed",
                Some(&json!({"turn":{"status":"failed", "error":{
                    "message":"stream disconnected", "additionalDetails":"after 3 attempts"
                }}}))
            ))
            .unwrap(),
            json!([
                {"kind":"thinking.signal", "active":false},
                {"kind":"error", "message":"stream disconnected: after 3 attempts",
                    "retryable":false}
            ])
        );
        assert_eq!(
            serde_json::to_value(mapper.map(
                "account/rateLimits/updated",
                Some(
                    &json!({"rateLimits":{"rateLimitReachedType":"rate_limit_reached",
                    "primary":{"resetsAt":86400}}})
                )
            ))
            .unwrap(),
            json!([{"kind":"usage.updated", "inputTokens":0, "outputTokens":0,
                "rateLimited":{"message":"Rate limit reached (rate_limit_reached); resets 1970-01-02T00:00:00.000Z",
                    "resetsAt":"1970-01-02T00:00:00.000Z"}}])
        );
    }

    /// T3 section 10: a limit Codex reports keeps its reset as a field (the
    /// first window that has one, in the usage windows' ISO form); a limit
    /// with no reset, or a reset of zero, carries none.
    #[test]
    fn a_reached_limit_keeps_its_reset_and_an_unknown_one_stays_absent() {
        let mut mapper = CodexEventMapper::default();
        let notice = |mapper: &mut CodexEventMapper, params: Value| match mapper
            .map("account/rateLimits/updated", Some(&params))
            .into_iter()
            .next()
            .map(|event| event.event)
        {
            Some(EngineEventKind::UsageUpdated {
                rate_limited: Some(notice),
                ..
            }) => notice,
            other => panic!("expected a rate limit, got {other:?}"),
        };
        let kept = notice(
            &mut mapper,
            json!({"rateLimits":{"rateLimitReachedType":"rate_limit_reached",
                "primary":null, "secondary":{"resetsAt":1_795_011_491_u64}}}),
        );
        assert_eq!(kept.resets_at.as_deref(), Some("2026-11-18T14:18:11.000Z"));
        for params in [
            json!({"rateLimits":{"rateLimitReachedType":"rate_limit_reached"}}),
            json!({"rateLimits":{"spendControlReached":true, "primary":{"resetsAt":0}}}),
        ] {
            let none = notice(&mut mapper, params);
            assert_eq!(none.resets_at, None);
            assert!(!none.message.contains("resets"));
        }
        assert!(
            mapper
                .map(
                    "account/rateLimits/updated",
                    Some(&json!({"rateLimits":{"rateLimitReachedType":null,
                        "primary":{"resetsAt":1_795_011_491_u64}}}))
                )
                .is_empty()
        );
    }

    #[test]
    fn file_change_and_diff_header_content_match_the_oracle() {
        let mut mapper = CodexEventMapper::default();
        let change = json!({"path":"/repo/a.ts", "kind":{"type":"update"},
            "diff":"@@ -1 +1 @@\n--- content\n+++ replacement"});
        let started = mapper.map(
            "item/started",
            Some(&json!({"item":{"type":"fileChange", "id":"patch-1",
                "changes":[change.clone()], "status":"inProgress"}})),
        );
        assert_eq!(
            serde_json::to_value(&started[0]).unwrap(),
            json!({"kind":"tool.call", "callId":"patch-1", "tool":"fileChange",
                "summary":"update /repo/a.ts", "input":{"type":"fileChange",
                "id":"patch-1", "changes":[change], "status":"inProgress"}})
        );
        assert_eq!(
            unified_diff_to_before_after("@@ -1 +1 @@\n--- content\n+++ replacement"),
            ("-- content".into(), "++ replacement".into())
        );
    }

    #[test]
    fn replayed_host_context_is_never_rendered_as_user_text() {
        let host_context = json!({"type":"text",
            "text":"[Interface host context — not user text]\nInterface host context: your \
                collaboration sourceSessionKey is \"codex#fresh-thread\"."});
        let item = json!({"type":"userMessage",
            "content":[host_context.clone(), {"type":"text","text":"hello"}]});
        assert_eq!(
            serde_json::to_value(map_stored_item(item.as_object().unwrap())).unwrap(),
            json!([{"kind":"user.message", "text":"hello"}])
        );
        let alone = json!({"type":"userMessage", "content":[host_context]});
        assert!(map_stored_item(alone.as_object().unwrap()).is_empty());
    }

    #[test]
    fn cross_language_image_history_fixture_matches_rust_events() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/provider-history-user-image-events.json"
        ))
        .unwrap();
        for name in ["codexInline", "codexLocalImage"] {
            let case = &fixture[name];
            let input = case["input"].as_object().unwrap();
            assert_eq!(
                serde_json::to_value(map_stored_item(input)).unwrap(),
                case["events"],
                "fixture case {name}"
            );
        }
    }
}
