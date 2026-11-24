//! The frozen human transcript renderer orchestration shows for an agent:
//! one line per visible provider event, with the Node byte-truncation rule.

use interface_contracts::engine::{EngineEvent, EngineEventKind, NoticeTone, SessionStatus};

/// One human-readable transcript line for an event, or `None` for events the
/// transcript does not show.
#[must_use]
pub fn orchestration_line(event: &EngineEvent) -> Option<String> {
    let clean = |value: &str| clean_orchestration_text(value);
    match &event.event {
        EngineEventKind::UserMessage { text, .. } => Some(format!("user: {}", clean(text))),
        EngineEventKind::MessageDelta { text, .. } => Some(format!("agent: {}", clean(text))),
        EngineEventKind::PeerResult {
            source_engine,
            text,
            ..
        } => Some(format!("untrusted peer ({source_engine}): {}", clean(text))),
        EngineEventKind::ToolCall { tool, summary, .. } => {
            Some(format!("tool {}: {}", clean(tool), clean(summary)))
        }
        EngineEventKind::ToolResult { ok, summary, .. } => Some(format!(
            "tool result ({}): {}",
            if *ok { "ok" } else { "failed" },
            clean(summary)
        )),
        EngineEventKind::ApprovalRequested { tool, summary, .. } => Some(format!(
            "approval requested for {}: {}",
            clean(tool),
            clean(summary)
        )),
        EngineEventKind::DiffProduced { path, .. } => Some(format!("diff: {}", clean(path))),
        EngineEventKind::StatusChanged { status } => Some(format!(
            "status: {}",
            match status {
                SessionStatus::Working => "working",
                SessionStatus::Waiting => "waiting",
                SessionStatus::Idle => "idle",
                SessionStatus::Failed => "failed",
            }
        )),
        EngineEventKind::Notice { message, tone } => Some(format!(
            "{}: {}",
            match tone {
                NoticeTone::Info => "info",
                NoticeTone::Warning => "warning",
            },
            clean(message)
        )),
        EngineEventKind::Error {
            message, retryable, ..
        } => Some(format!(
            "error{}: {}",
            if *retryable { " (retrying)" } else { "" },
            clean(message)
        )),
        EngineEventKind::SubagentStarted { description, .. } => {
            Some(format!("subagent started: {}", clean(description)))
        }
        EngineEventKind::SubagentFinished { ok, summary, .. } => Some(format!(
            "subagent {}: {}",
            if *ok { "finished" } else { "failed" },
            clean(summary)
        )),
        EngineEventKind::SessionClosed => Some("session closed".into()),
        _ => None,
    }
}

/// The newest `lines` transcript lines, oldest first.
#[must_use]
pub fn render_orchestration_transcript(events: &[EngineEvent], lines: usize) -> Vec<String> {
    let mut tail: Vec<String> = events
        .iter()
        .rev()
        .filter_map(orchestration_line)
        .take(lines)
        .collect();
    tail.reverse();
    tail
}

/// Drops control characters other than newline and tab, then truncates by
/// bytes: over 4096 bytes keeps the first 4080 and appends `…`, even when that
/// splits a character into U+FFFD. This is deliberate parity with Interface
/// 0.1.169's Node byte truncation.
#[must_use]
pub fn clean_orchestration_text(value: &str) -> String {
    let printable = value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>();
    if printable.len() <= 4_096 {
        return printable;
    }
    format!(
        "{}…",
        String::from_utf8_lossy(&printable.as_bytes()[..4_080])
    )
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
