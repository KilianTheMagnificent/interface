//! A provider's stored history into journal entries, for a thread that had
//! no journal yet. Each `user.message` starts a turn with origin `import`;
//! each imported turn ends `completed` with reason `import` (the history
//! says nothing about how it ended).

use interface_contracts::{
    engine::{EngineEvent, EngineEventKind},
    thread_journal::{JournalEntry, TurnOrigin},
    threads::RunOutcome,
};

use super::fold::JournalFold;
use crate::threads::prompt_digest;

/// The `reason` an imported turn's end carries.
pub const IMPORT_REASON: &str = "import";

/// [`import_entries`] for a history page that `older` says left its oldest
/// part out: such a page can begin inside a turn, and what comes before its
/// first prompt (the rest of an answer whose prompt is not in the page) is
/// left out, so the journal starts at a prompt.
#[must_use]
pub fn import_entries_from(
    events: &[EngineEvent],
    fallback_at: u64,
    older: bool,
) -> Vec<(u64, JournalEntry)> {
    let mut entries = import_entries(events, fallback_at);
    if older {
        let first = entries
            .iter()
            .position(|(_, entry)| matches!(entry, JournalEntry::TurnStarted { .. }))
            .unwrap_or(entries.len());
        entries.drain(..first);
    }
    entries
}

/// The entries `events` make, each with its time (the event's own, else
/// `fallback_at`).
#[must_use]
pub fn import_entries(events: &[EngineEvent], fallback_at: u64) -> Vec<(u64, JournalEntry)> {
    let mut fold = JournalFold::default();
    let mut out = Vec::new();
    let mut open = false;
    // Where the open turn's prompt is: a turn with nothing after its prompt
    // was cut off before any answer (a crash, a stop), not completed.
    let mut prompt_at = 0;
    let mut last_at = fallback_at;
    let close = |out: &mut Vec<(u64, JournalEntry)>, at: u64, prompt_at: usize| {
        let answered = out.len() > prompt_at + 1;
        out.push((
            at,
            JournalEntry::TurnEnded {
                outcome: if answered {
                    RunOutcome::Completed
                } else {
                    RunOutcome::Interrupted
                },
                reason: Some(IMPORT_REASON.into()),
            },
        ));
    };
    for event in events {
        let at = event.at.as_deref().and_then(iso_ms).unwrap_or(last_at);
        last_at = at;
        if let EngineEventKind::UserMessage {
            text,
            agent_key: None,
            ..
        } = &event.event
        {
            out.extend(fold.flush_all().into_iter().map(|entry| (at, entry)));
            if open {
                close(&mut out, at, prompt_at);
            }
            prompt_at = out.len();
            out.push((
                at,
                JournalEntry::TurnStarted {
                    origin: TurnOrigin::Import,
                    prompt: text.clone(),
                    digest: prompt_digest(text),
                    cut: false,
                },
            ));
            open = true;
            continue;
        }
        if matches!(event.event, EngineEventKind::SessionClosed) {
            continue;
        }
        out.extend(
            fold.event(&event.event)
                .into_iter()
                .map(|entry| (at, entry)),
        );
    }
    out.extend(fold.flush_all().into_iter().map(|entry| (last_at, entry)));
    if open {
        close(&mut out, last_at, prompt_at);
    }
    out
}

fn iso_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
}
