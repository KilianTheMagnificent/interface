//! The journal worker's rules, driven op by op with a fixed clock.

use std::path::Path;

use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    thread_journal::{JournalEntry, JournalLine, TurnOrigin},
    threads::RunOutcome,
};
use interface_settings::thread_journal::{self, JOURNAL_DIR, ThreadJournal};

use super::*;

const KEY: &str = "claude#11111111-1111-4111-8111-111111111111";
const AGENT: &str = "codex#agent-session";
const T: u64 = 1_791_300_000_000;

fn private_temp() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

fn core(data: &Path) -> JournalCore {
    JournalCore::new(ThreadJournal::open(data).unwrap())
}

fn event(key: &str, at: u64, event: EngineEventKind, run: Option<RunChange>) -> JournalOp {
    JournalOp::Event {
        key: key.into(),
        at,
        event,
        run,
    }
}

fn delta(text: &str) -> EngineEventKind {
    EngineEventKind::MessageDelta {
        text: text.into(),
        agent_key: None,
    }
}

fn idle() -> EngineEventKind {
    EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    }
}

fn prompt(key: &str, at: u64, origin: TurnOrigin, text: &str) -> JournalOp {
    JournalOp::Prompt {
        key: key.into(),
        at,
        origin,
        prompt: text.into(),
    }
}

fn lines(data: &Path, key: &str) -> Vec<JournalLine> {
    thread_journal::read(&data.join(JOURNAL_DIR), key, None, 2_000).entries
}

fn shown(lines: &[JournalLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match &line.entry {
            JournalEntry::TurnStarted { origin, prompt, .. } => {
                format!("{origin:?}>{prompt}").to_lowercase()
            }
            JournalEntry::Message { text, .. } => format!("message:{text}"),
            JournalEntry::TurnEnded { outcome, .. } => format!("end:{outcome:?}").to_lowercase(),
            other => serde_json::to_value(other).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned(),
        })
        .collect()
}

#[test]
fn a_launch_prompt_bound_after_its_answer_is_written_before_it() {
    let temp = private_temp();
    let mut core = core(temp.path());
    // The launch run answers and ends before `Bind` reaches the journal.
    for op in [
        event(KEY, T, delta("Hello "), Some(RunChange::Started)),
        event(KEY, T + 1, delta("there"), None),
        event(
            KEY,
            T + 2,
            idle(),
            Some(RunChange::Ended(RunOutcome::Completed)),
        ),
    ] {
        assert!(core.apply(op, T).is_empty());
    }
    assert!(lines(temp.path(), KEY).is_empty(), "held until the prompt");
    core.apply(prompt(KEY, T + 3, TurnOrigin::Launch, "Say hello"), T + 3);
    let written = lines(temp.path(), KEY);
    assert_eq!(
        shown(&written),
        ["launch>say hello", "message:Hello there", "end:completed"]
    );
    assert!(written.iter().all(|line| line.turn == Some(0)));
    // The prompt takes the time of what it started: `at` never runs backwards.
    assert!(written.windows(2).all(|pair| pair[0].at <= pair[1].at));
    assert_eq!(written[0].at, T + 2);
    // A later bind keeps the turns it has.
    core.apply(prompt(KEY, T + 4, TurnOrigin::Launch, "Say hello"), T + 4);
    assert_eq!(lines(temp.path(), KEY).len(), 3);
    assert!(core.next_deadline().is_some(), "an fsync is due");
}

#[test]
fn an_owner_turn_is_written_as_it_happens_and_its_end_syncs() {
    let temp = private_temp();
    let mut core = core(temp.path());
    core.apply(prompt(KEY, T, TurnOrigin::Owner, "Fix it"), T);
    for op in [
        event(KEY, T + 1, delta("Looking"), None),
        event(
            KEY,
            T + 2,
            EngineEventKind::ToolCall {
                call_id: "c1".into(),
                tool: "Bash".into(),
                summary: "ls".into(),
                input: serde_json::json!({}),
                agent_key: None,
            },
            None,
        ),
        event(KEY, T + 3, delta("Done"), None),
        event(
            KEY,
            T + 4,
            idle(),
            Some(RunChange::Ended(RunOutcome::Completed)),
        ),
        // A run the provider starts on its own after the end adds no second end.
        event(KEY, T + 5, delta("Background"), Some(RunChange::Started)),
        event(
            KEY,
            T + 6,
            idle(),
            Some(RunChange::Ended(RunOutcome::Completed)),
        ),
    ] {
        core.apply(op, T);
    }
    assert_eq!(
        shown(&lines(temp.path(), KEY)),
        [
            "owner>fix it",
            "message:Looking",
            "toolCall",
            "message:Done",
            "end:completed",
            "message:Background",
        ]
    );
}

