use std::{
    fs,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, NoticeTone, SessionStatus},
    thread_journal::{JournalEntry, JournalLine, JournalRole, TurnOrigin},
    threads::RunOutcome,
};
use serde_json::json;

use super::{
    ChainSegment, EndedTurn, JOURNAL_DIR, JournalFold, JournalLimits, ThreadJournal, bounded_line,
    import_entries, last_answer, last_ended_turn, read, replay, replay_chain, replay_chain_history,
    replay_history, search, tail, thread_hash, watch,
};

const KEY: &str = "claude#11111111-1111-4111-8111-111111111111";
const OTHER: &str = "codex#22222222-2222-4222-8222-222222222222";

fn later() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn delta(text: &str, agent: Option<&str>) -> EngineEventKind {
    EngineEventKind::MessageDelta {
        text: text.into(),
        agent_key: agent.map(str::to_owned),
    }
}

fn tool(call: &str, agent: Option<&str>) -> EngineEventKind {
    EngineEventKind::ToolCall {
        call_id: call.into(),
        tool: "Bash".into(),
        summary: "ls".into(),
        input: json!({"command": "ls"}),
        agent_key: agent.map(str::to_owned),
    }
}

fn message(text: &str) -> JournalEntry {
    JournalEntry::Message {
        text: text.into(),
        agent_key: None,
        cut: false,
    }
}

fn prompt(origin: TurnOrigin, text: &str) -> JournalEntry {
    JournalEntry::TurnStarted {
        origin,
        prompt: text.into(),
        digest: crate::threads::prompt_digest(text),
        cut: false,
    }
}

fn ended(outcome: RunOutcome) -> JournalEntry {
    JournalEntry::TurnEnded {
        outcome,
        reason: None,
    }
}

