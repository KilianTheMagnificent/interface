use interface_contracts::threads::RunOutcome;
use serde_json::json;

use super::*;

const ENGINES: [&str; 5] = ["claude", "codex", "acp:grok", "acp:qwen", "acp:custom-tool"];

/// Every mode each engine names, with the rung a caller in it stands on.
const CALLER_MODES: [(&str, Option<&str>, Rung); 17] = [
    ("claude", Some("plan"), Rung::Plan),
    ("claude", Some("dontAsk"), Rung::Plan),
    ("claude", Some("default"), Rung::Ask),
    ("claude", None, Rung::Ask),
    ("claude", Some("acceptEdits"), Rung::Edits),
    ("claude", Some("auto"), Rung::Edits),
    ("claude", Some("bypassPermissions"), Rung::Full),
    ("codex", Some("untrusted"), Rung::Ask),
    ("codex", Some("on-request"), Rung::Edits),
    ("codex", Some("on-failure"), Rung::Edits),
    ("codex", Some("never"), Rung::Edits),
    ("acp:grok", Some("default"), Rung::Ask),
    ("acp:grok", Some("auto"), Rung::Edits),
    ("acp:grok", Some("bypassPermissions"), Rung::Full),
    ("acp:qwen", Some("default"), Rung::Ask),
    ("acp:custom-tool", Some("bypassPermissions"), Rung::Ask),
    ("claude", Some("something-new"), Rung::Ask),
];

/// The rung each engine's chosen mode stands on, at its most, so a started
/// thread's mode can be checked against its caller's rung.
fn reach(engine: &str, mode: &str) -> Rung {
    match (engine, mode) {
        ("claude", "plan") => Rung::Plan,
        ("claude", "acceptEdits") | ("codex", "on-request") => Rung::Edits,
        ("claude" | "acp:grok", "bypassPermissions") | ("codex", "never") => Rung::Full,
        _ => Rung::Ask,
    }
}

#[test]
fn the_ladder_reads_every_engine_mode_conservatively() {
    for (engine, mode, rung) in CALLER_MODES {
        assert_eq!(caller_rung(engine, mode), rung, "{engine} {mode:?}");
    }
    assert!(Rung::Plan < Rung::Ask && Rung::Ask < Rung::Edits && Rung::Edits < Rung::Full);
}

/// For every caller engine and mode and every target engine, a started
/// thread runs at the caller's rung or the nearest lower one the target has,
/// never wider; a target with nothing that low refuses.
#[test]
fn a_started_thread_never_runs_wider_than_its_caller_for_every_engine_pair() {
    for (caller, mode, rung) in CALLER_MODES {
        for target in ENGINES {
            match started_mode(caller, mode, target) {
                Ok((at, chosen)) => {
                    assert!(at <= rung, "{caller} {mode:?} → {target}: {at:?}");
                    assert!(
                        reach(target, chosen) <= rung,
                        "{caller} {mode:?} → {target}: {chosen} is wider"
                    );
                    // The nearest lower rung: nothing the target has between.
                    let higher = child_mode(target, rung).map(|(at, _)| at);
                    assert_eq!(higher, Some(at));
                }
                Err(refusal) => {
                    assert_eq!(refusal.code, MODE_UNAVAILABLE);
                    assert_eq!(child_mode(target, rung), None);
                }
            }
        }
    }
    // The exact choices.
    let table = [
        ("claude", Some("bypassPermissions"), "codex", Some("never")),
        ("claude", Some("acceptEdits"), "codex", Some("on-request")),
        ("claude", None, "codex", Some("untrusted")),
        ("claude", Some("plan"), "codex", None),
        ("claude", Some("plan"), "claude", Some("plan")),
        ("codex", Some("never"), "claude", Some("acceptEdits")),
        ("codex", Some("never"), "codex", Some("on-request")),
        ("codex", Some("on-request"), "claude", Some("acceptEdits")),
        ("codex", Some("on-request"), "acp:grok", Some("default")),
        (
            "acp:grok",
            Some("bypassPermissions"),
            "acp:grok",
            Some("bypassPermissions"),
        ),
        ("acp:grok", Some("auto"), "claude", Some("acceptEdits")),
        ("codex", Some("never"), "acp:qwen", Some("default")),
        ("claude", Some("plan"), "acp:qwen", None),
        ("claude", Some("bypassPermissions"), "gemini", None),
    ];
    for (caller, mode, target, expected) in table {
        assert_eq!(
            started_mode(caller, mode, target)
                .ok()
                .map(|(_, mode)| mode),
            expected,
            "{caller} {mode:?} → {target}"
        );
    }
}

