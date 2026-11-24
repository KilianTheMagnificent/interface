//! The entry bound: a journal line is at most [`ENTRY_LIMIT`] bytes with its
//! newline. Past it, the longest text is cut at a character boundary until
//! the line fits, and entries that carry `cut` say so. A tool call's input
//! goes first (it becomes `null`), since its summary already says what ran.

use interface_contracts::thread_journal::{JournalEntry, JournalLine};
use serde_json::Value;

/// The most bytes one journal line takes, its newline included.
pub const ENTRY_LIMIT: usize = 64 * 1024;

/// `line` serialized with its newline, cut to fit [`ENTRY_LIMIT`].
///
/// # Panics
/// Never: a line whose texts are all empty is far below the limit.
#[must_use]
pub fn bounded_line(line: &mut JournalLine) -> Vec<u8> {
    bounded_line_within(line, ENTRY_LIMIT)
}

pub(super) fn bounded_line_within(line: &mut JournalLine, limit: usize) -> Vec<u8> {
    loop {
        let mut bytes = serde_json::to_vec(&line).expect("journal lines always encode");
        bytes.push(b'\n');
        if bytes.len() <= limit {
            return bytes;
        }
        let over = bytes.len() - limit;
        if !shrink(&mut line.entry, over) {
            // Nothing left to cut: a key or id alone is past the limit.
            // Answer what there is; the caller refuses it.
            return bytes;
        }
    }
}

/// Cuts `entry` by at least `over` serialized bytes when it can. Answers
/// whether it changed anything.
fn shrink(entry: &mut JournalEntry, over: usize) -> bool {
    if let JournalEntry::ToolCall { input, cut, .. } = entry
        && !input.is_null()
    {
        *input = Value::Null;
        *cut = true;
        return true;
    }
    let mut cut_flag = None;
    let mut texts: Vec<&mut String> = Vec::new();
    match entry {
        JournalEntry::TurnStarted { prompt, cut, .. } => {
            texts.push(prompt);
            cut_flag = Some(cut);
        }
        JournalEntry::Message { text, cut, .. } => {
            texts.push(text);
            cut_flag = Some(cut);
        }
        JournalEntry::ToolCall { summary, cut, .. } => {
            texts.push(summary);
            cut_flag = Some(cut);
        }
        JournalEntry::Diff {
            before, after, cut, ..
        } => {
            texts.extend(before.as_mut());
            texts.extend(after.as_mut());
            cut_flag = Some(cut);
        }
        JournalEntry::ToolResult { summary, .. }
        | JournalEntry::ApprovalRequested { summary, .. }
        | JournalEntry::SubagentFinished { summary, .. } => texts.push(summary),
        JournalEntry::SubagentStarted { description, .. } => texts.push(description),
        JournalEntry::PeerResult {
            objective, text, ..
        } => {
            texts.push(objective);
            texts.push(text);
        }
        JournalEntry::Notice { message, .. } | JournalEntry::Error { message, .. } => {
            texts.push(message);
        }
        JournalEntry::Plan { entries } => {
            texts.extend(entries.iter_mut().map(|entry| &mut entry.content));
        }
        JournalEntry::TurnEnded { reason, .. } => texts.extend(reason.as_mut()),
        JournalEntry::Visual { title, .. } => texts.push(title),
        JournalEntry::TurnRetracted {}
        | JournalEntry::Compact { .. }
        | JournalEntry::SessionClosed {} => {}
    }
    let Some(longest) = texts
        .into_iter()
        .filter(|text| !text.is_empty())
        .max_by_key(|text| text.len())
    else {
        return false;
    };
    // Escaping only makes serialized text longer than its bytes, so cutting
    // `over` bytes saves at least `over`.
    let keep = floor_char_boundary(longest, longest.len().saturating_sub(over));
    longest.truncate(keep);
    if let Some(cut) = cut_flag {
        *cut = true;
    }
    true
}

/// The largest character boundary at or below `index`.
pub(super) fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    (0..=index)
        .rev()
        .find(|at| text.is_char_boundary(*at))
        .unwrap_or(0)
}
