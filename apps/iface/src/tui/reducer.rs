//! Pure reduction of provider events and stored history into one session
//! row. Nothing here takes a lock, awaits, or calls a provider.
use interface_contracts::engine::FastModeState;
use interface_contracts::ipc::HistoryAvailability;

use super::{
    AgentSessionState, AgentStatus, CellTone, CommandCatalogue, ControlledSession, EngineEvent,
    EngineEventKind, LaunchScope, NoticeTone, PlanEntry, SessionStatus, SessionSummary,
    ToolResultCell, Transcript, TranscriptCell, VecDeque, approval::build_approval,
    capabilities_for, for_display,
};
use crate::diagnostics::{ContextTelemetryCategory, ContextTelemetryPhase, ContextTelemetrySample};

pub(super) fn replace_provider_history(
    row: &mut ControlledSession,
    history: interface_contracts::ipc::HistoryPage,
) {
    row.transcript.clear();
    if history.availability == Some(HistoryAvailability::Unavailable) {
        push_notice(
            row,
            history
                .reason
                .as_deref()
                .unwrap_or("History unavailable for this provider."),
            CellTone::Warning,
        );
    } else {
        for event in history.events {
            reduce_event(row, &event);
        }
        if history.older_messages > 0 {
            push_notice(
                row,
                &format!(
                    "{} older stored messages are not shown.",
                    history.older_messages
                ),
                CellTone::Info,
            );
        }
    }
    row.state.live = false;
    row.state.status = AgentStatus::NotLive;
    row.pending_approval = None;
}

pub(super) fn controlled_from_summary(key: String, summary: SessionSummary) -> ControlledSession {
    let capabilities = capabilities_for(&summary.engine);
    let last_activity = summary.last_activity.clone();
    ControlledSession {
        state: AgentSessionState {
            key,
            engine: summary.engine.to_string(),
            session_id: summary.session_id,
            status: AgentStatus::NotLive,
            cwd: summary.cwd,
            title: summary.title,
            model: summary.model,
            task: None,
            capabilities: Some(capabilities),
            live: false,
        },
        transcript: Transcript::default(),
        queued: VecDeque::new(),
        pending_approval: None,
        effort: summary.effort.flatten(),
        access: None,
        fast_mode: false,
        models: Vec::new(),
        commands: CommandCatalogue::default(),
        setting_sources: summary.setting_sources,
        disallowed_tools: summary.disallowed_tools,
        context: None,
        previous_context: None,
        last_activity,
        telemetry_startup_recorded: false,
        telemetry_pending: false,
        telemetry_recorded: None,
    }
}

pub(super) fn controlled_from_live_summary(
    key: String,
    summary: SessionSummary,
) -> ControlledSession {
    let status = map_status(summary.status);
    let mut row = controlled_from_summary(key, summary);
    row.state.status = status;
    row.state.live = true;
    row
}

pub(super) fn merge_registry_summary(
    row: &mut ControlledSession,
    summary: SessionSummary,
    locked: bool,
) {
    if locked {
        row.state.status = map_status(summary.status);
        row.state.live = true;
    }
    row.state.cwd = summary.cwd;
    row.state.title = summary.title;
    row.state.model = summary.model;
    row.effort = summary.effort.flatten();
    row.setting_sources = summary.setting_sources;
    row.disallowed_tools = summary.disallowed_tools;
    row.last_activity = summary.last_activity;
}