#[test]
fn start_limits_refuse_with_the_reason() {
    assert!(check_start_limits(0, 0, 0).is_ok());
    assert!(check_start_limits(3, 15, MAX_STARTED - 1).is_ok());
    assert_eq!(
        check_start_limits(4, 5, 10).unwrap_err().code,
        LIMIT_RUNNING
    );
    assert_eq!(check_start_limits(0, 16, 10).unwrap_err().code, LIMIT_TOTAL);
    assert_eq!(check_start_limits(4, 16, 10).unwrap_err().code, LIMIT_TOTAL);
    assert_eq!(
        check_start_limits(0, 0, MAX_STARTED).unwrap_err().code,
        TEAM_FULL
    );
    let refusal = check_start_limits(4, 4, 4).unwrap_err();
    assert!(
        refusal.message.contains("still running"),
        "{}",
        refusal.message
    );
    assert_eq!(
        refusal.to_json(),
        json!({"code": "limit-running", "message": refusal.message})
    );
}

#[test]
fn reading_takes_the_same_project_and_control_takes_having_started_it() {
    let table = [
        (Some("/p/atlas"), Some("/p/atlas"), true),
        (Some("/p/atlas"), Some("/p/other"), false),
        (Some("/p/atlas"), None, false),
        (None, None, false),
        (Some(""), Some(""), false),
    ];
    for (caller, target, allowed) in table {
        assert_eq!(may_read(caller, target), allowed, "{caller:?} {target:?}");
    }
    assert!(may_control(Some("claude#a"), "claude#a"));
    assert!(!may_control(Some("claude#a"), "claude#b"));
    assert!(!may_control(None, "claude#a"));
}

#[test]
fn status_follows_the_run_then_the_journal_then_the_outcome() {
    let table = [
        (StatusFacts::default(), TeamStatus::Done, None),
        (
            StatusFacts {
                run_open: true,
                ..StatusFacts::default()
            },
            TeamStatus::Running,
            None,
        ),
        (
            StatusFacts {
                run_open: true,
                waiting: Some(Needs::Question),
                ..StatusFacts::default()
            },
            TeamStatus::NeedsYou,
            Some(Needs::Question),
        ),
        (
            // A waiting flag left from an ended run says nothing.
            StatusFacts {
                waiting: Some(Needs::Approval),
                outcome: Some(RunOutcome::Completed),
                ..StatusFacts::default()
            },
            TeamStatus::Done,
            None,
        ),
        (
            // The tap saw the end; the journal has not written it yet.
            StatusFacts {
                journal_open: true,
                outcome: Some(RunOutcome::Completed),
                ..StatusFacts::default()
            },
            TeamStatus::Running,
            None,
        ),
        (
            StatusFacts {
                outcome: Some(RunOutcome::Failed),
                ..StatusFacts::default()
            },
            TeamStatus::Failed,
            None,
        ),
        (
            StatusFacts {
                outcome: Some(RunOutcome::Interrupted),
                ..StatusFacts::default()
            },
            TeamStatus::Stopped,
            None,
        ),
    ];
    for (facts, expected, needs) in table {
        assert_eq!(status(facts), (expected, needs), "{facts:?}");
    }
    assert!(!TeamStatus::Running.settles_wait());
    assert!(TeamStatus::NeedsYou.settles_wait() && TeamStatus::Done.settles_wait());
    assert_eq!(TeamStatus::NeedsYou.as_str(), "needsYou");
}

