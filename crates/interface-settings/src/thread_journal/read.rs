//! Reading the journal. Readers take no lock: segments only grow, and a
//! line still being written (or one that does not parse) is skipped, as is a
//! segment retention removes mid-read. Every reader takes the journal's
//! directory (`<data root>/thread-journal`).

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use interface_common::time::{IsoFraction, unix_millis_iso};
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::HistoryPage,
    thread_journal::{
        JournalEntry, JournalHeader, JournalHit, JournalLine, JournalPage, JournalRole,
        MAX_JOURNAL_PAGE_ENTRIES, TurnOrigin,
    },
    threads::RunOutcome,
};
use serde_json::Value;

use super::{
    PAGE_BYTE_LIMIT, SEARCH_HIT_LIMIT, SEARCH_HITS_PER_THREAD, SEGMENT_LIMIT,
    bounds::floor_char_boundary,
    store::{is_thread_hash, read_file, segment_ids, segment_path},
    thread_hash,
};

/// Characters of context an excerpt keeps before a match...
const EXCERPT_BEFORE: usize = 40;
/// ...and after it.
const EXCERPT_AFTER: usize = 80;

/// What a wait needs: the newest seq and whether a turn is open.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JournalWatch {
    pub last_seq: u64,
    pub turn_open: bool,
}

/// One thread's segments, oldest first.
fn segments(root: &Path, key: &str) -> Vec<(u64, PathBuf)> {
    let dir = root.join(thread_hash(key));
    segment_ids(&dir)
        .unwrap_or_default()
        .into_iter()
        .map(|id| (id, segment_path(&dir, id)))
        .collect()
}

/// The lines of one segment of `key`'s, skipping what does not parse. A
/// segment whose header names another key holds nothing of `key`'s.
fn segment_lines(path: &Path, key: Option<&str>) -> Option<(JournalHeader, Vec<JournalLine>)> {
    let bytes = read_file(path, SEGMENT_LIMIT * 2).ok()?;
    let mut chunks = bytes.split(|b| *b == b'\n');
    let header = serde_json::from_slice::<JournalHeader>(chunks.next()?).ok()?;
    if key.is_some_and(|key| key != header.key) {
        return None;
    }
    let lines = chunks
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| serde_json::from_slice::<JournalLine>(chunk).ok())
        .collect();
    Some((header, lines))
}

fn line_bytes(line: &JournalLine) -> usize {
    serde_json::to_vec(line).map_or(0, |bytes| bytes.len() + 1)
}

fn clamp_limit(limit: u32) -> usize {
    limit.clamp(1, MAX_JOURNAL_PAGE_ENTRIES) as usize
}

/// The kept seq range: the oldest segment's first seq and the newest line's.
fn bounds(root: &Path, key: &str) -> (u64, u64) {
    let all = segments(root, key);
    let floor = all.first().map_or(0, |(id, _)| *id);
    let last = all
        .iter()
        .rev()
        .find_map(|(_, path)| {
            let (header, lines) = segment_lines(path, Some(key))?;
            Some(
                lines
                    .last()
                    .map_or(header.first_seq.saturating_sub(1), |line| line.seq),
            )
        })
        .unwrap_or(0);
    (floor, last)
}