#[allow(clippy::too_many_lines)]
pub(super) fn reduce_event(row: &mut ControlledSession, event: &EngineEvent) {
    if let Some(at) = &event.at {
        row.last_activity = for_display(at);
    }
    match &event.event {
        EngineEventKind::SessionMeta {
            model, cwd, title, ..
        } => {
            row.state.model = Some(for_display(model));
            row.state.cwd.clone_from(cwd);
            if let Some(title) = title {
                row.state.title = for_display(title);
            }
        }
        EngineEventKind::UserMessage {
            text,
            images,
            image_references,
            ..
        } => {
            let safe = for_display(text);
            if row
                .transcript
                .last()
                .is_none_or(|last| {
                    !matches!(last, TranscriptCell::Message { role: "user", markdown } if markdown == &safe)
                })
            {
                push_message(row, "user", &safe);
            }
            let image_count = images.as_ref().map_or(0, Vec::len).max(
                image_references.as_ref().map_or(0, Vec::len),
            );
            if image_count > 0
                && !matches!(
                    row.transcript.last(),
                    Some(TranscriptCell::ImageAttachments { count }) if *count == image_count
                )
            {
                push_cell(
                    row,
                    TranscriptCell::ImageAttachments { count: image_count },
                );
            }
        }
        EngineEventKind::MessageDelta { text, .. } => {
            let safe = for_display(text);
            if let Some(last) = row
                .transcript
                .last_mut()
                .and_then(|last| match last {
                    TranscriptCell::Message {
                        role: "assistant",
                        markdown,
                    } => Some(markdown),
                    _ => None,
                })
            {
                last.push_str(&safe);
            } else {
                push_message(row, "assistant", &safe);
            }
            row.state.status = AgentStatus::Working;
        }
        EngineEventKind::ToolCall {
            call_id,
            tool,
            summary,
            input,
            ..
        } => {
            let input_preview = display_unknown(input);
            let command_activity = is_command_activity(tool, input);
            if let Some(existing) = find_tool_mut(row, call_id) {
                if let TranscriptCell::Tool {
                    tool: current_tool,
                    summary: current_summary,
                    input_preview: current_input,
                    command_activity: current_command,
                    ..
                } = existing
                {
                    *current_tool = for_display(tool);
                    *current_summary = for_display(summary);
                    *current_input = input_preview;
                    *current_command = command_activity;
                }
            } else {
                push_cell(
                    row,
                    TranscriptCell::Tool {
                        call_id: call_id.clone(),
                        tool: for_display(tool),
                        summary: for_display(summary),
                        input_preview,
                        result: None,
                        command_activity,
                    },
                );
            }
            row.state.status = AgentStatus::Working;
        }
        EngineEventKind::ToolResult {
            call_id,
            ok,
            summary,
            ..
        } => {
            ensure_tool(row, call_id, "Unknown tool", "Result received before tool metadata.");
            if let Some(TranscriptCell::Tool { result, .. }) = find_tool_mut(row, call_id) {
                *result = Some(ToolResultCell {
                    ok: *ok,
                    summary: for_display(summary),
                });
            }
        }
        EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input,
        } => {
            row.pending_approval = Some(build_approval(approval_id, tool, summary, input));
            row.state.status = AgentStatus::Blocked;
        }
        EngineEventKind::StatusChanged { status } => {
            row.state.status = map_status(*status);
            if *status != SessionStatus::Waiting {
                row.pending_approval = None;
            }
        }
        EngineEventKind::UsageUpdated { rate_limited, .. } => {
            if let Some(rate_limited) = rate_limited {
                push_notice(row, &rate_limited.message, CellTone::Warning);
            }
        }
        EngineEventKind::ThroughputUpdated { .. } | EngineEventKind::ThinkingSignal { .. } => {}
        EngineEventKind::ContextUsage { usage } => {
            row.previous_context = row.context.replace(usage.clone());
        }
        EngineEventKind::DiffProduced {
            call_id,
            path,
            before,
            after,
            ..
        } => {
            ensure_tool(row, call_id, "File change", "Diff received before tool metadata.");
            let safe_path = for_display(path);
            let withheld = interface_workspace::is_search_excluded_path(path, false)
                || interface_workspace::is_search_excluded_path(&safe_path, false);
            let mut truncated = false;
            let before = (!withheld).then(|| diff_side(before, &mut truncated));
            let after = (!withheld).then(|| diff_side(after, &mut truncated));
            push_cell(
                row,
                TranscriptCell::Diff {
                    call_id: call_id.clone(),
                    path: safe_path,
                    before,
                    after,
                    withheld,
                    truncated,
                },
            );
        }
        EngineEventKind::Notice { message, tone } => {
            push_notice(
                row,
                message,
                match tone {
                    NoticeTone::Info => CellTone::Info,
                    NoticeTone::Warning => CellTone::Warning,
                },
            );
        }
        EngineEventKind::TurnRetry {
            attempt,
            max_attempts,
            delay_ms,
            message,
        } => {
            push_cell(
                row,
                TranscriptCell::Retry {
                    message: for_display(message),
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    delay_ms: *delay_ms,
                },
            );
        }
        EngineEventKind::Error { message, retryable, .. } => {
            push_notice(row, message, CellTone::Error);
            if !retryable {
                row.state.status = AgentStatus::Failed;
            }
        }
        EngineEventKind::ModelsAvailable { models } => row.models.clone_from(models),
        EngineEventKind::CommandsAvailable { commands } => {
            row.commands = CommandCatalogue {
                known: true,
                commands: commands.clone(),
            };
        }
        EngineEventKind::CommandsInvalidated => row.commands = CommandCatalogue::default(),
        EngineEventKind::ConfigChanged { model, effort } => {
            if let Some(model) = model {
                row.state.model = Some(for_display(model));
            }
            if let Some(effort) = effort {
                row.effort = effort.as_deref().map(for_display);
            }
        }
        EngineEventKind::FastmodeChanged { state, .. } => {
            row.fast_mode = *state != FastModeState::Off;
        }
        EngineEventKind::AccessChanged { mode } => row.access = Some(for_display(mode)),
        EngineEventKind::ContextCompact {
            trigger,
            pre_tokens,
            post_tokens,
            ok,
        } => push_cell(
            row,
            TranscriptCell::Compact {
                trigger: format!("{trigger:?}").to_ascii_lowercase(),
                before_tokens: *pre_tokens,
                after_tokens: *post_tokens,
                ok: *ok,
            },
        ),
        EngineEventKind::PlanUpdated { entries } => push_cell(
            row,
            TranscriptCell::Plan {
                entries: entries
                    .iter()
                    .map(|entry| PlanEntry {
                        content: for_display(&entry.content),
                        priority: entry.priority,
                        status: entry.status,
                    })
                    .collect(),
            },
        ),
        EngineEventKind::SubagentStarted {
            agent_key,
            subagent_type,
            description,
            model,
        } => push_cell(
            row,
            TranscriptCell::Subagent {
                agent_key: agent_key.clone(),
                label: for_display(subagent_type),
                description: for_display(description),
                model: model.as_deref().map(for_display),
                result: None,
            },
        ),
        EngineEventKind::SubagentFinished {
            agent_key,
            ok,
            summary,
        } => {
            if let Some(TranscriptCell::Subagent { result, .. }) = row.transcript.rfind_mut(
                |cell| matches!(cell, TranscriptCell::Subagent { agent_key: candidate, .. } if candidate == agent_key),
            ) {
                *result = Some(ToolResultCell {
                    ok: *ok,
                    summary: for_display(summary),
                });
            }
        }
        // A visual reply (T3 section 8) is drawn only in the desktop's
        // sandboxed frame; the terminal names it.
        EngineEventKind::VisualShown { title, .. } => push_notice(
            row,
            &format!("Showed a visual: {title} (open the thread in Interface to see it)"),
            CellTone::Info,
        ),
        EngineEventKind::SessionClosed => {
            row.state.live = false;
            row.state.status = AgentStatus::NotLive;
        }
        EngineEventKind::PeerResult {
            source_engine,
            objective,
            text,
            ..
        } => push_cell(
            row,
            TranscriptCell::Peer {
                source_engine: source_engine.to_string(),
                objective: for_display(objective),
                markdown: for_display(text),
            },
        ),
    }
}

