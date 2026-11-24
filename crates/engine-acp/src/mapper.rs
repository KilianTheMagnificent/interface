use std::collections::{HashMap, HashSet};

use interface_contracts::engine::{
    ContextUsageInput, EngineEvent, EngineEventKind, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    ProviderCommand, ThroughputBasis, normalize_context_usage,
};
use serde_json::{Map, Value};

use crate::protocol::{AcpSessionUpdate, UpdateKind};

const MAX_SUMMARY_CHARS: usize = 240;
const MAX_COMMANDS: usize = 256;
const MAX_OPEN_TOOLS: usize = 512;
const MAX_SEEN_DIFFS: usize = 512;

#[derive(Clone, Debug)]
struct ToolState {
    title: String,
    tool: String,
    seen_diffs: HashSet<String>,
    subagent: bool,
}

#[derive(Default)]
pub struct AcpEventMapper {
    tools: HashMap<String, ToolState>,
    thinking_active: bool,
}

impl AcpEventMapper {
    #[must_use]
    pub fn map(&mut self, update: &AcpSessionUpdate) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        self.map_into(&mut events, update);
        events
    }

    pub(crate) fn map_into(&mut self, out: &mut Vec<EngineEvent>, update: &AcpSessionUpdate) {
        match update.kind() {
            UpdateKind::AgentMessageChunk => out.extend(self.agent_message(&update.fields)),
            UpdateKind::UserMessageChunk => out.extend(Self::user_message(&update.fields)),
            UpdateKind::AgentThoughtChunk => {
                self.thinking_active = true;
                out.push(kind(EngineEventKind::ThinkingSignal {
                    active: true,
                    agent_key: None,
                }));
            }
            UpdateKind::ToolCall => out.extend(self.tool_call(&update.fields)),
            UpdateKind::ToolCallUpdate => out.extend(self.tool_call_update(&update.fields)),
            UpdateKind::Plan => out.extend(map_plan(&update.fields)),
            UpdateKind::AvailableCommandsUpdate => out.extend(map_commands(&update.fields)),
            UpdateKind::UsageUpdate => out.extend(map_context_usage(&update.fields)),
            UpdateKind::Unknown => {}
        }
    }

    fn agent_message(&mut self, fields: &Map<String, Value>) -> Vec<EngineEvent> {
        let mut events = request_usage_events(fields);
        let Some(text) = content_text(fields.get("content")) else {
            return events;
        };
        let delta = kind(EngineEventKind::MessageDelta {
            text: text.to_owned(),
            agent_key: None,
        });
        if self.thinking_active {
            self.thinking_active = false;
            events.insert(
                0,
                kind(EngineEventKind::ThinkingSignal {
                    active: false,
                    agent_key: None,
                }),
            );
            events.insert(1, delta);
        } else {
            events.insert(0, delta);
        }
        events
    }

    fn user_message(fields: &Map<String, Value>) -> Vec<EngineEvent> {
        content_text(fields.get("content"))
            .map(|text| {
                kind(EngineEventKind::UserMessage {
                    text: text.to_owned(),
                    agent_key: None,
                    images: None,
                    image_references: None,
                })
            })
            .into_iter()
            .collect()
    }

    fn tool_call(&mut self, fields: &Map<String, Value>) -> Vec<EngineEvent> {
        let Some(call_id) = text(fields, "toolCallId") else {
            return Vec::new();
        };
        let info = tool_info(fields, None);
        let subagent = is_subagent(fields);
        let mut state = ToolState {
            title: info.summary.clone(),
            tool: info.tool.clone(),
            seen_diffs: HashSet::new(),
            subagent,
        };
        let mut events = vec![kind(EngineEventKind::ToolCall {
            call_id: call_id.to_owned(),
            tool: info.tool,
            summary: info.summary.clone(),
            input: info
                .input
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
            agent_key: None,
        })];
        if subagent {
            events.push(kind(EngineEventKind::SubagentStarted {
                agent_key: call_id.to_owned(),
                subagent_type: subagent_field(fields, "subagent_type")
                    .unwrap_or("subagent")
                    .into(),
                description: subagent_field(fields, "description")
                    .unwrap_or(&info.summary)
                    .to_owned(),
                model: subagent_field(fields, "model").map(ToOwned::to_owned),
            }));
        }
        events.extend(diff_events(call_id, fields.get("content"), &mut state));
        self.track(call_id, state);
        events
    }

    /// Bounds the open-tool ledger. An agent that never terminates its tool
    /// calls stops adding entries instead of growing for the session's life.
    ///
    /// Updates for a call past the bound still map, but they are stateless: the
    /// rename check compares against a fresh default every time, so each update
    /// re-emits a `ToolCall`; diffs are never deduplicated; and `settle_open`
    /// cannot settle the call, so no synthetic `ToolResult` closes it.
    fn track(&mut self, call_id: &str, state: ToolState) {
        if self.tools.len() < MAX_OPEN_TOOLS || self.tools.contains_key(call_id) {
            self.tools.insert(call_id.to_owned(), state);
        }
    }

    fn tool_call_update(&mut self, fields: &Map<String, Value>) -> Vec<EngineEvent> {
        let Some(call_id) = text(fields, "toolCallId") else {
            return Vec::new();
        };
        if !self.tools.contains_key(call_id) && self.tools.len() < MAX_OPEN_TOOLS {
            self.tools.insert(call_id.to_owned(), default_tool_state());
        }
        let mut untracked = None;
        let state = match self.tools.get_mut(call_id) {
            Some(state) => state,
            None => untracked.insert(default_tool_state()),
        };
        let info = tool_info(fields, Some(&*state));
        let renamed = info.tool != state.tool || info.summary != state.title;
        state.tool.clone_from(&info.tool);
        state.title.clone_from(&info.summary);
        let mut events = Vec::new();
        if renamed {
            events.push(kind(EngineEventKind::ToolCall {
                call_id: call_id.to_owned(),
                tool: info.tool,
                summary: info.summary,
                input: info
                    .input
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new())),
                agent_key: None,
            }));
        }
        events.extend(diff_events(call_id, fields.get("content"), state));
        let status = text(fields, "status");
        if !matches!(status, Some("completed" | "failed")) {
            return events;
        }
        let ok = status == Some("completed");
        let child_text = result_content_text(fields);
        let summary = result_summary(fields, state, ok, child_text.as_deref());
        let subagent = state.subagent;
        self.tools.remove(call_id);
        if subagent {
            if let Some(text) = child_text {
                events.push(kind(EngineEventKind::MessageDelta {
                    text,
                    agent_key: Some(call_id.to_owned()),
                }));
            }
        }
        events.push(kind(EngineEventKind::ToolResult {
            call_id: call_id.to_owned(),
            ok,
            summary: summary.clone(),
            agent_key: None,
        }));
        if subagent {
            events.push(kind(EngineEventKind::SubagentFinished {
                agent_key: call_id.to_owned(),
                ok,
                summary,
            }));
        }
        events
    }

    #[must_use]
    pub fn settle_open(&mut self, summary: &str) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        for (call_id, state) in self.tools.drain() {
            events.push(kind(EngineEventKind::ToolResult {
                call_id: call_id.clone(),
                ok: false,
                summary: summary.to_owned(),
                agent_key: None,
            }));
            if state.subagent {
                events.push(kind(EngineEventKind::SubagentFinished {
                    agent_key: call_id,
                    ok: false,
                    summary: summary.to_owned(),
                }));
            }
        }
        self.thinking_active = false;
        events
    }
}