#[test]
fn a_send_taken_back_is_retracted_and_a_steer_starts_a_steer_turn() {
    let temp = private_temp();
    let mut core = core(temp.path());
    core.apply(prompt(KEY, T, TurnOrigin::Owner, "never delivered"), T);
    core.apply(
        JournalOp::Retract {
            key: KEY.into(),
            at: T + 1,
        },
        T + 1,
    );
    core.apply(prompt(KEY, T + 2, TurnOrigin::Owner, "delivered"), T + 2);
    core.apply(event(KEY, T + 3, delta("part one"), None), T + 3);
    core.apply(prompt(KEY, T + 4, TurnOrigin::Steer, "also this"), T + 4);
    core.apply(
        event(
            KEY,
            T + 5,
            idle(),
            Some(RunChange::Ended(RunOutcome::Completed)),
        ),
        T + 5,
    );
    // A retract with no open turn writes nothing.
    core.apply(
        JournalOp::Retract {
            key: KEY.into(),
            at: T + 6,
        },
        T + 6,
    );
    let written = lines(temp.path(), KEY);
    assert_eq!(
        shown(&written),
        [
            "owner>never delivered",
            "turnRetracted",
            "owner>delivered",
            "message:part one",
            "steer>also this",
            "end:completed",
        ]
    );
    assert_eq!(written[1].turn, Some(0));
    assert_eq!(written[4].turn, Some(2));
    // Readers drop the retracted turn.
    let replayed = thread_journal::replay(&temp.path().join(JOURNAL_DIR), KEY, 100);
    assert!(
        matches!(&replayed[0].event, EngineEventKind::UserMessage { text, .. } if text == "delivered")
    );
}

#[test]
fn a_session_with_no_record_is_held_then_dropped_and_one_with_a_record_is_kept() {
    let temp = private_temp();
    let mut core = core(temp.path());
    core.apply(
        event(AGENT, T, delta("agent text"), Some(RunChange::Started)),
        T,
    );
    core.apply(
        event(
            KEY,
            T,
            delta("seeded thread text"),
            Some(RunChange::Started),
        ),
        T,
    );
    // Streaming text is still being joined: nothing is held yet.
    assert_eq!(core.next_deadline(), None);
    // A run's end flushes the text into the hold.
    for key in [AGENT, KEY] {
        core.apply(
            event(
                key,
                T + 1,
                idle(),
                Some(RunChange::Ended(RunOutcome::Completed)),
            ),
            T + 1,
        );
    }
    assert_eq!(core.next_deadline(), Some(T + 1 + HOLD_MS));
    assert!(core.tick(T + HOLD_MS).is_empty());
    let mut due = core.tick(T + 1 + HOLD_MS);
    due.sort();
    assert_eq!(due, [KEY.to_owned(), AGENT.to_owned()]);
    core.decide(AGENT, false, T + HOLD_MS);
    core.decide(KEY, true, T + HOLD_MS);
    assert!(lines(temp.path(), AGENT).is_empty());
    assert!(!core.journal.has_journal(AGENT));
    // With no turn open, the held end adds nothing.
    assert_eq!(
        shown(&lines(temp.path(), KEY)),
        ["message:seeded thread text"]
    );
    // A session found without a record keeps nothing until its next run.
    core.apply(event(AGENT, T + 2, delta("more"), None), T + 2);
    core.apply(
        event(AGENT, T + 3, EngineEventKind::SessionClosed, None),
        T + 3,
    );
    assert!(core.held_keys().is_empty());
    core.apply(
        event(AGENT, T + 4, delta("again"), Some(RunChange::Started)),
        T + 4,
    );
    core.apply(
        event(
            AGENT,
            T + 5,
            EngineEventKind::SessionClosed,
            Some(RunChange::Ended(RunOutcome::Interrupted)),
        ),
        T + 5,
    );
    // The close asks again at once.
    assert_eq!(core.held_keys(), [AGENT.to_owned()]);
}

#[test]
fn a_close_ends_the_message_then_the_turn_then_the_session() {
    let temp = private_temp();
    let mut core = core(temp.path());
    core.apply(prompt(KEY, T, TurnOrigin::Owner, "go"), T);
    core.apply(event(KEY, T + 1, delta("partial"), None), T + 1);
    let undecided = core.apply(
        event(
            KEY,
            T + 2,
            EngineEventKind::SessionClosed,
            Some(RunChange::Ended(RunOutcome::Interrupted)),
        ),
        T + 2,
    );
    assert!(undecided.is_empty());
    assert_eq!(
        shown(&lines(temp.path(), KEY)),
        [
            "owner>go",
            "message:partial",
            "end:interrupted",
            "sessionClosed"
        ]
    );
}