pub(super) fn scope_from_setting_sources(setting_sources: Option<&[String]>) -> LaunchScope {
    match setting_sources {
        Some([]) => LaunchScope::Isolated,
        Some(values) if values == ["project"] => LaunchScope::Project,
        None | Some(_) => LaunchScope::All,
    }
}

pub(super) fn context_telemetry_sample(
    key: &str,
    row: &mut ControlledSession,
    event: &EngineEvent,
    status_before: AgentStatus,
) -> Option<ContextTelemetrySample> {
    let phase = match event.event {
        EngineEventKind::ContextUsage { .. } => {
            row.telemetry_pending = true;
            if row.telemetry_startup_recorded {
                return None;
            }
            row.telemetry_startup_recorded = true;
            ContextTelemetryPhase::Startup
        }
        EngineEventKind::SessionClosed => ContextTelemetryPhase::Turn,
        EngineEventKind::StatusChanged { .. }
            if row.state.status != status_before
                && matches!(row.state.status, AgentStatus::Idle | AgentStatus::Failed) =>
        {
            ContextTelemetryPhase::Turn
        }
        _ => return None,
    };
    if !row.telemetry_pending {
        return None;
    }
    let usage = row.context.clone()?;
    let (used, size) = (usage.used?, usage.size?);
    row.telemetry_pending = false;
    if row.telemetry_recorded == Some((used, size)) {
        return None;
    }
    row.telemetry_recorded = Some((used, size));
    let scope = scope_from_setting_sources(row.setting_sources.as_deref()).as_str();
    Some(ContextTelemetrySample {
        session_key: key.to_owned(),
        engine: row.state.engine.clone(),
        model: row.state.model.clone(),
        scope: Some(scope.into()),
        used,
        size,
        percent: usage.percent,
        categories: usage.categories.as_ref().map(|categories| {
            categories
                .iter()
                .map(|category| ContextTelemetryCategory {
                    name: category.name.clone(),
                    tokens: category.tokens,
                })
                .collect()
        }),
        phase,
    })
}