#[test]
fn a_notice_is_sent_once_when_the_caller_is_idle_and_never_when_it_has_the_result() {
    let base = NoticeFacts {
        notify: true,
        outcome: Some(RunOutcome::Completed),
        ..NoticeFacts::default()
    };
    let table = [
        (base, NoticeDecision::Send),
        (
            NoticeFacts {
                outcome: Some(RunOutcome::Failed),
                ..base
            },
            NoticeDecision::Send,
        ),
        (
            NoticeFacts {
                caller_running: true,
                ..base
            },
            NoticeDecision::Wait,
        ),
        (
            NoticeFacts {
                caller_settled: true,
                caller_running: true,
                ..base
            },
            NoticeDecision::Drop,
        ),
        (
            NoticeFacts {
                caller_archived: true,
                ..base
            },
            NoticeDecision::Drop,
        ),
        (
            NoticeFacts {
                caller_away: true,
                ..base
            },
            NoticeDecision::Drop,
        ),
        (
            NoticeFacts {
                caller_pane: true,
                ..base
            },
            NoticeDecision::Drop,
        ),
        (
            NoticeFacts {
                handled: true,
                ..base
            },
            NoticeDecision::None,
        ),
        (
            NoticeFacts {
                notify: false,
                ..base
            },
            NoticeDecision::None,
        ),
        (
            NoticeFacts {
                outcome: Some(RunOutcome::Interrupted),
                ..base
            },
            NoticeDecision::None,
        ),
        (
            NoticeFacts {
                outcome: None,
                ..base
            },
            NoticeDecision::None,
        ),
    ];
    for (facts, decision) in table {
        assert_eq!(notice_decision(facts), decision, "{facts:?}");
    }
}

#[test]
fn notice_wording_quotes_the_answer_or_the_error() {
    assert_eq!(
        notice_text(
            "Fix the tests",
            "Claude",
            RunOutcome::Completed,
            Some(" All green. "),
            None
        ),
        "[Interface] The thread \"Fix the tests\" (Claude) you started finished: All green."
    );
    let long = "x".repeat(NOTICE_ANSWER_CHARS + 5);
    let text = notice_text("T", "Codex", RunOutcome::Completed, Some(&long), None);
    assert_eq!(
        text,
        format!(
            "[Interface] The thread \"T\" (Codex) you started finished: {}… Call read_thread for the rest.",
            "x".repeat(NOTICE_ANSWER_CHARS)
        )
    );
    assert_eq!(
        notice_text(
            "T",
            "Grok",
            RunOutcome::Failed,
            Some("partial"),
            Some("Not logged in")
        ),
        "[Interface] The thread \"T\" (Grok) you started failed: Not logged in"
    );
    assert_eq!(
        notice_text("T", "Grok", RunOutcome::Failed, None, None),
        "[Interface] The thread \"T\" (Grok) you started failed: its run ended with an error"
    );
    assert_eq!(
        notice_text("T", "Claude", RunOutcome::Completed, None, None),
        "[Interface] The thread \"T\" (Claude) you started finished: (no answer)"
    );
}

#[test]
fn titles_and_cuts_respect_character_boundaries() {
    assert_eq!(
        title_from_task("\n  append child.txt   hello\nmore"),
        "append child.txt hello"
    );
    assert_eq!(title_from_task("   "), "Started thread");
    let long = "é".repeat(TITLE_FROM_TASK_CHARS + 3);
    assert_eq!(
        title_from_task(&long),
        format!("{}…", "é".repeat(TITLE_FROM_TASK_CHARS))
    );
    assert_eq!(cut_bytes("aé", 2), ("a", true));
    assert_eq!(cut_bytes("aé", 3), ("aé", false));
    assert_eq!(cut_chars("abc", 2), ("ab", true));
}