struct ToolInfo<'a> {
    tool: String,
    summary: String,
    input: Option<&'a Value>,
}

fn default_tool_state() -> ToolState {
    ToolState {
        title: "Tool call".into(),
        tool: "other".into(),
        seen_diffs: HashSet::new(),
        subagent: false,
    }
}

fn tool_info<'a>(fields: &'a Map<String, Value>, previous: Option<&ToolState>) -> ToolInfo<'a> {
    let payload = fields.get("rawInput").and_then(Value::as_object);
    let metadata = xai_tool(fields);
    let meta_name = metadata.and_then(|metadata| text(metadata, "name"));
    let title = text(fields, "title");
    let qualified = (meta_name == Some("use_tool"))
        .then(|| payload.and_then(|payload| text(payload, "tool_name")))
        .flatten();
    let search = meta_name == Some("search_tool") || title == Some("search_tool");
    let tool = qualified
        .or_else(|| search.then_some("search_tool"))
        .or_else(|| metadata.and_then(|metadata| text(metadata, "label")))
        .or_else(|| previous.map(|previous| previous.tool.as_str()))
        .or_else(|| text(fields, "kind"))
        .or_else(|| metadata.and_then(|metadata| text(metadata, "kind")))
        .or(meta_name)
        .or(title)
        .unwrap_or("other")
        .to_owned();
    let subject = qualified
        .or_else(|| {
            search
                .then(|| payload.and_then(|payload| text(payload, "query")))
                .flatten()
        })
        .or_else(|| payload.and_then(|payload| text(payload, "description")))
        .or_else(|| payload.and_then(|payload| text(payload, "target_file")))
        .or_else(|| payload.and_then(|payload| text(payload, "path")))
        .or_else(|| payload.and_then(|payload| text(payload, "command")))
        .or(title)
        .unwrap_or(&tool);
    let summary = clip_summary(subject);
    ToolInfo {
        tool,
        summary,
        input: fields.get("rawInput"),
    }
}