#[test]
fn held_entries_past_the_cap_are_released_at_once() {
    let temp = private_temp();
    let mut core = core(temp.path());
    let tool = |n: usize| {
        event(
            KEY,
            T,
            EngineEventKind::ToolResult {
                call_id: format!("c{n}"),
                ok: true,
                summary: String::new(),
                agent_key: None,
            },
            None,
        )
    };
    for n in 0..HOLD_CAP - 1 {
        assert!(core.apply(tool(n), T).is_empty());
    }
    assert_eq!(core.apply(tool(HOLD_CAP), T), [KEY.to_owned()]);
}

#[test]
fn an_import_writes_a_thread_with_no_journal_once() {
    let temp = private_temp();
    let mut core = core(temp.path());
    let history = vec![
        EngineEvent {
            at: Some("2026-11-14T10:00:00.000Z".into()),
            event: EngineEventKind::UserMessage {
                text: "Old question".into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
        },
        EngineEvent {
            at: Some("2026-11-14T10:00:01.000Z".into()),
            event: delta("Old answer"),
        },
    ];
    core.apply(
        JournalOp::Import {
            key: KEY.into(),
            events: history.clone(),
            older: false,
        },
        T,
    );
    assert_eq!(
        shown(&lines(temp.path(), KEY)),
        ["import>old question", "message:Old answer", "end:completed"]
    );
    // Never twice, and never over a live session's journal.
    core.apply(
        JournalOp::Import {
            key: KEY.into(),
            events: history.clone(),
            older: false,
        },
        T,
    );
    assert_eq!(lines(temp.path(), KEY).len(), 3);
    core.apply(prompt(AGENT, T, TurnOrigin::Owner, "live"), T);
    core.apply(
        JournalOp::Import {
            key: AGENT.into(),
            events: history,
            older: false,
        },
        T,
    );
    assert_eq!(shown(&lines(temp.path(), AGENT)), ["owner>live"]);
    // An empty history still marks the thread imported.
    core.apply(
        JournalOp::Import {
            key: "claude#empty".into(),
            events: Vec::new(),
            older: false,
        },
        T,
    );
    assert!(core.journal.has_journal("claude#empty"));
}

#[test]
fn removing_a_session_removes_its_journal() {
    let temp = private_temp();
    let mut core = core(temp.path());
    core.apply(prompt(KEY, T, TurnOrigin::Owner, "x"), T);
    assert!(core.journal.has_journal(KEY));
    core.apply(JournalOp::Removed { key: KEY.into() }, T);
    assert!(!core.journal.has_journal(KEY));
    assert!(
        !temp
            .path()
            .join(JOURNAL_DIR)
            .join(thread_journal::thread_hash(KEY))
            .exists()
    );
}

#[test]
fn appends_are_fsynced_within_two_seconds() {
    let temp = private_temp();
    let mut core = core(temp.path());
    let outcome = core.apply_batch(vec![prompt(KEY, T, TurnOrigin::Owner, "x")], T);
    assert!(outcome.undecided.is_empty() && outcome.drain.is_none());
    assert_eq!(core.next_deadline(), Some(T + SYNC_MS));
    core.apply_batch(Vec::new(), T + SYNC_MS - 1);
    assert!(core.journal.dirty());
    core.apply_batch(Vec::new(), T + SYNC_MS);
    assert!(!core.journal.dirty());
    assert_eq!(core.next_deadline(), None);
    // A drain stops the batch where it stands.
    let (done, _) = tokio::sync::oneshot::channel();
    let outcome = core.apply_batch(
        vec![
            JournalOp::Drain(done),
            prompt(KEY, T, TurnOrigin::Owner, "after the drain"),
        ],
        T,
    );
    assert!(outcome.drain.is_some());
    assert_eq!(lines(temp.path(), KEY).len(), 1);
}

#[test]
fn a_newer_search_cancels_the_ones_still_running() {
    let temp = private_temp();
    let handle = JournalHandle::new(ThreadJournalConfig {
        data_root: temp.path().to_path_buf(),
        notices: None,
    });
    let first = handle.begin_search();
    assert!(!first.cancelled());
    let second = handle.begin_search();
    assert!(first.cancelled());
    assert!(!second.cancelled());
    // A cancelled search stops before reading a thread.
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(
            KEY,
            T,
            JournalEntry::Message {
                text: "flaky".into(),
                agent_key: None,
                cut: false,
            },
        )
        .unwrap();
    journal.sync_all().unwrap();
    drop(journal);
    let search = || ThreadJournalCommand::Search {
        query: "flaky".into(),
    };
    let root = Some(temp.path().join(JOURNAL_DIR));
    assert_eq!(
        answer(root.clone(), search(), Some(&second))
            .hits
            .map(|hits| hits.len()),
        Some(1)
    );
    assert_eq!(answer(root, search(), Some(&first)).hits, Some(Vec::new()));
}

fn asked(n: usize) -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::UserMessage {
            text: format!("old prompt {n}"),
            agent_key: None,
            images: None,
            image_references: None,
        },
    }
}