#[test]
fn receipts_replay_the_first_result_and_never_retry_an_unknown_one() {
    let digest = call_digest("start_thread", &json!({"task": "a", "requestId": "r1"}));
    assert_ne!(
        digest,
        call_digest("start_thread", &json!({"task": "b", "requestId": "r1"}))
    );
    assert_eq!(receipt_decision(None, &digest), ReceiptDecision::Fresh);
    let mut receipt = Receipt {
        caller: "claude#a".into(),
        request_id: "r1".into(),
        tool: "start_thread".into(),
        digest: digest.clone(),
        at: 1,
        result: None,
    };
    assert_eq!(
        receipt_decision(Some(&receipt), &digest),
        ReceiptDecision::Unknown
    );
    assert_eq!(
        receipt_decision(Some(&receipt), "other"),
        ReceiptDecision::Reused
    );
    receipt.result = Some("{\"thread\":\"claude#b\"}".into());
    assert_eq!(
        receipt_decision(Some(&receipt), &digest),
        ReceiptDecision::Replay("{\"thread\":\"claude#b\"}".into())
    );
}

fn private_temp() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        temp.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    temp
}

fn started(parent: &str, child: &str) -> StartedThread {
    StartedThread {
        parent: parent.into(),
        child: child.into(),
        started_at: 10,
        notify: true,
        delivered: None,
        started_by: None,
    }
}

#[test]
fn the_ledger_and_receipts_survive_reopening_and_stay_bounded() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    let mut store = ThreadTeamStore::open(&file).unwrap();
    store.record_start(started("claude#a", "codex#b")).unwrap();
    store.record_start(started("claude#a", "claude#c")).unwrap();
    assert!(store.record_start(started("claude#x", "codex#b")).is_err());
    assert!(store.record_start(started("claude#a", "claude#a")).is_err());
    store
        .mark_delivered("codex#b", 2, DeliveryHow::Read)
        .unwrap();
    // An older turn never replaces a newer one.
    store
        .mark_delivered("codex#b", 1, DeliveryHow::Sent)
        .unwrap();
    store
        .reserve("claude#a", "r1", "start_thread", "d1", 5)
        .unwrap();
    assert!(
        store
            .reserve("claude#a", "r1", "start_thread", "d1", 6)
            .is_err()
    );
    store.settle("claude#a", "r1", "{\"ok\":true}").unwrap();
    store
        .reserve("claude#a", "r2", "send_to_thread", "d2", 7)
        .unwrap();
    drop(store);

    let store = ThreadTeamStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    assert_eq!(store.len(), 2);
    assert_eq!(store.children("claude#a").len(), 2);
    assert_eq!(
        store.started("codex#b").unwrap().delivered,
        Some(Delivered {
            turn: 2,
            how: DeliveryHow::Read
        })
    );
    assert_eq!(
        store.receipt("claude#a", "r1").unwrap().result.as_deref(),
        Some("{\"ok\":true}")
    );
    assert_eq!(store.receipt("claude#a", "r2").unwrap().result, None);
    // Past the bound the oldest receipts go. The file is filled to the bound
    // directly (one write, not a thousand), then two reservations push out
    // the two oldest.
    drop(store);
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let receipts = document["receipts"].as_array_mut().unwrap();
    for index in 0..MAX_RECEIPTS - receipts.len() {
        receipts.push(json!({
            "caller": "claude#z",
            "requestId": format!("bulk-{index}"),
            "tool": "send_to_thread",
            "digest": "d",
            "at": 100 + index as u64,
        }));
    }
    std::fs::write(&file, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut store = ThreadTeamStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    assert!(store.receipt("claude#a", "r1").is_some());
    for (index, at) in [(0_u64, 10_000_u64), (1, 10_001)] {
        store
            .reserve(
                "claude#z",
                &format!("over-{index}"),
                "send_to_thread",
                "d",
                at,
            )
            .unwrap();
    }
    assert!(store.receipt("claude#a", "r1").is_none());
    assert!(store.receipt("claude#a", "r2").is_none());
    assert!(store.receipt("claude#z", "bulk-0").is_some());
    // A result over its bound stays unknown.
    store
        .settle("claude#z", "bulk-1", &"x".repeat(MAX_RESULT_BYTES + 1))
        .unwrap();
    assert_eq!(store.receipt("claude#z", "bulk-1").unwrap().result, None);
    // Forgetting a session drops what it started and its receipts.
    store.forget("claude#a").unwrap();
    assert!(store.is_empty());
}

