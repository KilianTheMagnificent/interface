//! Engine events into journal entries. There is no "message completed"
//! event: an agent's `message.delta` text is joined until that agent's next
//! other event, the turn's end or the session's close, as the renderer joins
//! it into one block.

use std::collections::BTreeMap;

use interface_contracts::{engine::EngineEventKind, thread_journal::JournalEntry};

/// The message text each agent (`None`: the main agent) has streamed and not
/// yet journalled.
#[derive(Debug, Default)]
pub struct JournalFold {
    open: BTreeMap<Option<String>, String>,
}

impl JournalFold {
    /// The entries `event` makes, in order: a joined message it ends first,
    /// then its own entry. A delta only joins.
    pub fn event(&mut self, event: &EngineEventKind) -> Vec<JournalEntry> {
        if let EngineEventKind::MessageDelta { text, agent_key } = event {
            self.open
                .entry(agent_key.clone())
                .or_default()
                .push_str(text);
            return Vec::new();
        }
        if matches!(event, EngineEventKind::SessionClosed) {
            let mut entries = self.flush_all();
            entries.push(JournalEntry::SessionClosed {});
            return entries;
        }
        let entry = entry_for_event(event);
        let mut entries = Vec::new();
        // An event for an agent ends that agent's message; thinking does too
        // (the renderer starts a new block after it). Events the journal
        // leaves out (status, usage, model, ...) are transparent.
        if entry.is_some() || matches!(event, EngineEventKind::ThinkingSignal { .. }) {
            entries.extend(self.flush(agent_of(event)));
        }
        entries.extend(entry);
        entries
    }

    /// Every agent's open message, the main agent's first (a turn's start or
    /// end, the session's close).
    pub fn flush_all(&mut self) -> Vec<JournalEntry> {
        std::mem::take(&mut self.open)
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(agent_key, text)| JournalEntry::Message {
                text,
                agent_key,
                cut: false,
            })
            .collect()
    }

    fn flush(&mut self, agent: Option<&str>) -> Option<JournalEntry> {
        let key = agent.map(str::to_owned);
        let text = self.open.remove(&key)?;
        (!text.is_empty()).then_some(JournalEntry::Message {
            text,
            agent_key: key,
            cut: false,
        })
    }

    /// Whether any agent has message text not yet journalled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.values().all(String::is_empty)
    }
}

/// The agent an event speaks for: its own `agentKey`, else the main agent.
/// A subagent's start and end are the main agent's (its Task tool).
fn agent_of(event: &EngineEventKind) -> Option<&str> {
    match event {
        EngineEventKind::MessageDelta { agent_key, .. }
        | EngineEventKind::UserMessage { agent_key, .. }
        | EngineEventKind::ThinkingSignal { agent_key, .. }
        | EngineEventKind::ToolCall { agent_key, .. }
        | EngineEventKind::ToolResult { agent_key, .. }
        | EngineEventKind::DiffProduced { agent_key, .. } => agent_key.as_deref(),
        _ => None,
    }
}

/// Whether [`entry_for_event`] makes an entry of `event`, without building it.
#[must_use]
pub const fn is_journalled(event: &EngineEventKind) -> bool {
    matches!(
        event,
        EngineEventKind::ToolCall { .. }
            | EngineEventKind::ToolResult { .. }
            | EngineEventKind::ApprovalRequested { .. }
            | EngineEventKind::DiffProduced { .. }
            | EngineEventKind::PlanUpdated { .. }
            | EngineEventKind::SubagentStarted { .. }
            | EngineEventKind::SubagentFinished { .. }
            | EngineEventKind::PeerResult { .. }
            | EngineEventKind::Notice { .. }
            | EngineEventKind::Error { .. }
            | EngineEventKind::ContextCompact { .. }
            | EngineEventKind::VisualShown { .. }
    )
}

/// The entry one engine event is, alone. Message deltas join (see
/// [`JournalFold`]); live `user.message` is not the journal's prompt (the
/// send hook is); status, usage, throughput, context, model, command,
/// config, access, fast mode, retry and thinking events are not journalled.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn entry_for_event(event: &EngineEventKind) -> Option<JournalEntry> {
    Some(match event.clone() {
        EngineEventKind::ToolCall {
            call_id,
            tool,
            summary,
            input,
            agent_key,
        } => JournalEntry::ToolCall {
            call_id,
            tool,
            summary,
            input,
            agent_key,
            cut: false,
        },
        EngineEventKind::ToolResult {
            call_id,
            ok,
            summary,
            agent_key,
        } => JournalEntry::ToolResult {
            call_id,
            ok,
            summary,
            agent_key,
        },
        EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input: _,
        } => JournalEntry::ApprovalRequested {
            approval_id,
            tool,
            summary,
        },
        EngineEventKind::DiffProduced {
            call_id,
            path,
            before,
            after,
            agent_key,
        } => JournalEntry::Diff {
            call_id,
            path,
            before: (!before.is_empty()).then_some(before),
            after: (!after.is_empty()).then_some(after),
            agent_key,
            cut: false,
        },
        EngineEventKind::PlanUpdated { entries } => JournalEntry::Plan { entries },
        EngineEventKind::SubagentStarted {
            agent_key,
            subagent_type,
            description,
            model,
        } => JournalEntry::SubagentStarted {
            agent_key,
            subagent_type,
            description,
            model,
        },
        EngineEventKind::SubagentFinished {
            agent_key,
            ok,
            summary,
        } => JournalEntry::SubagentFinished {
            agent_key,
            ok,
            summary,
        },
        EngineEventKind::PeerResult {
            operation_id,
            source_engine,
            source_session_id,
            objective,
            text,
            untrusted,
        } => JournalEntry::PeerResult {
            operation_id,
            source_engine,
            source_session_id,
            objective,
            text,
            untrusted,
        },
        EngineEventKind::Notice { message, tone } => JournalEntry::Notice { message, tone },
        EngineEventKind::Error {
            message,
            retryable,
            cause,
        } => JournalEntry::Error {
            message,
            retryable,
            cause,
        },
        EngineEventKind::ContextCompact {
            trigger,
            pre_tokens,
            post_tokens,
            ok,
        } => JournalEntry::Compact {
            trigger,
            pre_tokens,
            post_tokens,
            ok,
        },
        EngineEventKind::VisualShown { id, title, bytes } => {
            JournalEntry::Visual { id, title, bytes }
        }
        _ => return None,
    })
}