fn kinds(lines: &[JournalLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            serde_json::to_value(&line.entry).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn texts(fold_out: &[JournalEntry]) -> Vec<String> {
    fold_out
        .iter()
        .map(|entry| match entry {
            JournalEntry::Message {
                text, agent_key, ..
            } => format!("{}:{text}", agent_key.as_deref().unwrap_or("main")),
            other => serde_json::to_value(other).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned(),
        })
        .collect()
}

fn root(data: &Path) -> std::path::PathBuf {
    data.join(JOURNAL_DIR)
}

fn private_temp() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

#[test]
fn deltas_join_until_that_agents_next_event_the_turn_end_or_the_close() {
    let mut fold = JournalFold::default();
    let mut out = Vec::new();
    for event in [
        delta("Hel", None),
        delta("sub ", Some("agent-1")),
        delta("lo", None),
        // Another agent's tool does not end the main agent's message.
        tool("call-1", Some("agent-1")),
        // Status, usage and thinking-off events are transparent...
        EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
        delta(" there", None),
        tool("call-2", None),
        delta("after", None),
        // ...but thinking ends the block, as the renderer does.
        EngineEventKind::ThinkingSignal {
            active: true,
            agent_key: None,
        },
        delta("tail", None),
        delta("sub tail", Some("agent-1")),
    ] {
        out.extend(fold.event(&event));
    }
    assert_eq!(
        texts(&out),
        [
            "agent-1:sub ",
            "toolCall",
            "main:Hello there",
            "toolCall",
            "main:after"
        ]
    );
    assert!(!fold.is_empty());
    // A turn's end flushes every agent, the main agent first.
    assert_eq!(texts(&fold.flush_all()), ["main:tail", "agent-1:sub tail"]);
    assert!(fold.is_empty());
    out.clear();
    out.extend(fold.event(&delta("last", None)));
    out.extend(fold.event(&EngineEventKind::SessionClosed));
    assert_eq!(texts(&out), ["main:last", "sessionClosed"]);
    // The cheap check agrees with the entry each event makes.
    for event in [
        tool("c", None),
        delta("x", None),
        EngineEventKind::SessionClosed,
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
        EngineEventKind::Notice {
            message: "n".into(),
            tone: NoticeTone::Info,
        },
    ] {
        assert_eq!(
            super::is_journalled(&event),
            super::entry_for_event(&event).is_some()
        );
    }
    // Events the journal leaves out make no entry.
    assert!(
        fold.event(&EngineEventKind::UsageUpdated {
            input_tokens: 1,
            output_tokens: 1,
            rate_limited: None,
        })
        .is_empty()
    );
}

#[test]
fn entries_past_64_kib_are_cut_at_a_character_boundary_and_say_so() {
    let mut line = JournalLine {
        seq: 1,
        at: 1,
        turn: Some(0),
        entry: JournalEntry::Message {
            text: "é".repeat(40_000),
            agent_key: None,
            cut: false,
        },
    };
    let bytes = bounded_line(&mut line);
    assert!(bytes.len() <= super::ENTRY_LIMIT, "{}", bytes.len());
    assert!(bytes.len() > super::ENTRY_LIMIT - 64);
    let JournalEntry::Message { text, cut, .. } = &line.entry else {
        panic!("message");
    };
    assert!(*cut);
    assert!(text.chars().all(|c| c == 'é'));
    // A small entry is untouched.
    let mut small = JournalLine {
        seq: 2,
        at: 1,
        turn: None,
        entry: message("short"),
    };
    let small_bytes = bounded_line(&mut small);
    assert_eq!(small.entry, message("short"));
    assert_eq!(small_bytes.last(), Some(&b'\n'));
    // A tool call loses its input first.
    let mut call = JournalLine {
        seq: 3,
        at: 1,
        turn: None,
        entry: JournalEntry::ToolCall {
            call_id: "c".into(),
            tool: "Write".into(),
            summary: "Write a.txt".into(),
            input: json!({"content": "x".repeat(100_000)}),
            agent_key: None,
            cut: false,
        },
    };
    assert!(bounded_line(&mut call).len() <= super::ENTRY_LIMIT);
    assert!(matches!(
        &call.entry,
        JournalEntry::ToolCall { input, summary, cut: true, .. } if input.is_null() && summary == "Write a.txt"
    ));
}

#[test]
fn appends_stamp_seq_and_turn_and_read_pages_through_them() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    assert!(!journal.has_journal(KEY));
    journal.append(KEY, 10, message("before any turn")).unwrap();
    journal
        .append(KEY, 11, prompt(TurnOrigin::Owner, "First"))
        .unwrap();
    journal.append(KEY, 12, message("one")).unwrap();
    journal
        .append(KEY, 13, ended(RunOutcome::Completed))
        .unwrap();
    let second = journal
        .append(KEY, 14, prompt(TurnOrigin::Owner, "Second"))
        .unwrap();
    assert_eq!((second.seq, second.turn), (5, Some(1)));
    assert!(journal.turn_open(KEY));
    journal.sync_all().unwrap();
    let root = root(temp.path());
    let page = read(&root, KEY, None, 2);
    assert_eq!(
        page.entries
            .iter()
            .map(|l| (l.seq, l.turn))
            .collect::<Vec<_>>(),
        [(1, None), (2, Some(0))]
    );
    assert_eq!(
        (page.first_seq, page.last_seq, page.floor_seq, page.has_more),
        (Some(1), 5, 1, true)
    );
    let page = read(&root, KEY, Some(2), 10);
    assert_eq!(
        kinds(&page.entries),
        ["message", "turnEnded", "turnStarted"]
    );
    assert!(!page.has_more);
    let page = tail(&root, KEY, 2);
    assert_eq!(
        page.entries.iter().map(|l| l.seq).collect::<Vec<_>>(),
        [4, 5]
    );
    assert!(page.has_more);
    let missing = read(&root, OTHER, None, 10);
    assert!(missing.entries.is_empty());
    assert_eq!(
        (missing.last_seq, missing.floor_seq, missing.has_more),
        (0, 0, false)
    );
    // A second writer is refused while the first holds the lock.
    assert!(ThreadJournal::open(temp.path()).is_err());
    drop(journal);
    let journal = ThreadJournal::open(temp.path()).unwrap();
    assert_eq!(journal.last_seq(KEY), 5);
    assert!(journal.turn_open(KEY) && journal.has_turn(KEY));
    let dir = root.join(thread_hash(KEY));
    assert_eq!(thread_hash(KEY).len(), 16);
    let first = fs::read_to_string(dir.join(format!("segment-{:020}.jsonl", 1))).unwrap();
    assert_eq!(
        first.lines().next().unwrap(),
        format!(r#"{{"v":1,"key":"{KEY}","firstSeq":1}}"#)
    );
}

#[test]
fn a_torn_last_line_is_cut_off_at_open_and_the_next_append_follows_it() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal.append(KEY, 1, message("one")).unwrap();
    journal.append(KEY, 2, message("two")).unwrap();
    drop(journal);
    let segment = root(temp.path())
        .join(thread_hash(KEY))
        .join(format!("segment-{:020}.jsonl", 1));
    let mut bytes = fs::read(&segment).unwrap();
    bytes.extend_from_slice(br#"{"seq":3,"at":3,"entry":{"kind":"mess"#);
    fs::write(&segment, &bytes).unwrap();
    // Readers skip it before the writer opens.
    assert_eq!(read(&root(temp.path()), KEY, None, 10).entries.len(), 2);
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    assert!(journal.take_quarantined().is_empty());
    assert_eq!(journal.append(KEY, 4, message("three")).unwrap().seq, 3);
    let page = read(&root(temp.path()), KEY, None, 10);
    assert_eq!(
        page.entries.iter().map(|l| l.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(!fs::read_to_string(&segment).unwrap().contains("mess\""));
    // A whole last line that lost only its newline is kept and ended.
    drop(journal);
    let mut bytes = fs::read(&segment).unwrap();
    bytes.pop();
    fs::write(&segment, &bytes).unwrap();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    assert_eq!(journal.append(KEY, 5, message("four")).unwrap().seq, 4);
    assert_eq!(read(&root(temp.path()), KEY, None, 10).entries.len(), 4);
}

fn small_limits() -> JournalLimits {
    JournalLimits {
        entry_bytes: 512,
        segment_bytes: 1024,
        retained_segments: 3,
        max_threads: 4096,
        max_total_bytes: 1 << 30,
    }
}

#[test]
fn segments_rotate_at_their_size_and_a_thread_keeps_its_newest() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open_with(temp.path(), small_limits()).unwrap();
    let text = "x".repeat(300);
    for at in 1..=12 {
        journal.append(KEY, at, message(&text)).unwrap();
    }
    assert_eq!(journal.segment_count(KEY), 3);
    let dir = root(temp.path()).join(thread_hash(KEY));
    let mut names = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    // Two entries a segment (a header and two ~345-byte lines; a third
    // would pass 1 KiB).
    assert_eq!(
        names,
        [7, 9, 11].map(|seq| format!("segment-{seq:020}.jsonl"))
    );
    for name in &names {
        assert!(fs::metadata(dir.join(name)).unwrap().len() <= 1024);
    }
    let page = read(&root(temp.path()), KEY, None, 100);
    assert_eq!(page.floor_seq, 7);
    assert_eq!(page.entries.first().unwrap().seq, 7);
    assert_eq!(page.last_seq, 12);
    // Reading after a seq that retention removed starts at the floor.
    assert_eq!(read(&root(temp.path()), KEY, Some(1), 1).entries[0].seq, 7);
    assert_eq!(
        read(&root(temp.path()), KEY, Some(8), 2)
            .entries
            .iter()
            .map(|l| l.seq)
            .collect::<Vec<_>>(),
        [9, 10]
    );
}

#[test]
fn past_the_directory_bounds_the_threads_idle_longest_go_first() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open_with(
        temp.path(),
        JournalLimits {
            max_total_bytes: 3 * 1024,
            retained_segments: 32,
            ..small_limits()
        },
    )
    .unwrap();
    let text = "y".repeat(300);
    for at in 1..=4 {
        journal.append(OTHER, at, message(&text)).unwrap();
    }
    assert_eq!(journal.segment_count(OTHER), 2);
    for at in 100..=104 {
        journal.append(KEY, at, message(&text)).unwrap();
    }
    // KEY's third segment pushed the directory over 3 KiB: OTHER (idle
    // longest) lost its oldest segment.
    assert!(
        journal.total_bytes() <= 3 * 1024,
        "{}",
        journal.total_bytes()
    );
    assert_eq!(journal.segment_count(KEY), 3);
    assert_eq!(journal.segment_count(OTHER), 1);
    assert_eq!(read(&root(temp.path()), OTHER, None, 10).floor_seq, 3);
    for at in 200..=201 {
        journal.append(KEY, at, message(&text)).unwrap();
    }
    // Its last segment went next, and the thread with it.
    assert!(!journal.has_journal(OTHER));
    assert!(!root(temp.path()).join(thread_hash(OTHER)).exists());
    assert!(journal.total_bytes() <= 3 * 1024);
    // At the thread bound, a new thread pushes out the idlest whole thread.
    drop(journal);
    let mut journal = ThreadJournal::open_with(
        temp.path(),
        JournalLimits {
            max_threads: 2,
            ..small_limits()
        },
    )
    .unwrap();
    journal
        .append("claude#third", 500, message("third"))
        .unwrap();
    journal
        .append("claude#fourth", 600, message("fourth"))
        .unwrap();
    assert!(!journal.has_journal(KEY));
    assert!(journal.has_journal("claude#third") && journal.has_journal("claude#fourth"));
    assert_eq!(journal.thread_count(), 2);
}

#[test]
fn a_segment_that_does_not_parse_is_quarantined_and_the_thread_continues() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    for at in 1..=3 {
        journal.append(KEY, at, message("ok")).unwrap();
    }
    drop(journal);
    let dir = root(temp.path()).join(thread_hash(KEY));
    let segment = dir.join(format!("segment-{:020}.jsonl", 1));
    let text = fs::read_to_string(&segment).unwrap();
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    lines[2] = "not json".into();
    fs::write(&segment, lines.join("\n") + "\n").unwrap();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    let quarantined = journal.take_quarantined();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        quarantined[0],
        root(temp.path()).join("quarantine").join(format!(
            "{}-segment-{:020}.jsonl",
            thread_hash(KEY),
            1
        ))
    );
    assert!(quarantined[0].exists() && !segment.exists());
    // The last readable seq was 1: the thread goes on at 2.
    assert_eq!(journal.append(KEY, 9, message("after")).unwrap().seq, 2);
    let page = read(&root(temp.path()), KEY, None, 10);
    assert_eq!(page.entries.iter().map(|l| l.seq).collect::<Vec<_>>(), [2]);
    assert_eq!(page.floor_seq, 2);
}