#[test]
fn an_unreadable_ledger_is_quarantined_and_the_store_starts_empty() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    for bytes in [
        b"{".to_vec(),
        br#"{"version":3,"started":[],"receipts":[]}"#.to_vec(),
        br#"{"version":2,"started":[{"parent":"outside:ab","child":"claude#c","startedAt":1,"notify":false}],"receipts":[]}"#.to_vec(),
        br#"{"version":2,"started":[{"parent":"claude#a","child":"claude#c","startedAt":1,"notify":false,"startedBy":{"kind":"outside","client":"x"}}],"receipts":[]}"#.to_vec(),
        br#"{"version":1,"started":[{"parent":"a","child":"a","startedAt":1,"notify":true}],"receipts":[]}"#.to_vec(),
        br#"{"version":1,"started":[],"receipts":[],"extra":1}"#.to_vec(),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        let store = ThreadTeamStore::open(&file).unwrap();
        assert!(store.quarantined_artifact().is_some(), "{}", String::from_utf8_lossy(&bytes));
        assert!(store.is_empty());
        drop(store);
        let _ = std::fs::remove_file(&file);
    }
}

#[test]
fn the_ledger_refuses_past_its_bound() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    let mut store = ThreadTeamStore::open(&file).unwrap();
    // Built in memory past the commit path: the bound is the validator's.
    let mut state = State::default();
    for index in 0..MAX_STARTED {
        state
            .started
            .push(started("claude#p", &format!("claude#c{index}")));
    }
    assert!(valid_state(&state));
    state.started.push(started("claude#p", "claude#over"));
    assert!(!valid_state(&state));
    state.started.pop();
    store.commit(state).unwrap();
    assert!(
        store
            .record_start(started("claude#p", "claude#over"))
            .is_err()
    );
}