fn map_plan(fields: &Map<String, Value>) -> Vec<EngineEvent> {
    let Some(entries) = fields.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let entries = entries
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let priority = match text(entry, "priority")? {
                "high" => PlanEntryPriority::High,
                "medium" => PlanEntryPriority::Medium,
                "low" => PlanEntryPriority::Low,
                _ => return None,
            };
            let status = match text(entry, "status")? {
                "pending" => PlanEntryStatus::Pending,
                "in_progress" => PlanEntryStatus::InProgress,
                "completed" => PlanEntryStatus::Completed,
                _ => return None,
            };
            Some(PlanEntry {
                content: text(entry, "content")?.to_owned(),
                priority,
                status,
            })
        })
        .collect();
    vec![kind(EngineEventKind::PlanUpdated { entries })]
}

fn map_commands(fields: &Map<String, Value>) -> Vec<EngineEvent> {
    let Some(commands) = fields.get("availableCommands").and_then(Value::as_array) else {
        return Vec::new();
    };
    let commands = commands
        .iter()
        .take(MAX_COMMANDS)
        .filter_map(|command| {
            let command = command.as_object()?;
            let name = text(command, "name")?.trim_start_matches('/').to_owned();
            (!name.is_empty()).then(|| ProviderCommand {
                name,
                description: text(command, "description").unwrap_or_default().to_owned(),
                argument_hint: command
                    .get("input")
                    .and_then(Value::as_object)
                    .and_then(|input| text(input, "hint"))
                    .map(ToOwned::to_owned),
                aliases: None,
            })
        })
        .collect();
    vec![kind(EngineEventKind::CommandsAvailable { commands })]
}

fn map_context_usage(fields: &Map<String, Value>) -> Vec<EngineEvent> {
    let usage = normalize_context_usage(ContextUsageInput {
        used: non_negative_f64(fields.get("used")),
        size: non_negative_f64(fields.get("size")),
        ..ContextUsageInput::default()
    });
    usage
        .map(|usage| kind(EngineEventKind::ContextUsage { usage }))
        .into_iter()
        .collect()
}

fn request_usage_events(fields: &Map<String, Value>) -> Vec<EngineEvent> {
    let metadata = meta_object(fields);
    let usage = metadata
        .and_then(|metadata| metadata.get("usage"))
        .and_then(Value::as_object);
    let (Some(input_tokens), Some(output_tokens)) = (
        usage.and_then(|usage| non_negative_u64(usage.get("inputTokens"))),
        usage.and_then(|usage| non_negative_u64(usage.get("outputTokens"))),
    ) else {
        return Vec::new();
    };
    let mut events = vec![kind(EngineEventKind::UsageUpdated {
        input_tokens,
        output_tokens,
        rate_limited: None,
    })];
    if let Some(duration_ms) = metadata
        .and_then(|metadata| non_negative_u64(metadata.get("durationMs")))
        .filter(|duration| *duration > 0)
    {
        if output_tokens > 0 {
            events.push(kind(EngineEventKind::ThroughputUpdated {
                output_tokens,
                duration_ms,
                basis: ThroughputBasis::EndToEnd,
            }));
        }
    }
    events
}