#[test]
fn a_turn_open_at_start_gets_the_restart_marker() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(KEY, 1, prompt(TurnOrigin::Owner, "Go"))
        .unwrap();
    journal.append(KEY, 2, message("working")).unwrap();
    journal
        .append(OTHER, 1, prompt(TurnOrigin::Owner, "Done"))
        .unwrap();
    journal
        .append(OTHER, 2, ended(RunOutcome::Completed))
        .unwrap();
    journal.sync_all().unwrap();
    assert!(watch(&root(temp.path()), KEY).turn_open);
    drop(journal);
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    assert_eq!(journal.close_open_turns(50).unwrap(), [KEY]);
    assert!(!journal.turn_open(KEY));
    let page = tail(&root(temp.path()), KEY, 1);
    assert_eq!(
        page.entries[0].entry,
        JournalEntry::TurnEnded {
            outcome: RunOutcome::Interrupted,
            reason: Some("restart".into()),
        }
    );
    assert_eq!(
        (
            page.entries[0].seq,
            page.entries[0].at,
            page.entries[0].turn
        ),
        (3, 50, Some(0))
    );
    let watched = watch(&root(temp.path()), KEY);
    assert_eq!((watched.last_seq, watched.turn_open), (3, false));
    // Nothing is open any more.
    drop(journal);
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    assert!(journal.close_open_turns(60).unwrap().is_empty());
}