#[test]
fn a_thread_reads_as_compact_items_within_the_page_bound() {
    use interface_contracts::thread_journal::{JournalEntry, JournalLine, TurnOrigin};
    let line = |seq: u64, entry: JournalEntry| JournalLine {
        seq,
        at: seq,
        turn: Some(0),
        entry,
    };
    let lines = vec![
        line(
            1,
            JournalEntry::TurnStarted {
                origin: TurnOrigin::Host,
                prompt: "append child.txt hello".into(),
                digest: "d".into(),
                cut: false,
            },
        ),
        line(
            2,
            JournalEntry::ToolCall {
                call_id: "c".into(),
                tool: "Edit".into(),
                summary: "child.txt\n  more".into(),
                input: json!({}),
                agent_key: None,
                cut: false,
            },
        ),
        line(
            3,
            JournalEntry::Diff {
                call_id: "c".into(),
                path: "child.txt".into(),
                before: Some("a\nb".into()),
                after: Some("a\nb\nc".into()),
                agent_key: None,
                cut: false,
            },
        ),
        line(
            4,
            JournalEntry::ToolResult {
                call_id: "c".into(),
                ok: false,
                summary: "denied".into(),
                agent_key: None,
            },
        ),
        line(
            5,
            JournalEntry::Message {
                text: "Done.\nTwo lines.".into(),
                agent_key: None,
                cut: false,
            },
        ),
        line(
            6,
            JournalEntry::TurnEnded {
                outcome: RunOutcome::Completed,
                reason: None,
            },
        ),
    ];
    let (items, next) = compact_page(&lines, false);
    assert_eq!(next, None);
    assert_eq!(
        items,
        vec![
            json!({"seq":1,"turn":0,"kind":"prompt","text":"(from Interface) append child.txt hello"}),
            json!({"seq":2,"turn":0,"kind":"toolCall","text":"Edit: child.txt more"}),
            json!({"seq":3,"turn":0,"kind":"diff","text":"child.txt (2 → 3 lines)"}),
            json!({"seq":4,"turn":0,"kind":"toolResult","text":"failed: denied"}),
            json!({"seq":5,"turn":0,"kind":"answer","text":"Done.\nTwo lines."}),
            json!({"seq":6,"turn":0,"kind":"turnEnded","text":"completed"}),
        ]
    );
    // Long answers fill the page; the rest comes with `next`.
    let long = (1..=10)
        .map(|seq| {
            line(
                seq,
                JournalEntry::Message {
                    text: "y".repeat(READ_TEXT_CHARS + 50),
                    agent_key: None,
                    cut: false,
                },
            )
        })
        .collect::<Vec<_>>();
    let (items, next) = compact_page(&long, false);
    assert_eq!(items.len(), READ_PAGE_CHARS / (READ_TEXT_CHARS + 1));
    assert_eq!(next, Some(items.len() as u64));
    let (items, next) = compact_page(&long[..1], true);
    assert_eq!(
        (items.len(), next),
        (1, Some(1)),
        "more follow in the journal"
    );
}

// --- Outside agents (T3 section 11) ---------------------------------------------

fn outside_started(session: &str, child: &str, at: u64) -> StartedThread {
    StartedThread {
        parent: outside_caller(session),
        child: child.into(),
        started_at: at,
        notify: false,
        delivered: None,
        started_by: Some(StartedBy {
            kind: StartedByKind::Outside,
            client: "claude-code".into(),
        }),
    }
}

/// Off refuses everything, Read reaches only the reading tools, Full every
/// tool; nothing else is a tool. The level is the owner's, never the input's.
#[test]
fn outside_levels_gate_each_tool() {
    for tool in OUTSIDE_READ_TOOLS.iter().chain(OUTSIDE_FULL_TOOLS.iter()) {
        assert_eq!(
            outside_may_call(OutsideAgents::Off, tool).unwrap_err().code,
            OUTSIDE_OFF,
            "{tool}"
        );
        assert!(
            outside_may_call(OutsideAgents::Full, tool).is_ok(),
            "{tool}"
        );
    }
    for tool in OUTSIDE_READ_TOOLS {
        assert!(
            outside_may_call(OutsideAgents::Read, tool).is_ok(),
            "{tool}"
        );
    }
    for tool in OUTSIDE_FULL_TOOLS {
        assert_eq!(
            outside_may_call(OutsideAgents::Read, tool)
                .unwrap_err()
                .code,
            READ_ONLY,
            "{tool}"
        );
    }
    // The calling thread's own tools are never an outside session's.
    for tool in [
        "team_info",
        "show_visual",
        "schedule_prompt",
        "list_schedules",
        "cancel_schedule",
        "request_secret",
        "",
    ] {
        assert_eq!(
            outside_may_call(OutsideAgents::Full, tool)
                .unwrap_err()
                .code,
            INVALID_INPUT,
            "{tool}"
        );
    }
}