/// Entries after `after_seq` (from the oldest kept), oldest first: at most
/// `limit` (≤ 2000) and 1 MiB. `hasMore` when newer entries follow.
#[must_use]
pub fn read(root: &Path, key: &str, after_seq: Option<u64>, limit: u32) -> JournalPage {
    let limit = clamp_limit(limit);
    let after = after_seq.unwrap_or(0);
    let all = segments(root, key);
    let (floor, last) = bounds(root, key);
    let mut entries = Vec::new();
    let mut bytes = 0;
    let mut has_more = false;
    'segments: for (index, (_, path)) in all.iter().enumerate() {
        // Skip a segment when the next one starts at or before `after + 1`.
        if all
            .get(index + 1)
            .is_some_and(|(next, _)| *next <= after.saturating_add(1))
        {
            continue;
        }
        let Some((_, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        for line in lines.into_iter().filter(|line| line.seq > after) {
            let size = line_bytes(&line);
            if entries.len() >= limit || bytes + size > PAGE_BYTE_LIMIT {
                has_more = true;
                break 'segments;
            }
            bytes += size;
            entries.push(line);
        }
    }
    page(key, entries, last, floor, has_more)
}

/// The newest `limit` (≤ 2000) entries, oldest first, within 1 MiB.
/// `hasMore` when older kept entries come before them.
#[must_use]
pub fn tail(root: &Path, key: &str, limit: u32) -> JournalPage {
    let limit = clamp_limit(limit);
    let (floor, last) = bounds(root, key);
    let (entries, has_more) = tail_lines(root, key, limit, PAGE_BYTE_LIMIT);
    page(key, entries, last, floor, has_more)
}

/// Every kept line of `key`'s journal, oldest first.
#[must_use]
pub fn lines(root: &Path, key: &str) -> Vec<JournalLine> {
    tail_lines(root, key, usize::MAX, usize::MAX).0
}

/// The newest `limit` lines within `byte_limit`, oldest first, and whether
/// older ones exist.
fn tail_lines(root: &Path, key: &str, limit: usize, byte_limit: usize) -> (Vec<JournalLine>, bool) {
    let mut newest_first = Vec::new();
    let mut bytes = 0;
    let mut has_more = false;
    'segments: for (_, path) in segments(root, key).iter().rev() {
        let Some((_, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        for line in lines.into_iter().rev() {
            let size = line_bytes(&line);
            if newest_first.len() >= limit || bytes + size > byte_limit {
                has_more = true;
                break 'segments;
            }
            bytes += size;
            newest_first.push(line);
        }
    }
    newest_first.reverse();
    (newest_first, has_more)
}

fn page(
    key: &str,
    entries: Vec<JournalLine>,
    last: u64,
    floor: u64,
    has_more: bool,
) -> JournalPage {
    JournalPage {
        key: key.to_owned(),
        first_seq: entries.first().map(|line| line.seq),
        entries,
        last_seq: last,
        floor_seq: floor,
        has_more,
    }
}

/// The turns `lines` retract.
fn retracted(lines: &[JournalLine]) -> HashSet<u32> {
    lines
        .iter()
        .filter(|line| matches!(line.entry, JournalEntry::TurnRetracted {}))
        .filter_map(|line| line.turn)
        .collect()
}

/// `lines` without retracted turns, with a launch prompt that was written
/// after the session's first entries moved before them: a turn's prompt
/// always comes before its entries.
fn conversation(lines: Vec<JournalLine>) -> Vec<JournalLine> {
    let retracted = retracted(&lines);
    let mut kept = lines
        .into_iter()
        .filter(|line| line.turn.is_none_or(|turn| !retracted.contains(&turn)))
        .collect::<Vec<_>>();
    if let Some(launch) = kept.iter().position(|line| {
        matches!(
            line.entry,
            JournalEntry::TurnStarted {
                origin: TurnOrigin::Launch,
                ..
            }
        ) && line.turn == Some(0)
    }) && let Some(first) = kept[..launch].iter().position(|line| line.turn.is_none())
    {
        let prompt = kept.remove(launch);
        kept.insert(first, prompt);
    }
    kept
}

fn iso(at: u64) -> String {
    unix_millis_iso(i64::try_from(at).unwrap_or(i64::MAX), IsoFraction::Millis)
}

/// The engine event an entry replays as, if any.
#[allow(clippy::too_many_lines)]
fn replayed(line: &JournalLine) -> Option<EngineEvent> {
    let event = match line.entry.clone() {
        JournalEntry::TurnStarted { prompt, .. } => EngineEventKind::UserMessage {
            text: prompt,
            agent_key: None,
            images: None,
            image_references: None,
        },
        JournalEntry::Message {
            text, agent_key, ..
        } => EngineEventKind::MessageDelta { text, agent_key },
        JournalEntry::ToolCall {
            call_id,
            tool,
            summary,
            input,
            agent_key,
            ..
        } => EngineEventKind::ToolCall {
            call_id,
            tool,
            summary,
            input,
            agent_key,
        },
        JournalEntry::ToolResult {
            call_id,
            ok,
            summary,
            agent_key,
        } => EngineEventKind::ToolResult {
            call_id,
            ok,
            summary,
            agent_key,
        },
        JournalEntry::ApprovalRequested {
            approval_id,
            tool,
            summary,
        } => EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input: Value::Object(serde_json::Map::new()),
        },
        JournalEntry::Diff {
            call_id,
            path,
            before,
            after,
            agent_key,
            ..
        } => EngineEventKind::DiffProduced {
            call_id,
            path,
            before: before.unwrap_or_default(),
            after: after.unwrap_or_default(),
            agent_key,
        },
        JournalEntry::Plan { entries } => EngineEventKind::PlanUpdated { entries },
        JournalEntry::SubagentStarted {
            agent_key,
            subagent_type,
            description,
            model,
        } => EngineEventKind::SubagentStarted {
            agent_key,
            subagent_type,
            description,
            model,
        },
        JournalEntry::SubagentFinished {
            agent_key,
            ok,
            summary,
        } => EngineEventKind::SubagentFinished {
            agent_key,
            ok,
            summary,
        },
        JournalEntry::PeerResult {
            operation_id,
            source_engine,
            source_session_id,
            objective,
            text,
            untrusted,
        } => EngineEventKind::PeerResult {
            operation_id,
            source_engine,
            source_session_id,
            objective,
            text,
            untrusted,
        },
        JournalEntry::Notice { message, tone } => EngineEventKind::Notice { message, tone },
        JournalEntry::Visual { id, title, bytes } => {
            EngineEventKind::VisualShown { id, title, bytes }
        }
        JournalEntry::Error {
            message,
            retryable,
            cause,
        } => EngineEventKind::Error {
            message,
            retryable,
            cause,
        },
        JournalEntry::Compact {
            trigger,
            pre_tokens,
            post_tokens,
            ok,
        } => EngineEventKind::ContextCompact {
            trigger,
            pre_tokens,
            post_tokens,
            ok,
        },
        JournalEntry::TurnEnded { outcome, .. } => EngineEventKind::StatusChanged {
            status: match outcome {
                RunOutcome::Failed => SessionStatus::Failed,
                RunOutcome::Completed | RunOutcome::Interrupted => SessionStatus::Idle,
            },
        },
        JournalEntry::TurnRetracted {} | JournalEntry::SessionClosed {} => return None,
    };
    Some(EngineEvent {
        at: Some(iso(line.at)),
        event,
    })
}

/// `key`'s newest `limit` entries as the engine events a provider's history
/// would answer: each prompt a `user.message`, each message one
/// `message.delta`, a turn's end a `status.changed`. Retracted turns are
/// left out; a turn's prompt always comes before its entries.
#[must_use]
pub fn replay(root: &Path, key: &str, limit: usize) -> Vec<EngineEvent> {
    let (lines, _) = tail_lines(root, key, limit.max(1), usize::MAX);
    conversation(lines).iter().filter_map(replayed).collect()
}

/// [`replay`] as a history page, or `None` when the journal holds nothing
/// to show.
#[must_use]
pub fn replay_history(root: &Path, key: &str, limit: usize) -> Option<HistoryPage> {
    let (lines, older) = tail_lines(root, key, limit.max(1), usize::MAX);
    let events = conversation(lines)
        .iter()
        .filter_map(replayed)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return None;
    }
    let messages = events
        .iter()
        .filter(|event| {
            matches!(
                event.event,
                EngineEventKind::UserMessage { .. } | EngineEventKind::MessageDelta { .. }
            )
        })
        .count() as u64;
    Some(HistoryPage {
        availability: None,
        reason: None,
        events,
        messages: Some(messages),
        // Older entries exist past the page; how many messages they hold
        // is not counted.
        older_messages: u64::from(older),
    })
}