#[test]
fn a_retracted_turn_is_left_out_of_replay_search_and_the_last_answer() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(KEY, 1, prompt(TurnOrigin::Owner, "kept prompt"))
        .unwrap();
    journal.append(KEY, 2, message("kept answer")).unwrap();
    journal
        .append(KEY, 3, ended(RunOutcome::Completed))
        .unwrap();
    journal
        .append(KEY, 4, prompt(TurnOrigin::Owner, "never sent prompt"))
        .unwrap();
    journal
        .append(KEY, 5, JournalEntry::TurnRetracted {})
        .unwrap();
    let root = root(temp.path());
    let replayed = replay(&root, KEY, 100);
    assert_eq!(
        replayed.iter().map(|e| e.event.clone()).collect::<Vec<_>>(),
        [
            EngineEventKind::UserMessage {
                text: "kept prompt".into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
            delta("kept answer", None),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle
            },
        ]
    );
    assert_eq!(replayed[0].at.as_deref(), Some("1970-01-01T00:00:00.001Z"));
    assert!(search(&root, "never sent", later()).is_empty());
    assert_eq!(search(&root, "PROMPT", later()).len(), 1);
    assert_eq!(last_answer(&root, KEY).as_deref(), Some("kept answer"));
    // The raw page still shows the retraction.
    assert_eq!(
        kinds(&read(&root, KEY, Some(3), 10).entries),
        ["turnStarted", "turnRetracted"]
    );
    // The turn after a retracted one numbers on.
    assert_eq!(
        journal
            .append(KEY, 6, prompt(TurnOrigin::Owner, "again"))
            .unwrap()
            .turn,
        Some(2)
    );
}

/// Team tools (T3 section 2) read the newest ended turn: its number, how it
/// ended, its last main-agent message and its last error. A retracted turn
/// and an open one are passed over.
#[test]
fn the_last_ended_turn_names_its_answer_outcome_and_error() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    let root = root(temp.path());
    assert_eq!(last_ended_turn(&root, KEY), None);
    journal
        .append(KEY, 1, prompt(TurnOrigin::Owner, "one"))
        .unwrap();
    journal.append(KEY, 2, message("first part")).unwrap();
    journal.append(KEY, 3, message("final answer")).unwrap();
    journal
        .append(KEY, 4, ended(RunOutcome::Completed))
        .unwrap();
    let done = Some(EndedTurn {
        turn: 0,
        outcome: RunOutcome::Completed,
        answer: Some("final answer".into()),
        error: None,
    });
    assert_eq!(last_ended_turn(&root, KEY), done);
    // A turn still open, and one taken back, do not count.
    journal
        .append(KEY, 5, prompt(TurnOrigin::Host, "two"))
        .unwrap();
    assert_eq!(last_ended_turn(&root, KEY), done);
    journal
        .append(KEY, 6, JournalEntry::TurnRetracted {})
        .unwrap();
    assert_eq!(last_ended_turn(&root, KEY), done);
    journal
        .append(KEY, 7, prompt(TurnOrigin::Host, "three"))
        .unwrap();
    journal
        .append(
            KEY,
            8,
            JournalEntry::Error {
                message: "the provider stopped".into(),
                retryable: false,
                cause: None,
            },
        )
        .unwrap();
    journal.append(KEY, 9, ended(RunOutcome::Failed)).unwrap();
    assert_eq!(
        last_ended_turn(&root, KEY),
        Some(EndedTurn {
            turn: 2,
            outcome: RunOutcome::Failed,
            answer: None,
            error: Some("the provider stopped".into()),
        })
    );
}

#[test]
fn replay_puts_a_launch_prompt_before_the_entries_it_started() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    // A launch whose bind reached the journal after its answer began.
    journal.append(KEY, 1, message("answer to launch")).unwrap();
    journal
        .append(KEY, 2, prompt(TurnOrigin::Launch, "launch prompt"))
        .unwrap();
    journal.append(KEY, 3, message("more")).unwrap();
    journal
        .append(KEY, 4, ended(RunOutcome::Completed))
        .unwrap();
    let events = replay(&root(temp.path()), KEY, 100);
    assert!(
        matches!(&events[0].event, EngineEventKind::UserMessage { text, .. } if text == "launch prompt")
    );
    assert_eq!(events[1].event, delta("answer to launch", None));
    let page = replay_history(&root(temp.path()), KEY, 500).unwrap();
    assert_eq!((page.messages, page.older_messages), (Some(3), 0));
    assert!(page.availability.is_none());
    assert!(replay_history(&root(temp.path()), OTHER, 500).is_none());
}