/// An outside session's limits: 4 running, 16 in all, 64 a day across
/// sessions, and the ledger's bound; the day's limit is checked first.
#[test]
fn outside_start_limits_refuse_with_the_reason() {
    assert!(check_outside_start_limits(3, 15, 63, MAX_STARTED - 1).is_ok());
    assert_eq!(
        check_outside_start_limits(0, 0, MAX_OUTSIDE_STARTED_PER_DAY, 0)
            .unwrap_err()
            .code,
        LIMIT_DAY
    );
    assert_eq!(
        check_outside_start_limits(0, MAX_OUTSIDE_STARTED_PER_SESSION, 0, 0)
            .unwrap_err()
            .code,
        LIMIT_TOTAL
    );
    assert_eq!(
        check_outside_start_limits(MAX_OUTSIDE_RUNNING, 4, 4, 0)
            .unwrap_err()
            .code,
        LIMIT_RUNNING
    );
    assert_eq!(
        check_outside_start_limits(0, 0, 0, MAX_STARTED)
            .unwrap_err()
            .code,
        TEAM_FULL
    );
    assert_eq!(
        (
            MAX_OUTSIDE_RUNNING,
            MAX_OUTSIDE_STARTED_PER_SESSION,
            MAX_OUTSIDE_STARTED_PER_DAY
        ),
        (4, 16, 64)
    );
}

/// A start names an existing project only: its exact root or a name one
/// project has; any other path, even one inside a project, is refused.
#[test]
fn an_outside_start_names_a_known_project_only() {
    let known = vec![
        "/Users/fixture/Developer/atlas".to_owned(),
        "/Users/fixture/Developer/beacon".to_owned(),
        "/Users/fixture/Work/beacon".to_owned(),
    ];
    assert_eq!(
        resolve_project(&known, "/Users/fixture/Developer/atlas").unwrap(),
        known[0]
    );
    assert_eq!(resolve_project(&known, "atlas").unwrap(), known[0]);
    assert_eq!(
        resolve_project(&known, "/Users/fixture/Work/beacon").unwrap(),
        known[2]
    );
    for wanted in [
        "beacon",
        "/Users/fixture/Developer/atlas/src",
        "/Users/fixture/Developer/atlas/",
        "/tmp",
        "../atlas",
        "",
    ] {
        let refused = resolve_project(&known, wanted).unwrap_err();
        assert_eq!(refused.code, UNKNOWN_PROJECT, "{wanted}");
    }
    assert!(
        resolve_project(&[], "atlas")
            .unwrap_err()
            .message
            .contains("no projects")
    );
    assert_eq!(project_name("/Users/fixture/Developer/atlas"), "atlas");
}

/// A client's declared name is display text: bounded, one line, never empty.
#[test]
fn an_outside_clients_name_is_bounded_and_one_line() {
    assert_eq!(client_name(Some("claude-code")), "claude-code");
    assert_eq!(
        client_name(Some("  Codex\n\tCLI\u{1b}[31m ")),
        "Codex CLI [31m"
    );
    assert_eq!(client_name(None), UNNAMED_CLIENT);
    assert_eq!(client_name(Some("\u{7}\n ")), UNNAMED_CLIENT);
    let long = "x".repeat(500);
    assert_eq!(
        client_name(Some(&long)).chars().count(),
        MAX_CLIENT_NAME_CHARS
    );
    assert!(is_outside_caller(&outside_caller("0123456789ab")));
    assert!(!is_outside_caller("claude#a"));
}

/// The socket sits under the data root, or under a private temporary
/// directory named for the uid and the data root when that path is too long.
#[test]
fn the_outside_socket_path_fits_a_unix_socket() {
    let short = Path::new("/Users/fixture/Library/Application Support/Interface");
    assert_eq!(
        outside_socket_path(short, 501),
        short.join("team").join("outside.sock")
    );
    let long = PathBuf::from(format!("/Users/fixture/{}", "deep/".repeat(30)));
    let fallback = outside_socket_path(&long, 501);
    assert!(fallback.as_os_str().len() <= MAX_SOCKET_PATH_BYTES);
    assert!(
        fallback
            .to_string_lossy()
            .starts_with("/private/tmp/iface-out-501-"),
        "{}",
        fallback.display()
    );
    assert_eq!(fallback, outside_socket_path(&long, 501), "stable");
    assert_ne!(
        fallback,
        outside_socket_path(&long.join("other"), 501),
        "one per data root"
    );
}