/// The visuals `key`'s journal holds from `since` (ms) on, oldest first, as
/// `visual.shown` events (T3 section 8). Reads segments newest first and
/// stops at the first that began before `since`.
#[must_use]
pub fn visual_events(root: &Path, key: &str, since: Option<u64>) -> Vec<EngineEvent> {
    let since = since.unwrap_or(0);
    let mut newest_first = Vec::new();
    for (_, path) in segments(root, key).iter().rev() {
        let Some((_, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        let began = lines.first().map_or(0, |line| line.at);
        for line in lines.iter().rev() {
            if line.at >= since && matches!(line.entry, JournalEntry::Visual { .. }) {
                newest_first.extend(replayed(line));
            }
        }
        if began < since {
            break;
        }
    }
    newest_first.reverse();
    newest_first
}

/// An event's `at` in milliseconds since the epoch, when it has one.
fn event_ms(event: &EngineEvent) -> Option<u64> {
    let at = event.at.as_deref()?;
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
}

/// Places `visuals` (oldest first, each with its `at`) into a provider's
/// history page (T3 section 8): providers keep no record of a page Interface
/// showed. Each goes, in this order of preference:
///
/// 1. right after the `tool.result` of the `show_visual` call that made it
///    (its summary carries the visual's id, as the tool answered it);
/// 2. by time, after the last event at or before it, when the page's events
///    carry times;
/// 3. at the page's end, when the page is the whole conversation.
///
/// A visual from before a page's first event, or one that cannot be placed
/// in a page that has older events past it, belongs to the part not shown
/// and is left out. A visual already in the page is not added again.
pub fn merge_visuals(page: &mut HistoryPage, visuals: Vec<EngineEvent>) {
    let shown = page
        .events
        .iter()
        .filter_map(|event| match &event.event {
            EngineEventKind::VisualShown { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let earliest = page.events.iter().find_map(event_ms);
    let whole = page.older_messages == 0;
    for visual in visuals {
        let EngineEventKind::VisualShown { id, .. } = &visual.event else {
            continue;
        };
        if shown.contains(id) {
            continue;
        }
        let answered = page.events.iter().position(|event| {
            matches!(&event.event, EngineEventKind::ToolResult { summary, .. } if summary.contains(id.as_str()))
        });
        let position = match (answered, event_ms(&visual), earliest) {
            (Some(index), _, _) => index + 1,
            (None, Some(at), Some(earliest)) => {
                if at < earliest && !whole {
                    continue;
                }
                page.events
                    .iter()
                    .rposition(|event| event_ms(event).is_some_and(|time| time <= at))
                    .map_or(0, |index| index + 1)
            }
            _ if whole => page.events.len(),
            _ => continue,
        };
        page.events.insert(position, visual);
    }
}

/// One segment of a thread whose provider was switched (T3 section 5), as
/// [`replay_chain`] reads it.
#[derive(Clone, Copy, Debug)]
pub struct ChainSegment<'a> {
    pub key: &'a str,
    /// The provider as the owner reads it ("Codex"), for the "Switched to"
    /// notice before its stretch.
    pub label: &'a str,
}

/// The journals of a switched thread's segments as one conversation: each
/// segment's [`replay`] (its newest `limit` entries), merged by time (a tie
/// keeps the earlier segment first), with a `notice` "Switched to <label>"
/// wherever the segment changes, newest `limit` events kept. `since` keeps
/// only events after that moment (ms), for what a provider missed.
#[must_use]
pub fn replay_chain(
    root: &Path,
    segments: &[ChainSegment<'_>],
    limit: usize,
    since: Option<u64>,
) -> Vec<EngineEvent> {
    replay_chain_page(root, segments, limit, since).0
}

/// [`replay_chain`] and whether older lines exist past it.
fn replay_chain_page(
    root: &Path,
    segments: &[ChainSegment<'_>],
    limit: usize,
    since: Option<u64>,
) -> (Vec<EngineEvent>, bool) {
    let limit = limit.max(1);
    let since = since.map(iso);
    let mut older = false;
    let mut tagged = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let (lines, more) = tail_lines(root, segment.key, limit, usize::MAX);
        older |= more;
        tagged.extend(
            conversation(lines)
                .iter()
                .filter_map(replayed)
                .filter(|event| {
                    since
                        .as_deref()
                        .is_none_or(|since| event.at.as_deref().is_some_and(|at| at > since))
                })
                .map(|event| (index, event)),
        );
    }
    // Stable: equal times keep segment order, then each journal's own order.
    tagged.sort_by(|left, right| left.1.at.cmp(&right.1.at));
    let mut events = Vec::with_capacity(tagged.len() + segments.len());
    let mut current = None;
    for (index, event) in tagged {
        if current.is_some_and(|current| current != index) {
            events.push(EngineEvent {
                at: event.at.clone(),
                event: EngineEventKind::Notice {
                    message: format!("Switched to {}", segments[index].label),
                    tone: interface_contracts::engine::NoticeTone::Info,
                },
            });
        }
        current = Some(index);
        events.push(event);
    }
    if events.len() > limit {
        older = true;
        events.drain(..events.len() - limit);
    }
    (events, older)
}

/// [`replay_chain`] as a history page (the whole thread, newest `limit`
/// events), or `None` when no segment's journal holds anything to show.
#[must_use]
pub fn replay_chain_history(
    root: &Path,
    segments: &[ChainSegment<'_>],
    limit: usize,
) -> Option<HistoryPage> {
    let (events, older) = replay_chain_page(root, segments, limit, None);
    if events.is_empty() {
        return None;
    }
    let messages = events
        .iter()
        .filter(|event| {
            matches!(
                event.event,
                EngineEventKind::UserMessage { .. } | EngineEventKind::MessageDelta { .. }
            )
        })
        .count() as u64;
    Some(HistoryPage {
        availability: None,
        reason: None,
        events,
        messages: Some(messages),
        older_messages: u64::from(older),
    })
}

/// The main agent's last message in the newest ended turn.
#[must_use]
pub fn last_answer(root: &Path, key: &str) -> Option<String> {
    let mut ended: Option<u32> = None;
    let mut retracted = HashSet::new();
    for (_, path) in segments(root, key).iter().rev() {
        let Some((_, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        for line in lines.into_iter().rev() {
            match (&line.entry, line.turn) {
                (JournalEntry::TurnRetracted {}, Some(turn)) => {
                    retracted.insert(turn);
                }
                (JournalEntry::TurnEnded { .. }, Some(turn))
                    if ended.is_none() && !retracted.contains(&turn) =>
                {
                    ended = Some(turn);
                }
                (
                    JournalEntry::Message {
                        text,
                        agent_key: None,
                        ..
                    },
                    turn,
                ) if ended.is_some() && turn == ended => return Some(text.clone()),
                (_, turn) if ended.is_some_and(|ended| turn.is_none_or(|turn| turn < ended)) => {
                    return None;
                }
                _ => {}
            }
        }
    }
    None
}

/// The newest ended turn of a thread, for team tools' waits and notices
/// (T3 section 2): its number, how it ended, the main agent's last message in
/// it and the last error it recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndedTurn {
    pub turn: u32,
    pub outcome: RunOutcome,
    pub answer: Option<String>,
    pub error: Option<String>,
}

/// `key`'s newest ended turn that was not taken back, or `None`.
#[must_use]
pub fn last_ended_turn(root: &Path, key: &str) -> Option<EndedTurn> {
    let mut found: Option<EndedTurn> = None;
    let mut retracted = HashSet::new();
    for (_, path) in segments(root, key).iter().rev() {
        let Some((_, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        for line in lines.into_iter().rev() {
            let Some(turn) = line.turn else {
                if found.is_some() {
                    return found;
                }
                continue;
            };
            if let Some(ended) = &found
                && turn < ended.turn
            {
                return found;
            }
            match line.entry {
                JournalEntry::TurnRetracted {} => {
                    retracted.insert(turn);
                }
                JournalEntry::TurnEnded { outcome, .. }
                    if found.is_none() && !retracted.contains(&turn) =>
                {
                    found = Some(EndedTurn {
                        turn,
                        outcome,
                        answer: None,
                        error: None,
                    });
                }
                JournalEntry::Message {
                    text,
                    agent_key: None,
                    ..
                } => {
                    if let Some(ended) = found.as_mut().filter(|ended| ended.turn == turn)
                        && ended.answer.is_none()
                    {
                        ended.answer = Some(text);
                    }
                }
                JournalEntry::Error { message, .. } => {
                    if let Some(ended) = found.as_mut().filter(|ended| ended.turn == turn)
                        && ended.error.is_none()
                    {
                        ended.error = Some(message);
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// `key`'s newest seq and whether a turn is open in it.
#[must_use]
pub fn watch(root: &Path, key: &str) -> JournalWatch {
    let mut watch = JournalWatch::default();
    for (_, path) in segments(root, key).iter().rev() {
        let Some((header, lines)) = segment_lines(path, Some(key)) else {
            continue;
        };
        if watch.last_seq == 0 {
            watch.last_seq = lines
                .last()
                .map_or(header.first_seq.saturating_sub(1), |line| line.seq);
        }
        for line in lines.iter().rev() {
            match line.entry {
                JournalEntry::TurnStarted { .. } => {
                    watch.turn_open = true;
                    return watch;
                }
                JournalEntry::TurnEnded { .. } | JournalEntry::TurnRetracted {} => return watch,
                _ => {}
            }
        }
    }
    watch
}

/// Prompts and messages matching `query` (case-insensitive, literal), in
/// threads with the newest activity first and newest entries first within
/// a thread: at most 100 hits, 5 a thread. Stops at `deadline` with what it
/// found.
#[must_use]
pub fn search(root: &Path, query: &str, deadline: Instant) -> Vec<JournalHit> {
    search_with(root, query, deadline, &|| false).hits
}

/// What [`search_with`] found, and whether it looked everywhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<JournalHit>,
    /// It stopped at its deadline or was cancelled before reading every
    /// thread it might have found more in.
    pub cut_short: bool,
}

/// [`search`] that also stops, between segments, once `cancelled` answers
/// true (a newer search replaced it), and says whether it was cut short.
#[must_use]
pub fn search_with(
    root: &Path,
    query: &str,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> SearchOutcome {
    let needle = query.trim().to_lowercase();
    let mut outcome = SearchOutcome::default();
    if needle.is_empty() {
        return outcome;
    }
    let prefilter = RawPrefilter::new(&needle);
    let mut threads = fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_str().is_some_and(is_thread_hash))
                .map(|entry| {
                    let dir = entry.path();
                    let newest = segment_ids(&dir)
                        .ok()
                        .and_then(|ids| ids.last().copied())
                        .map(|id| segment_path(&dir, id));
                    let modified = newest
                        .as_ref()
                        .and_then(|path| fs::metadata(path).ok())
                        .and_then(|metadata| metadata.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (modified, dir)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    threads.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let stop = || Instant::now() >= deadline || cancelled();
    for (_, dir) in threads {
        if outcome.hits.len() >= SEARCH_HIT_LIMIT {
            break;
        }
        if stop() || !search_thread(&dir, &needle, prefilter.as_ref(), &mut outcome.hits, &stop) {
            outcome.cut_short = true;
            break;
        }
    }
    outcome
}

/// Searches one thread's segments, newest first. Answers false when `stop`
/// cut it short.
fn search_thread(
    dir: &Path,
    needle: &str,
    prefilter: Option<&RawPrefilter>,
    hits: &mut Vec<JournalHit>,
    stop: &dyn Fn() -> bool,
) -> bool {
    let mut found = 0;
    let mut retracted = HashSet::new();
    let ids = segment_ids(dir).unwrap_or_default();
    let mut folded = Vec::new();
    for id in ids.into_iter().rev() {
        if stop() {
            return false;
        }
        let Ok(bytes) = read_file(&segment_path(dir, id), SEGMENT_LIMIT * 2) else {
            continue;
        };
        let mut chunks = bytes.split(|b| *b == b'\n');
        let Some(header) = chunks
            .next()
            .and_then(|chunk| serde_json::from_slice::<JournalHeader>(chunk).ok())
        else {
            continue;
        };
        if dir.file_name().and_then(|name| name.to_str()) != Some(&thread_hash(&header.key)) {
            continue;
        }
        let raw = chunks.filter(|chunk| !chunk.is_empty()).collect::<Vec<_>>();
        for chunk in raw.into_iter().rev() {
            // Only prompts, answers and take-backs matter; a line that cannot
            // match is not parsed (tool inputs and diffs are most of the bytes).
            match raw_kind(chunk) {
                RawKind::Other => continue,
                RawKind::Text
                    if prefilter.is_some_and(|filter| !filter.may_match(chunk, &mut folded)) =>
                {
                    continue;
                }
                RawKind::Text | RawKind::Retracted | RawKind::Unknown => {}
            }
            let Ok(line) = serde_json::from_slice::<JournalLine>(chunk) else {
                continue;
            };
            let (text, role) = match &line.entry {
                JournalEntry::TurnRetracted {} => {
                    retracted.extend(line.turn);
                    continue;
                }
                JournalEntry::TurnStarted { prompt, .. } => (prompt, JournalRole::Prompt),
                JournalEntry::Message { text, .. } => (text, JournalRole::Message),
                _ => continue,
            };
            if line.turn.is_some_and(|turn| retracted.contains(&turn)) {
                continue;
            }
            let Some(excerpt) = excerpt(text, needle) else {
                continue;
            };
            hits.push(JournalHit {
                key: header.key.clone(),
                seq: line.seq,
                turn: line.turn,
                role,
                excerpt,
                at: line.at,
            });
            found += 1;
            if found >= SEARCH_HITS_PER_THREAD || hits.len() >= SEARCH_HIT_LIMIT {
                return true;
            }
        }
    }
    true
}

/// A journal line's entry kind, read from its raw bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RawKind {
    /// A prompt or an answer: the lines search reads.
    Text,
    /// A take-back: its turn's lines are left out.
    Retracted,
    /// Anything else (tool calls, diffs, ends...): never a hit.
    Other,
    /// Not in the shape the writer makes: parsed, as before.
    Unknown,
}

const ENTRY_KIND: &[u8] = b"\"entry\":{\"kind\":\"";

/// The writer serializes `{"seq":N,"at":N[,"turn":N],"entry":{"kind":"…"`:
/// only those keys and numbers come before the entry, so when that prefix
/// holds nothing else the first `"entry":{"kind":"` is the entry's own.
fn raw_kind(line: &[u8]) -> RawKind {
    let Some(at) = memchr::memmem::find(line, ENTRY_KIND) else {
        return RawKind::Unknown;
    };
    let prefix = &line[..at];
    let plain = prefix.first() == Some(&b'{')
        && prefix[1..]
            .iter()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'"' | b':' | b','));
    if !plain {
        return RawKind::Unknown;
    }
    let kind = &line[at + ENTRY_KIND.len()..];
    if kind.starts_with(b"turnStarted\"") || kind.starts_with(b"message\"") {
        RawKind::Text
    } else if kind.starts_with(b"turnRetracted\"") {
        RawKind::Retracted
    } else {
        RawKind::Other
    }
}

/// A raw-byte test a prompt or answer line must pass to match: the needle,
/// ASCII case folded, somewhere in the line. Exact only for an ASCII needle
/// with nothing JSON escapes (quote, backslash, control characters): then a
/// match in the text is the same bytes in the line, except where the text
/// holds one of the two characters outside ASCII that lower-case into it
/// ([`ASCII_FOLDING`]); a line holding either is always parsed.
struct RawPrefilter {
    needle: Vec<u8>,
}

/// U+0130 (İ, lower-cases to "i̇") and U+212A (Kelvin sign, to "k") in
/// UTF-8: the only characters outside ASCII whose lower case holds ASCII.
const ASCII_FOLDING: [&[u8]; 2] = [&[0xC4, 0xB0], &[0xE2, 0x84, 0xAA]];

impl RawPrefilter {
    fn new(needle: &str) -> Option<Self> {
        needle
            .bytes()
            .all(|b| b.is_ascii() && b >= 0x20 && b != b'"' && b != b'\\')
            .then(|| Self {
                needle: needle.as_bytes().to_vec(),
            })
    }

    fn may_match(&self, line: &[u8], folded: &mut Vec<u8>) -> bool {
        folded.clear();
        folded.extend(line.iter().map(u8::to_ascii_lowercase));
        memchr::memmem::find(folded, &self.needle).is_some()
            || ASCII_FOLDING
                .iter()
                .any(|bytes| memchr::memmem::find(line, bytes).is_some())
    }
}

/// The first case-insensitive match of `needle` (already lower case) in
/// `text`, with a little text around it on one line, or `None`.
fn excerpt(text: &str, needle: &str) -> Option<String> {
    // Lower-case `text` keeping where each lower-cased character came from.
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (at, character) in text.char_indices() {
        for folded in character.to_lowercase() {
            let start = lower.len();
            lower.push(folded);
            origin.extend(std::iter::repeat_n(at, lower.len() - start));
        }
    }
    let found = lower.find(needle)?;
    let start = origin[found];
    let end = origin
        .get(found + needle.len())
        .copied()
        .unwrap_or(text.len());
    let from = text[..start]
        .char_indices()
        .rev()
        .nth(EXCERPT_BEFORE.saturating_sub(1))
        .map_or(0, |(at, _)| at);
    let to = text[end..]
        .char_indices()
        .nth(EXCERPT_AFTER)
        .map_or(text.len(), |(at, _)| end + at);
    let to = floor_char_boundary(text, to);
    let mut excerpt = String::new();
    if from > 0 {
        excerpt.push('…');
    }
    excerpt.push_str(
        &text[from..to]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
    if to < text.len() {
        excerpt.push('…');
    }
    Some(excerpt)
}

#[cfg(test)]
pub(super) fn excerpt_for_tests(text: &str, needle: &str) -> Option<String> {
    excerpt(text, needle)
}