#[test]
fn search_finds_prompts_and_messages_newest_threads_first_five_a_thread() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(OTHER, 1, prompt(TurnOrigin::Owner, "Fix the Flaky test"))
        .unwrap();
    for n in 0..7 {
        journal
            .append(KEY, 10 + n, message(&format!("flaky number {n}")))
            .unwrap();
    }
    journal.append(KEY, 20, message("Ünïcode FLAKY")).unwrap();
    journal
        .append(
            KEY,
            21,
            JournalEntry::Notice {
                message: "flaky notice".into(),
                tone: NoticeTone::Info,
            },
        )
        .unwrap();
    journal.sync_all().unwrap();
    let root = root(temp.path());
    let set = |key: &str, secs: u64| {
        let dir = root.join(thread_hash(key));
        for entry in fs::read_dir(dir).unwrap() {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(entry.unwrap().path())
                .unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        }
    };
    set(OTHER, 2_000);
    set(KEY, 1_000);
    let hits = search(&root, "flaky", later());
    assert_eq!(hits.len(), 6);
    assert_eq!(hits[0].key, OTHER);
    assert_eq!(
        (hits[0].role, hits[0].excerpt.as_str()),
        (JournalRole::Prompt, "Fix the Flaky test")
    );
    assert_eq!(hits[1].excerpt, "Ünïcode FLAKY");
    assert_eq!(hits[1].role, JournalRole::Message);
    assert!(hits[1..].iter().all(|hit| hit.key == KEY));
    assert_eq!(hits[2].excerpt, "flaky number 6");
    set(KEY, 3_000);
    assert_eq!(search(&root, "flaky", later())[0].key, KEY);
    // A spent deadline answers what it has: nothing.
    assert!(
        search(
            &root,
            "flaky",
            Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap()
        )
        .is_empty()
    );
    assert!(search(&root, "   ", later()).is_empty());
    assert!(search(&root, "absent", later()).is_empty());
}

#[test]
fn excerpts_keep_a_little_text_around_the_match_on_one_line() {
    let long = format!("{}needle{}", "a ".repeat(60), "b\n".repeat(60));
    let excerpt = super::read::excerpt_for_tests(&long, "needle").unwrap();
    assert!(
        excerpt.starts_with('…') && excerpt.ends_with('…'),
        "{excerpt}"
    );
    assert!(excerpt.contains("needle"));
    assert!(!excerpt.contains('\n'));
    assert!(excerpt.chars().count() < 140);
    assert_eq!(
        super::read::excerpt_for_tests("İstanbul trip", "trip").as_deref(),
        Some("İstanbul trip")
    );
    assert!(super::read::excerpt_for_tests("nothing", "needle").is_none());
}