fn diff_events(call_id: &str, content: Option<&Value>, state: &mut ToolState) -> Vec<EngineEvent> {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|content| {
            let content = content.as_object()?;
            if text(content, "type") != Some("diff") {
                return None;
            }
            let path = text(content, "path")?;
            let before = content
                .get("oldText")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let after = content.get("newText")?.as_str()?;
            let fingerprint = format!("{path}\0{before}\0{after}");
            // Past the bound the ledger stops growing and diffs are emitted
            // undeduplicated: a repeated DiffProduced is safe, a swallowed one
            // is not.
            if state.seen_diffs.len() < MAX_SEEN_DIFFS && !state.seen_diffs.insert(fingerprint) {
                return None;
            }
            Some(kind(EngineEventKind::DiffProduced {
                call_id: call_id.to_owned(),
                path: path.to_owned(),
                before: before.to_owned(),
                after: after.to_owned(),
                agent_key: None,
            }))
        })
        .collect()
}

fn is_subagent(fields: &Map<String, Value>) -> bool {
    subagent_tool_name(fields) == Some("spawn_subagent")
}

fn subagent_tool_name(fields: &Map<String, Value>) -> Option<&str> {
    xai_tool(fields)
        .and_then(|tool| text(tool, "name"))
        .or_else(|| text(fields, "title"))
}

fn meta_object(fields: &Map<String, Value>) -> Option<&Map<String, Value>> {
    fields
        .get("_meta")
        .or_else(|| fields.get("meta"))
        .and_then(Value::as_object)
}

fn xai_tool(fields: &Map<String, Value>) -> Option<&Map<String, Value>> {
    meta_object(fields)
        .and_then(|metadata| metadata.get("x.ai/tool"))
        .and_then(Value::as_object)
}

fn subagent_field<'a>(fields: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    fields
        .get("rawInput")
        .and_then(Value::as_object)
        .and_then(|input| text(input, key))
}

fn content_text(content: Option<&Value>) -> Option<&str> {
    let content = content?.as_object()?;
    (text(content, "type") == Some("text"))
        .then(|| text(content, "text"))
        .flatten()
}

fn result_content_text(fields: &Map<String, Value>) -> Option<String> {
    let text = fields
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let item = item.as_object()?;
            (text(item, "type") == Some("content"))
                .then(|| content_text(item.get("content")))
                .flatten()
        })
        .collect::<Vec<_>>();
    (!text.is_empty()).then(|| text.join("\n"))
}

fn result_summary(
    fields: &Map<String, Value>,
    state: &ToolState,
    ok: bool,
    child_text: Option<&str>,
) -> String {
    if let Some(text) = child_text {
        return clip_summary(text);
    }
    if let Some(raw) = fields.get("rawOutput") {
        if let Some(raw) = raw.as_str() {
            return clip_summary(raw);
        }
        return clip_summary(&bounded_json(raw));
    }
    format!(
        "{} {}",
        state.title,
        if ok { "completed" } else { "failed" }
    )
}

fn text<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|value| !value.is_empty())
}