fn answered(n: usize) -> EngineEvent {
    EngineEvent {
        at: None,
        event: delta(&format!("old answer {n}")),
    }
}

fn history_of(turns: std::ops::Range<usize>) -> Vec<EngineEvent> {
    turns.flat_map(|n| [asked(n), answered(n)]).collect()
}

/// What 0.3.177 and 0.3.178 wrote for a thread with a long history: the
/// newest items only, starting with an answer whose prompt was cut off.
fn short_import(data: &Path, key: &str) -> Vec<JournalLine> {
    let mut journal = ThreadJournal::open(data).unwrap();
    let mut events = vec![answered(299)];
    events.extend(history_of(300..400));
    for (at, entry) in thread_journal::import_entries(&events, T) {
        journal.append(key, at, entry).unwrap();
    }
    journal.sync_all().unwrap();
    drop(journal);
    lines(data, key)
}

fn rebuild(key: &str, turns: std::ops::Range<usize>) -> JournalOp {
    JournalOp::Rebuild {
        key: key.into(),
        events: history_of(turns),
        older: false,
    }
}

fn prompts(lines: &[JournalLine]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| match &line.entry {
            JournalEntry::TurnStarted { prompt, .. } => Some(prompt.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_journal_imported_short_is_rebuilt_once_from_the_whole_history() {
    let temp = private_temp();
    let before = short_import(temp.path(), KEY);
    assert_eq!(prompts(&before).len(), 100);
    let mut core = core(temp.path());
    assert_eq!(core.journal.short_import_candidates(), [KEY.to_owned()]);
    core.apply(rebuild(KEY, 0..400), T);
    let after = lines(temp.path(), KEY);
    assert_eq!(prompts(&after).len(), 400);
    assert_eq!(prompts(&after)[0], "old prompt 0");
    assert!(matches!(after[0].entry, JournalEntry::TurnStarted { .. }));
    assert_eq!(after.last().unwrap().seq, after.len() as u64);
    // Never again: not a candidate now, nor after a restart.
    assert!(core.journal.short_import_candidates().is_empty());
    drop(core);
    let mut core = super::JournalCore::new(ThreadJournal::open(temp.path()).unwrap());
    assert!(core.journal.short_import_candidates().is_empty());
    // It is the thread's journal: live writes go on after it.
    core.apply(prompt(KEY, T + 1, TurnOrigin::Owner, "live again"), T + 1);
    let live = lines(temp.path(), KEY);
    assert_eq!(live.len(), after.len() + 1);
    assert_eq!(live.last().unwrap().turn, Some(400));
    // Nothing was left beside it.
    let names = std::fs::read_dir(temp.path().join(JOURNAL_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != "journal.lock")
        .collect::<Vec<_>>();
    assert_eq!(names, [thread_journal::thread_hash(KEY)]);
}

#[test]
fn a_journal_with_live_turns_after_a_short_import_is_left_alone() {
    let temp = private_temp();
    short_import(temp.path(), KEY);
    {
        let mut core = core(temp.path());
        core.apply(prompt(KEY, T, TurnOrigin::Owner, "a live turn"), T);
        core.journal.sync_all().unwrap();
    }
    let before = lines(temp.path(), KEY);
    let mut core = core(temp.path());
    assert!(core.journal.short_import_candidates().is_empty());
    core.apply(rebuild(KEY, 0..400), T);
    assert_eq!(lines(temp.path(), KEY), before);
}

#[test]
fn a_history_that_would_lose_a_kept_prompt_rebuilds_nothing() {
    let temp = private_temp();
    let before = short_import(temp.path(), KEY);
    let mut core = core(temp.path());
    // Longer, but without prompts the journal kept (the provider's store
    // was compacted or replaced).
    core.apply(rebuild(KEY, 0..350), T);
    assert_eq!(lines(temp.path(), KEY), before);
    assert!(core.journal.short_import_candidates().is_empty());
}

#[test]
fn a_failed_rebuild_keeps_the_old_journal() {
    use std::os::unix::fs::PermissionsExt;
    let temp = private_temp();
    let before = short_import(temp.path(), KEY);
    let mut core = core(temp.path());
    let root = temp.path().join(JOURNAL_DIR);
    // Nothing can be staged beside the journal.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    core.apply(rebuild(KEY, 0..400), T);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(lines(temp.path(), KEY), before);
    // Tried again next launch.
    assert_eq!(core.journal.short_import_candidates(), [KEY.to_owned()]);
    drop(core);
    let core = super::JournalCore::new(ThreadJournal::open(temp.path()).unwrap());
    assert_eq!(core.journal.short_import_candidates(), [KEY.to_owned()]);
}