#[test]
fn import_turns_each_user_message_into_an_imported_turn() {
    let at = |ms: &str| Some(format!("2026-11-14T10:00:00.{ms}Z"));
    let events = [
        EngineEvent {
            at: at("000"),
            event: EngineEventKind::UserMessage {
                text: "First ask".into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
        },
        EngineEvent {
            at: at("100"),
            event: delta("Part one, ", None),
        },
        EngineEvent {
            at: at("200"),
            event: delta("part two", None),
        },
        EngineEvent {
            at: at("300"),
            event: tool("call-1", None),
        },
        EngineEvent {
            at: None,
            event: EngineEventKind::UserMessage {
                text: "Second ask".into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
        },
        EngineEvent {
            at: at("500"),
            event: delta("Second answer", None),
        },
    ];
    let entries = import_entries(&events, 7);
    let shown = entries
        .iter()
        .map(|(at, entry)| (*at % 1_000, texts(std::slice::from_ref(entry)).remove(0)))
        .collect::<Vec<_>>();
    assert_eq!(
        shown,
        [
            (0, "turnStarted".to_owned()),
            (300, "main:Part one, part two".to_owned()),
            (300, "toolCall".to_owned()),
            (300, "turnEnded".to_owned()),
            (300, "turnStarted".to_owned()),
            (500, "main:Second answer".to_owned()),
            (500, "turnEnded".to_owned()),
        ]
    );
    assert!(
        matches!(&entries[0].1, JournalEntry::TurnStarted { origin: TurnOrigin::Import, prompt, .. } if prompt == "First ask")
    );
    assert_eq!(
        entries[3].1,
        JournalEntry::TurnEnded {
            outcome: RunOutcome::Completed,
            reason: Some("import".into())
        }
    );
    // Written to a journal they read back as two ended turns.
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    for (at, entry) in entries {
        journal.append(KEY, at, entry).unwrap();
    }
    assert!(!journal.turn_open(KEY));
    assert_eq!(
        last_answer(&root(temp.path()), KEY).as_deref(),
        Some("Second answer")
    );
    assert!(import_entries(&[], 1).is_empty());
}

#[test]
fn removing_a_thread_deletes_its_journal_and_begin_marks_an_empty_one() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal.append(KEY, 1, message("x")).unwrap();
    journal.begin(OTHER).unwrap();
    assert!(journal.has_journal(OTHER) && !journal.has_turn(OTHER));
    assert_eq!(read(&root(temp.path()), OTHER, None, 10).last_seq, 0);
    journal.remove(KEY).unwrap();
    assert!(!journal.has_journal(KEY));
    assert!(!root(temp.path()).join(thread_hash(KEY)).exists());
    drop(journal);
    let journal = ThreadJournal::open(temp.path()).unwrap();
    assert!(journal.has_journal(OTHER) && !journal.has_journal(KEY));
    assert_eq!(journal.thread_count(), 1);
}

/// One line of a replayed conversation, for comparing: who spoke and what.
fn said(event: &EngineEvent) -> String {
    match &event.event {
        EngineEventKind::UserMessage { text, .. } => format!("user: {text}"),
        EngineEventKind::MessageDelta { text, .. } => format!("assistant: {text}"),
        EngineEventKind::Notice { message, .. } => format!("notice: {message}"),
        EngineEventKind::StatusChanged { .. } => "end".into(),
        other => format!("{other:?}"),
    }
}

/// T3 section 5: a switched thread's segments read as one conversation,
/// merged by time with "Switched to" where the provider changes; the newest
/// `limit` events are kept, and `since` keeps only what came after.
#[test]
fn a_switched_threads_segments_replay_as_one_conversation() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(KEY, 1_000, prompt(TurnOrigin::Owner, "fix the login"))
        .unwrap();
    journal.append(KEY, 1_100, message("fixed it")).unwrap();
    journal
        .append(KEY, 1_200, ended(RunOutcome::Completed))
        .unwrap();
    journal
        .append(
            OTHER,
            2_000,
            prompt(TurnOrigin::Host, "You are taking over a thread from Claude"),
        )
        .unwrap();
    journal
        .append(OTHER, 2_100, message("where it stands"))
        .unwrap();
    journal
        .append(OTHER, 2_200, ended(RunOutcome::Completed))
        .unwrap();
    journal
        .append(OTHER, 3_000, prompt(TurnOrigin::Owner, "add a test"))
        .unwrap();
    journal.append(OTHER, 3_100, message("added")).unwrap();
    journal
        .append(OTHER, 3_200, ended(RunOutcome::Completed))
        .unwrap();
    drop(journal);
    let segments = [
        ChainSegment {
            key: KEY,
            label: "Claude",
        },
        ChainSegment {
            key: OTHER,
            label: "Codex",
        },
    ];
    let root = root(temp.path());
    let events = replay_chain(&root, &segments, 100, None);
    assert_eq!(
        events.iter().map(said).collect::<Vec<_>>(),
        [
            "user: fix the login",
            "assistant: fixed it",
            "end",
            "notice: Switched to Codex",
            "user: You are taking over a thread from Claude",
            "assistant: where it stands",
            "end",
            "user: add a test",
            "assistant: added",
            "end",
        ]
    );
    assert!(matches!(
        events[3].event,
        EngineEventKind::Notice {
            tone: NoticeTone::Info,
            ..
        }
    ));
    // What Claude missed since it last ran (1 200 ms): Codex's stretch only.
    let missed = replay_chain(&root, &segments[1..], 100, Some(1_200));
    assert_eq!(
        missed.iter().map(said).collect::<Vec<_>>()[3..],
        ["user: add a test", "assistant: added", "end"]
    );
    assert!(
        replay_chain(&root, &segments[1..], 100, Some(3_200)).is_empty(),
        "nothing after its end"
    );
    // The newest `limit` events, and the page says older ones exist.
    let page = replay_chain_history(&root, &segments, 4).unwrap();
    assert_eq!(
        page.events.iter().map(said).collect::<Vec<_>>(),
        ["end", "user: add a test", "assistant: added", "end"]
    );
    assert_eq!((page.messages, page.older_messages), (Some(2), 1));
    assert!(replay_chain_history(&root, &[], 10).is_none());
}