fn non_negative_f64(value: Option<&Value>) -> Option<f64> {
    value?
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn non_negative_u64(value: Option<&Value>) -> Option<u64> {
    value?.as_u64()
}

/// Serializes only as much of `value` as `clip_summary` can possibly keep. An
/// unbounded `rawOutput` is never fully materialized to produce 240 characters.
///
/// The cap is on bytes, while `clip_summary` counts characters *after*
/// collapsing whitespace, so a value whose first kilobyte is mostly whitespace
/// or multi-byte text can yield a shorter summary here than an unbounded
/// serialization would. That is accepted: this is a preview line, and the
/// alternative is materializing a megabyte of JSON to trim it to 240 chars.
fn bounded_json(value: &Value) -> String {
    struct Bounded {
        buffer: Vec<u8>,
        limit: usize,
    }

    impl std::io::Write for Bounded {
        fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
            let room = self.limit.saturating_sub(self.buffer.len());
            if room == 0 {
                return Err(std::io::Error::other("summary bound reached"));
            }
            let take = chunk.len().min(room);
            self.buffer.extend_from_slice(&chunk[..take]);
            Ok(take)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // Four bytes per kept character, plus slack for a truncated final one.
    let mut bounded = Bounded {
        buffer: Vec::new(),
        limit: MAX_SUMMARY_CHARS * 4 + 64,
    };
    let _ = serde_json::to_writer(&mut bounded, value);
    String::from_utf8_lossy(&bounded.buffer).into_owned()
}

fn clip_summary(value: &str) -> String {
    let mut collapsed = String::new();
    let mut chars = 0usize;
    for word in value.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
            chars += 1;
        }
        collapsed.push_str(word);
        chars += word.chars().count();
        if chars > MAX_SUMMARY_CHARS {
            break;
        }
    }
    if chars <= MAX_SUMMARY_CHARS {
        return collapsed;
    }
    let mut output = collapsed
        .chars()
        .take(MAX_SUMMARY_CHARS - 1)
        .collect::<String>();
    output.push('…');
    output
}