const fn map_status(status: SessionStatus) -> AgentStatus {
    match status {
        SessionStatus::Working => AgentStatus::Working,
        SessionStatus::Waiting => AgentStatus::Blocked,
        SessionStatus::Idle => AgentStatus::Idle,
        SessionStatus::Failed => AgentStatus::Failed,
    }
}

pub(super) fn push_message(session: &mut ControlledSession, role: &'static str, text: &str) {
    push_cell(
        session,
        TranscriptCell::Message {
            role,
            markdown: for_display(text),
        },
    );
}

pub(super) fn push_notice(session: &mut ControlledSession, message: &str, tone: CellTone) {
    push_cell(
        session,
        TranscriptCell::Notice {
            message: for_display(message),
            tone,
        },
    );
}

/// Bytes of each side of a diff a transcript cell keeps. Only the details
/// overlay reads them; the transcript draws a diff as one line.
pub(super) const DIFF_SIDE_LIMIT_BYTES: usize = 64 * 1024;

/// `text` made safe to display, cut to its first `DIFF_SIDE_LIMIT_BYTES` on a
/// character boundary; a cut sets `truncated`.
fn diff_side(text: &str, truncated: &mut bool) -> String {
    if text.len() <= DIFF_SIDE_LIMIT_BYTES {
        return for_display(text);
    }
    *truncated = true;
    let mut end = DIFF_SIDE_LIMIT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    for_display(&text[..end])
}

pub(super) fn push_cell(session: &mut ControlledSession, cell: TranscriptCell) {
    session.transcript.push(cell);
}

pub(super) fn find_tool_mut<'a>(
    session: &'a mut ControlledSession,
    call_id: &str,
) -> Option<&'a mut TranscriptCell> {
    session.transcript.tool_mut(call_id)
}

fn ensure_tool(session: &mut ControlledSession, call_id: &str, tool: &str, summary: &str) {
    if find_tool_mut(session, call_id).is_none() {
        push_cell(
            session,
            TranscriptCell::Tool {
                call_id: call_id.to_owned(),
                tool: tool.into(),
                summary: summary.into(),
                input_preview: String::new(),
                result: None,
                command_activity: false,
            },
        );
    }
}

/// Characters of a tool input kept for its preview.
const TOOL_INPUT_PREVIEW_CHARACTERS: usize = 4_096;

pub(super) fn display_unknown(value: &serde_json::Value) -> String {
    // Enough bytes for the kept characters plus one more at four bytes each,
    // so a Write or Edit input carrying a whole file is never serialized whole.
    let mut capped = CappedWriter {
        bytes: Vec::new(),
        cap: (TOOL_INPUT_PREVIEW_CHARACTERS + 1) * 4,
        overflowed: false,
    };
    if serde_json::to_writer_pretty(&mut capped, value).is_err() && !capped.overflowed {
        return for_display("[unavailable]");
    }
    let source = match std::str::from_utf8(&capped.bytes) {
        Ok(source) => source,
        // The cap can split the last character; it lies past the kept ones.
        Err(error) => std::str::from_utf8(&capped.bytes[..error.valid_up_to()]).unwrap_or_default(),
    };
    let mut output = source
        .chars()
        .take(TOOL_INPUT_PREVIEW_CHARACTERS)
        .collect::<String>();
    if capped.overflowed
        || source
            .char_indices()
            .nth(TOOL_INPUT_PREVIEW_CHARACTERS)
            .is_some()
    {
        output.push('…');
    }
    for_display(&output)
}

/// Collects serializer output up to `cap` bytes, then refuses the rest.
struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
    overflowed: bool,
}

impl std::io::Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let room = self.cap.saturating_sub(self.bytes.len());
        if bytes.len() > room {
            self.bytes.extend_from_slice(&bytes[..room]);
            self.overflowed = true;
            return Err(std::io::Error::other("tool input preview is full"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn is_command_activity(tool: &str, input: &serde_json::Value) -> bool {
    let identity = tool
        .chars()
        .filter(|character| !matches!(character, ' ' | '_' | '-' | '/' | '\\'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if matches!(
        identity.as_str(),
        "bash" | "commandexecution" | "execcommand" | "runterminalcommand"
    ) {
        return true;
    }
    matches!(identity.as_str(), "execute" | "run" | "shell" | "terminal")
        && input
            .as_object()
            .and_then(|record| record.get("command"))
            .is_some_and(serde_json::Value::is_string)
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