/// T3 section 8: a visual the agent showed folds into a `visual` entry that
/// ends the message before it, and replays as `visual.shown` in a thread's
/// replay, its history fallback and a switched thread's chain history.
#[test]
fn a_visual_folds_into_the_journal_and_replays_as_visual_shown() {
    let shown = EngineEventKind::VisualShown {
        id: "v0000000001150-0a1b2c3d".into(),
        title: "Signups by week".into(),
        bytes: 1_834,
    };
    let mut fold = JournalFold::default();
    let mut out = fold.event(&delta("Here it is:", None));
    out.extend(fold.event(&shown));
    out.extend(fold.event(&delta("Signups rose.", None)));
    out.extend(fold.flush_all());
    assert_eq!(
        texts(&out),
        ["main:Here it is:", "visual", "main:Signups rose."]
    );
    assert!(super::is_journalled(&shown));
    assert_eq!(
        out[1],
        JournalEntry::Visual {
            id: "v0000000001150-0a1b2c3d".into(),
            title: "Signups by week".into(),
            bytes: 1_834,
        }
    );

    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(KEY, 1_000, prompt(TurnOrigin::Owner, "chart signups"))
        .unwrap();
    for (offset, entry) in out.into_iter().enumerate() {
        journal
            .append(KEY, 1_100 + offset as u64 * 50, entry)
            .unwrap();
    }
    journal
        .append(KEY, 1_300, ended(RunOutcome::Completed))
        .unwrap();
    journal
        .append(
            OTHER,
            2_000,
            prompt(TurnOrigin::Host, "You are taking over"),
        )
        .unwrap();
    drop(journal);
    let root = root(temp.path());
    let is_visual = |event: &EngineEvent| event.event == shown;
    let replayed = replay(&root, KEY, 100);
    assert_eq!(replayed.iter().filter(|event| is_visual(event)).count(), 1);
    let at = replayed
        .iter()
        .find(|event| is_visual(event))
        .and_then(|event| event.at.clone());
    assert_eq!(at.as_deref(), Some("1970-01-01T00:00:01.150Z"));
    let history = replay_history(&root, KEY, 100).unwrap();
    assert!(history.events.iter().any(is_visual));
    let segments = [
        ChainSegment {
            key: KEY,
            label: "Claude",
        },
        ChainSegment {
            key: OTHER,
            label: "Codex",
        },
    ];
    let chain = replay_chain_history(&root, &segments, 100).unwrap();
    assert_eq!(
        chain.events.iter().filter(|event| is_visual(event)).count(),
        1
    );
    // A team tools reader sees one line saying what was shown.
    let page = read(&root, KEY, None, 100);
    let line = page
        .entries
        .iter()
        .find(|line| matches!(line.entry, JournalEntry::Visual { .. }))
        .unwrap();
    assert_eq!(
        crate::thread_team::compact_line(line)["text"],
        "showed a visual: Signups by week"
    );
}