fn kind(event: EngineEventKind) -> EngineEvent {
    EngineEvent { at: None, event }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    fn update(kind: &str, value: &Value) -> AcpSessionUpdate {
        let mut fields = value.as_object().cloned().unwrap_or_else(Map::new);
        fields.remove("sessionUpdate");
        AcpSessionUpdate {
            session_update: kind.into(),
            fields,
        }
    }

    #[test]
    fn thought_is_private_and_visible_answer_turns_spinner_off_once() {
        let mut mapper = AcpEventMapper::default();
        let thought = mapper.map(&update(
            "agent_thought_chunk",
            &json!({"content": {"type": "text", "text": "secret"}}),
        ));
        assert!(matches!(
            thought[0].event,
            EngineEventKind::ThinkingSignal { active: true, .. }
        ));
        let answer = mapper.map(&update(
            "agent_message_chunk",
            &json!({"content": {"type": "text", "text": "answer"}}),
        ));
        assert!(matches!(
            answer[0].event,
            EngineEventKind::ThinkingSignal { active: false, .. }
        ));
        assert!(matches!(
            answer[1].event,
            EngineEventKind::MessageDelta { .. }
        ));
    }

    #[test]
    fn map_into_appends_to_the_caller_buffer_without_clearing_it() {
        let mut mapper = AcpEventMapper::default();
        let mut events = vec![kind(EngineEventKind::StatusChanged {
            status: interface_contracts::engine::SessionStatus::Working,
        })];
        mapper.map_into(
            &mut events,
            &update(
                "agent_message_chunk",
                &json!({"content": {"type": "text", "text": "a"}}),
            ),
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1].event,
            EngineEventKind::MessageDelta { .. }
        ));
    }

    #[test]
    fn diff_dedup_stops_growing_and_keeps_emitting_past_its_bound() {
        let mut mapper = AcpEventMapper::default();
        let _ = mapper.map(&update("tool_call", &json!({"toolCallId": "t-1"})));
        for index in 0..=MAX_SEEN_DIFFS {
            let events = mapper.map(&update(
                "tool_call_update",
                &json!({"toolCallId": "t-1", "content": [
                    {"type": "diff", "path": format!("/f{index}"), "newText": "x"}
                ]}),
            ));
            assert_eq!(events.len(), 1, "diff {index} was swallowed");
        }
        assert_eq!(mapper.tools["t-1"].seen_diffs.len(), MAX_SEEN_DIFFS);
    }

    #[test]
    fn open_tool_ledger_stops_growing_past_its_bound() {
        let mut mapper = AcpEventMapper::default();
        for index in 0..=MAX_OPEN_TOOLS {
            let _ = mapper.map(&update(
                "tool_call",
                &json!({"toolCallId": format!("t-{index}")}),
            ));
        }
        assert_eq!(mapper.tools.len(), MAX_OPEN_TOOLS);
    }

    #[test]
    fn tool_diff_and_terminal_update_keep_the_same_call_id() {
        let mut mapper = AcpEventMapper::default();
        let started = mapper.map(&update(
            "tool_call",
            &json!({
                "toolCallId": "call-1", "title": "Edit", "kind": "edit",
                "rawInput": {"path": "/tmp/a"},
                "content": [{"type":"diff","path":"/tmp/a","oldText":"old","newText":"new"}]
            }),
        ));
        assert_eq!(started.len(), 2);
        let finished = mapper.map(&update(
            "tool_call_update",
            &json!({"toolCallId":"call-1","status":"completed","rawOutput":"done"}),
        ));
        assert!(matches!(
            &finished.last().unwrap().event,
            EngineEventKind::ToolResult { call_id, ok: true, .. } if call_id == "call-1"
        ));
    }

    #[test]
    fn repeated_diff_blocks_emit_once() {
        let mut mapper = AcpEventMapper::default();
        let payload = json!({
            "toolCallId": "call-1", "title": "Edit", "kind": "edit",
            "rawInput": {"path": "/tmp/a"},
            "content": [{"type":"diff","path":"/tmp/a","oldText":"old","newText":"new"}]
        });
        let started = mapper.map(&update("tool_call", &payload));
        assert_eq!(
            started
                .iter()
                .filter(|event| matches!(event.event, EngineEventKind::DiffProduced { .. }))
                .count(),
            1
        );
        let repeated = mapper.map(&update("tool_call_update", &payload));
        assert!(
            repeated
                .iter()
                .all(|event| { !matches!(event.event, EngineEventKind::DiffProduced { .. }) })
        );
    }

    #[test]
    fn streaming_tool_update_skips_tool_call_unless_renamed() {
        let mut mapper = AcpEventMapper::default();
        let _ = mapper.map(&update(
            "tool_call",
            &json!({
                "toolCallId": "call-1", "title": "Edit", "kind": "edit",
                "rawInput": {"path": "/tmp/a"}
            }),
        ));
        let same = mapper.map(&update(
            "tool_call_update",
            &json!({
                "toolCallId": "call-1",
                "rawInput": {"path": "/tmp/a", "contents": "x".repeat(32)}
            }),
        ));
        assert!(
            same.iter()
                .all(|event| !matches!(event.event, EngineEventKind::ToolCall { .. }))
        );
        let renamed = mapper.map(&update(
            "tool_call_update",
            &json!({
                "toolCallId": "call-1",
                "rawInput": {"path": "/tmp/b"}
            }),
        ));
        assert!(matches!(
            &renamed[0].event,
            EngineEventKind::ToolCall { summary, input, .. }
                if summary == "/tmp/b" && input == &json!({"path": "/tmp/b"})
        ));
    }

    #[test]
    fn result_summary_clips_long_raw_output_after_whitespace_collapse() {
        let mut mapper = AcpEventMapper::default();
        let _ = mapper.map(&update(
            "tool_call",
            &json!({"toolCallId": "call-1", "title": "edit", "kind": "edit"}),
        ));
        let exact = mapper.map(&update(
            "tool_call_update",
            &json!({
                "toolCallId": "call-1",
                "status": "completed",
                "rawOutput": format!("  {}  ", "a".repeat(MAX_SUMMARY_CHARS))
            }),
        ));
        match &exact[0].event {
            EngineEventKind::ToolResult { summary, .. } => {
                assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
                assert!(!summary.ends_with('…'));
            }
            other => panic!("expected tool result, got {other:?}"),
        }

        let mut mapper = AcpEventMapper::default();
        let _ = mapper.map(&update(
            "tool_call",
            &json!({"toolCallId": "call-2", "title": "edit", "kind": "edit"}),
        ));
        let overflow = mapper.map(&update(
            "tool_call_update",
            &json!({
                "toolCallId": "call-2",
                "status": "completed",
                "rawOutput": "a".repeat(MAX_SUMMARY_CHARS + 1)
            }),
        ));
        match &overflow[0].event {
            EngineEventKind::ToolResult { summary, .. } => {
                assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
                assert!(summary.ends_with('…'));
                assert_eq!(
                    summary.chars().take(MAX_SUMMARY_CHARS - 1).count(),
                    MAX_SUMMARY_CHARS - 1
                );
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }
}