/// The level and outside starts survive reopening; a thread an outside agent
/// started names its client; the day's count reads the start times.
#[test]
fn the_outside_level_and_starts_survive_reopening() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    let mut store = ThreadTeamStore::open(&file).unwrap();
    assert_eq!(store.outside_agents(), OutsideAgents::Off);
    store.set_outside_agents(OutsideAgents::Full).unwrap();
    store
        .record_start(outside_started("0123456789ab", "claude#o1", 1_000))
        .unwrap();
    store
        .record_start(outside_started("0123456789ab", "codex#o2", 5_000))
        .unwrap();
    store.record_start(started("claude#a", "claude#t")).unwrap();
    // An outside start must say who started it, and only it may.
    let mut unnamed = outside_started("ffffffffffff", "claude#o3", 1);
    unnamed.started_by = None;
    assert!(store.record_start(unnamed).is_err());
    let mut claimed = started("claude#a", "claude#o4");
    claimed.started_by = outside_started("x", "y", 0).started_by;
    assert!(store.record_start(claimed).is_err());
    drop(store);

    let store = ThreadTeamStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    assert_eq!(store.outside_agents(), OutsideAgents::Full);
    assert_eq!(
        store.outside_started().collect::<Vec<_>>(),
        [("claude#o1", "claude-code"), ("codex#o2", "claude-code")]
    );
    assert_eq!(store.outside_started_since(0), 2);
    assert_eq!(store.outside_started_since(2_000), 1);
    assert_eq!(store.children(&outside_caller("0123456789ab")).len(), 2);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written["version"], 2);
    assert_eq!(written["outsideAgents"], "full");
}

/// The version 1 ledger is read once, carried over as version 2 with outside
/// agents off, and left exactly as it was; an unreadable one is left alone.
#[test]
fn a_v1_ledger_is_carried_over_once_and_left_unchanged() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    let v1 = temp.path().join(THREAD_TEAM_V1_FILE_NAME);
    let bytes = br#"{"version":1,"started":[{"parent":"claude#a","child":"codex#b","startedAt":7,"notify":true,"delivered":{"turn":1,"how":"read"}}],"receipts":[{"caller":"claude#a","requestId":"r1","tool":"start_thread","digest":"d","at":5,"result":"{}"}]}"#;
    std::fs::write(&v1, bytes).unwrap();
    std::fs::set_permissions(&v1, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let mut store = ThreadTeamStore::open(&file).unwrap();
    assert_eq!(store.len(), 1);
    assert_eq!(store.started("codex#b").unwrap().started_at, 7);
    assert!(store.receipt("claude#a", "r1").is_some());
    assert_eq!(store.outside_agents(), OutsideAgents::Off);
    assert!(file.exists(), "carried over at once");
    // Later changes go to version 2 only.
    store.forget("claude#a").unwrap();
    drop(store);
    assert_eq!(std::fs::read(&v1).unwrap(), bytes);
    let store = ThreadTeamStore::open(&file).unwrap();
    assert!(store.is_empty(), "version 1 is never read again");
    drop(store);

    // An unreadable version 1 file is left for the build that wrote it.
    let temp = private_temp();
    let file = temp.path().join(THREAD_TEAM_FILE_NAME);
    let v1 = temp.path().join(THREAD_TEAM_V1_FILE_NAME);
    std::fs::write(&v1, b"{").unwrap();
    std::fs::set_permissions(&v1, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let store = ThreadTeamStore::open(&file).unwrap();
    assert!(store.is_empty() && store.quarantined_artifact().is_none());
    assert_eq!(std::fs::read(&v1).unwrap(), b"{");
}