/// T3 section 8: a provider keeps no record of a visual Interface showed;
/// the journal's visuals go into its history page after the call that made
/// them, else by time, else at the end of a whole page.
#[test]
#[allow(clippy::too_many_lines)] // One journal, then every placement rule.
fn visuals_merge_into_a_provider_history_page_by_time() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    let visual = |id: &str| JournalEntry::Visual {
        id: id.into(),
        title: id.into(),
        bytes: 10,
    };
    journal
        .append(KEY, 1_000, prompt(TurnOrigin::Owner, "chart"))
        .unwrap();
    journal.append(KEY, 1_500, visual("early")).unwrap();
    journal.append(KEY, 2_500, visual("middle")).unwrap();
    journal.append(KEY, 9_000, visual("late")).unwrap();
    drop(journal);
    let root = root(temp.path());
    let ids = |events: &[EngineEvent]| {
        events
            .iter()
            .map(|event| match &event.event {
                EngineEventKind::VisualShown { id, .. } => format!("visual:{id}"),
                EngineEventKind::MessageDelta { text, .. } => text.clone(),
                _ => "other".into(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&super::visual_events(&root, KEY, None)),
        ["visual:early", "visual:middle", "visual:late"]
    );
    assert_eq!(
        ids(&super::visual_events(&root, KEY, Some(2_000))),
        ["visual:middle", "visual:late"]
    );
    let at = |ms: i64| {
        Some(interface_common::time::unix_millis_iso(
            ms,
            interface_common::time::IsoFraction::Millis,
        ))
    };
    let said = |ms: i64, text: &str| EngineEvent {
        at: at(ms),
        event: delta(text, None),
    };
    let page = |older: u64| interface_contracts::ipc::HistoryPage {
        availability: None,
        reason: None,
        events: vec![said(1_200, "a"), said(2_000, "b"), said(3_000, "c")],
        messages: Some(3),
        older_messages: older,
    };
    let visuals = super::visual_events(&root, KEY, None);
    let mut whole = page(0);
    super::merge_visuals(&mut whole, visuals.clone());
    assert_eq!(
        ids(&whole.events),
        [
            "a",
            "visual:early",
            "b",
            "visual:middle",
            "c",
            "visual:late"
        ]
    );
    // Merging again adds nothing.
    super::merge_visuals(&mut whole, visuals.clone());
    assert_eq!(whole.events.len(), 6);
    // Older events exist past a page that starts at 2 000 ms: the visual
    // from before it belongs to the part not shown.
    let mut newer = page(1);
    newer.events.remove(0);
    super::merge_visuals(&mut newer, visuals.clone());
    assert_eq!(
        ids(&newer.events),
        ["b", "visual:middle", "c", "visual:late"]
    );
    // A page with no times gets them at its end.
    let mut untimed = page(0);
    for event in &mut untimed.events {
        event.at = None;
    }
    super::merge_visuals(&mut untimed, visuals.clone());
    assert_eq!(
        ids(&untimed.events),
        [
            "a",
            "b",
            "c",
            "visual:early",
            "visual:middle",
            "visual:late"
        ]
    );
    // A provider that keeps no times (Claude's store): a visual goes right
    // after the show_visual result that names it; one that cannot be placed
    // in a page with older events past it is left out.
    let answered = |id: &str| EngineEvent {
        at: None,
        event: EngineEventKind::ToolResult {
            call_id: "call-1".into(),
            ok: true,
            summary: format!("{{\"bytes\":10,\"id\":\"{id}\"}}"),
            agent_key: None,
        },
    };
    let mut claude = page(1);
    for event in &mut claude.events {
        event.at = None;
    }
    claude.events.insert(1, answered("middle"));
    super::merge_visuals(&mut claude, visuals);
    assert_eq!(
        ids(&claude.events),
        ["a", "other", "visual:middle", "b", "c"]
    );
}

#[test]
fn only_two_characters_outside_ascii_lower_case_into_ascii() {
    // The search's raw-byte prefilter relies on this list being complete.
    let folding = (0x80..=0x10_FFFF_u32)
        .filter_map(char::from_u32)
        .filter(|character| character.to_lowercase().any(|lower| lower.is_ascii()))
        .collect::<Vec<_>>();
    assert_eq!(folding, ['\u{130}', '\u{212A}']);
}

#[test]
fn search_skips_unparsed_lines_without_changing_what_it_finds() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal
        .append(KEY, 1, prompt(TurnOrigin::Owner, "say \"KELVIN\" twice"))
        .unwrap();
    // A tool call's input holds the word: never a hit, as before.
    journal
        .append(
            KEY,
            2,
            JournalEntry::ToolCall {
                call_id: "c".into(),
                tool: "Bash".into(),
                summary: "kelvin".into(),
                input: json!({"command": "echo kelvin"}),
                agent_key: None,
                cut: false,
            },
        )
        .unwrap();
    // Characters outside ASCII that lower-case into the needle still match.
    journal
        .append(KEY, 3, message("\u{212A}ELVIN with the Kelvin sign"))
        .unwrap();
    journal
        .append(KEY, 4, message("D\u{130}ZZY on line\ntwo"))
        .unwrap();
    journal
        .append(KEY, 5, message("a \"quoted\" word and a back\\slash"))
        .unwrap();
    // A taken-back turn's prompt is not found.
    journal
        .append(KEY, 6, prompt(TurnOrigin::Owner, "kelvin retracted"))
        .unwrap();
    journal
        .append(KEY, 7, JournalEntry::TurnRetracted {})
        .unwrap();
    journal.sync_all().unwrap();
    let root = root(temp.path());
    let found = |query: &str| {
        search(&root, query, later())
            .into_iter()
            .map(|hit| hit.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(found("kelvin"), [3, 1]);
    assert_eq!(found("\"kelvin\""), [1]);
    assert_eq!(found("di\u{307}zzy"), [4]);
    assert_eq!(found("dizzy"), Vec::<u64>::new());
    assert_eq!(found("line two"), Vec::<u64>::new());
    assert_eq!(found("\"QUOTED\" WORD"), [5]);
    assert_eq!(found("back\\slash"), [5]);
    assert_eq!(found("echo"), Vec::<u64>::new());
    assert_eq!(found("retracted"), Vec::<u64>::new());
}

#[test]
fn a_search_a_newer_one_cancels_stops_and_says_it_was_cut_short() {
    let temp = private_temp();
    let mut journal = ThreadJournal::open(temp.path()).unwrap();
    journal.append(KEY, 1, message("flaky")).unwrap();
    journal.append(OTHER, 1, message("flaky too")).unwrap();
    journal.sync_all().unwrap();
    let root = root(temp.path());
    let whole = super::search_with(&root, "flaky", later(), &|| false);
    assert_eq!((whole.hits.len(), whole.cut_short), (2, false));
    let cancelled = super::search_with(&root, "flaky", later(), &|| true);
    assert_eq!((cancelled.hits.len(), cancelled.cut_short), (0, true));
    // Cancelled after its first segment: the rest is not read.
    let checks = std::cell::Cell::new(0);
    let partial = super::search_with(&root, "flaky", later(), &|| {
        checks.set(checks.get() + 1);
        checks.get() > 2
    });
    assert_eq!((partial.hits.len(), partial.cut_short), (1, true));
    let late = super::search_with(&root, "flaky", Instant::now(), &|| false);
    assert!(late.cut_short && late.hits.is_empty());
}

#[test]
fn an_import_starts_at_a_prompt_and_an_unanswered_turn_ends_interrupted() {
    let user = |text: &str| EngineEvent {
        at: None,
        event: EngineEventKind::UserMessage {
            text: text.into(),
            agent_key: None,
            images: None,
            image_references: None,
        },
    };
    let said = |text: &str| EngineEvent {
        at: None,
        event: delta(text, None),
    };
    // A page that left its oldest part out begins with an answer whose
    // prompt is not in it; the last turn was cut off before any answer.
    let events = [
        said("the end of an older answer"),
        user("asked"),
        said("answered"),
        user("cut off"),
    ];
    let entries = super::import_entries_from(&events, 7, true);
    assert_eq!(
        texts(
            &entries
                .iter()
                .map(|(_, entry)| entry.clone())
                .collect::<Vec<_>>()
        ),
        [
            "turnStarted",
            "main:answered",
            "turnEnded",
            "turnStarted",
            "turnEnded"
        ]
    );
    let ends = entries
        .iter()
        .filter_map(|(_, entry)| match entry {
            JournalEntry::TurnEnded { outcome, reason } => Some((*outcome, reason.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ends,
        [
            (RunOutcome::Completed, Some("import".to_owned())),
            (RunOutcome::Interrupted, Some("import".to_owned())),
        ]
    );
    // A whole history keeps what comes before its first prompt.
    assert_eq!(super::import_entries_from(&events, 7, false).len(), 6);
}
