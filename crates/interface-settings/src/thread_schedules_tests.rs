use interface_contracts::threads::DiffSide;
use serde_json::json;

use super::*;

const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const NOW: u64 = 1_791_300_000_000;

fn millis(value: &str) -> u64 {
    u64::try_from(
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis(),
    )
    .unwrap()
}

fn calendar(hour: u8, minute: u8, weekdays: Vec<u8>) -> ScheduleWhen {
    ScheduleWhen::Calendar {
        calendar: ScheduleCalendar {
            timezone: "America/Chicago".into(),
            weekdays,
            hour,
            minute,
        },
    }
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

fn new(id: &str, key: &str, when: ScheduleWhen) -> NewSchedule {
    NewSchedule {
        id: id.into(),
        thread_key: key.into(),
        name: None,
        prompt: format!("prompt {id}"),
        when,
        set_by: ScheduleSetBy::Owner,
        kind: ScheduleKind::Prompt,
        receipt: None,
    }
}

/// When each kind of schedule is due, and what comes after: a one-time
/// schedule fires at its time; an interval keeps its grid when on time and
/// fires once after a long sleep, then goes on from now (never a burst); a
/// calendar fires its latest date and goes on at its next one.
#[test]
fn the_due_time_table_fires_each_kind_once_and_never_bursts() {
    let interval = ScheduleWhen::Interval { minutes: 60 };
    let table: [(&ScheduleWhen, u64, u64, Due); 7] = [
        (&ScheduleWhen::Once { at: NOW }, NOW, NOW - 1, Due::NotYet),
        (
            &ScheduleWhen::Once { at: NOW },
            NOW,
            NOW + 5_000,
            Due::Fire {
                occurrence: NOW,
                next: None,
            },
        ),
        (&interval, NOW, NOW - 1, Due::NotYet),
        // On time (a few seconds late): the grid holds.
        (
            &interval,
            NOW,
            NOW + 5_000,
            Due::Fire {
                occurrence: NOW,
                next: Some(NOW + HOUR),
            },
        ),
        // Asleep for five hours: one firing, at the latest due time, then
        // an hour from now.
        (
            &interval,
            NOW,
            NOW + 5 * HOUR + 10 * MINUTE,
            Due::Fire {
                occurrence: NOW + 5 * HOUR,
                next: Some(NOW + 6 * HOUR + 10 * MINUTE),
            },
        ),
        // Asleep for three days: still one firing (its latest slot is
        // minutes old), never seventy-two.
        (
            &interval,
            NOW,
            NOW + 3 * DAY + 30 * MINUTE,
            Due::Fire {
                occurrence: NOW + 3 * DAY,
                next: Some(NOW + 3 * DAY + 90 * MINUTE),
            },
        ),
        // A one-time schedule more than a day late is skipped.
        (
            &ScheduleWhen::Once { at: NOW },
            NOW,
            NOW + DAY + 1,
            Due::Missed {
                occurrence: NOW,
                next: None,
            },
        ),
    ];
    for (when, next_at, now, expected) in table {
        assert_eq!(
            due(when, next_at, now),
            expected,
            "{when:?} at +{}",
            now.saturating_sub(next_at)
        );
    }
}

/// A calendar fires its latest missed date once (within a day) and goes on
/// at its first date after now; over the spring-forward gap it keeps the
/// local time (23 hours apart) and over the fall-back repeat it fires once.
#[test]
fn a_calendar_schedule_fires_its_latest_date_and_keeps_local_time_across_dst() {
    let daily = calendar(9, 0, vec![]);
    let first = millis("2026-03-06T15:00:00Z"); // 09:00 CST
    assert_eq!(first_due(&daily, first - 1).unwrap(), first);
    // On time on the Friday before spring forward.
    assert_eq!(
        due(&daily, first, first + 1_000),
        Due::Fire {
            occurrence: first,
            next: Some(millis("2026-03-07T15:00:00Z"))
        }
    );
    // Saturday's run, then Sunday's is an hour earlier in UTC (CDT).
    let saturday = millis("2026-03-07T15:00:00Z");
    assert_eq!(
        due(&daily, saturday, saturday),
        Due::Fire {
            occurrence: saturday,
            next: Some(millis("2026-03-08T14:00:00Z"))
        }
    );
    // Asleep from Friday's run until Monday 10:00: Monday's 09:00 fires
    // once (an hour late), Friday's to Sunday's never do.
    let monday = millis("2026-03-09T14:00:00Z");
    assert_eq!(
        due(&daily, first, monday + HOUR),
        Due::Fire {
            occurrence: monday,
            next: Some(millis("2026-03-10T14:00:00Z"))
        }
    );
    // A weekly Monday schedule asleep until Wednesday: two days late, skipped.
    let mondays = calendar(9, 0, vec![1]);
    assert_eq!(
        due(&mondays, monday, monday + 2 * DAY),
        Due::Missed {
            occurrence: monday,
            next: Some(millis("2026-03-16T14:00:00Z"))
        }
    );
    // Fall back: 01:30 happens twice on 1 November; it fires once.
    let early = calendar(1, 30, vec![]);
    let repeat = millis("2026-11-01T06:30:00Z");
    assert_eq!(
        due(&early, repeat, repeat + 2 * HOUR),
        Due::Fire {
            occurrence: repeat,
            next: Some(millis("2026-11-02T07:30:00Z"))
        }
    );
}

/// The missed-run policy: a firing up to a day late runs once; more than a
/// day late it is skipped and recorded, and the schedule goes on.
#[test]
fn a_firing_more_than_a_day_late_is_skipped_and_recorded() {
    let temp = private_temp();
    let mut store =
        ThreadScheduleStore::open(&temp.path().join(THREAD_SCHEDULES_FILE_NAME)).unwrap();
    store
        .add(
            new(
                "late",
                "claude#a",
                ScheduleWhen::Interval {
                    minutes: 2 * 24 * 60,
                },
            ),
            NOW,
            NOW - DAY,
        )
        .unwrap();
    store
        .add(
            new("soon", "claude#a", ScheduleWhen::Once { at: NOW }),
            NOW,
            NOW - DAY,
        )
        .unwrap();
    let advanced = store.advance(NOW + DAY + MINUTE, |_| false).unwrap();
    assert_eq!(
        advanced,
        [
            Advanced::Skipped {
                id: "late".into(),
                occurrence: NOW
            },
            Advanced::Skipped {
                id: "soon".into(),
                occurrence: NOW
            },
        ]
    );
    let late = store.schedule("late").unwrap();
    assert!(late.enabled);
    assert_eq!(late.next_at, Some(NOW + 2 * DAY));
    let run = late.last_run.as_ref().unwrap();
    assert_eq!(run.outcome, ScheduleOutcome::Skipped);
    assert!(run.detail.as_deref().unwrap().contains("more than a day"));
    // A one-time schedule that was missed is over.
    let soon = store.schedule("soon").unwrap();
    assert!(!soon.enabled);
    assert_eq!(soon.next_at, None);
    // Nothing is due again until its next time.
    assert!(
        store
            .advance(NOW + DAY + 2 * MINUTE, |_| false)
            .unwrap()
            .is_empty()
    );
}

/// Each firing is delivered at most once, ever: it waits, its attempt is
/// recorded before the send (and the firing stops waiting), a second
/// attempt of the same occurrence finds nothing, and a reopened store has
/// nothing waiting and nothing due. An unknown outcome stays unknown.
#[test]
fn an_occurrence_is_attempted_once_and_a_reopened_store_never_offers_it_again() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_SCHEDULES_FILE_NAME);
    let mut store = ThreadScheduleStore::open(&file).unwrap();
    store
        .add(
            new("s-1", "claude#a", ScheduleWhen::Interval { minutes: 30 }),
            NOW,
            NOW - 30 * MINUTE,
        )
        .unwrap();
    assert_eq!(occurrence_id("s-1", NOW), format!("scheduled:s-1:{NOW}"));
    assert_eq!(
        store.advance(NOW, |_| false).unwrap(),
        [Advanced::Waiting {
            id: "s-1".into(),
            occurrence: NOW
        }]
    );
    // The next pass does not fire it again, even though it still waits.
    assert!(store.advance(NOW + 1_000, |_| false).unwrap().is_empty());
    let attempt = store.begin_attempt("s-1", NOW, NOW + 2_000).unwrap();
    assert_eq!(attempt.unwrap().prompt, "prompt s-1");
    let recorded = store.schedule("s-1").unwrap();
    assert_eq!(recorded.pending, None);
    assert_eq!(
        recorded.last_run.as_ref().unwrap().outcome,
        ScheduleOutcome::Unknown,
        "recorded before the send"
    );
    assert!(
        store
            .begin_attempt("s-1", NOW, NOW + 3_000)
            .unwrap()
            .is_none()
    );
    drop(store);

    let mut store = ThreadScheduleStore::open(&file).unwrap();
    assert!(!store.has_waiting());
    assert!(
        store
            .begin_attempt("s-1", NOW, NOW + 4_000)
            .unwrap()
            .is_none()
    );
    assert!(store.advance(NOW + 5_000, |_| false).unwrap().is_empty());
    assert_eq!(store.next_wake(), Some(NOW + 30 * MINUTE));
    assert_eq!(
        store
            .schedule("s-1")
            .unwrap()
            .last_run
            .as_ref()
            .unwrap()
            .outcome,
        ScheduleOutcome::Unknown
    );
    // A firing while one still waits is skipped and recorded, never queued.
    store.advance(NOW + 30 * MINUTE, |_| false).unwrap();
    assert_eq!(
        store.advance(NOW + 60 * MINUTE, |_| false).unwrap(),
        [Advanced::Skipped {
            id: "s-1".into(),
            occurrence: NOW + 60 * MINUTE
        }]
    );
    assert_eq!(
        store
            .schedule("s-1")
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .occurrence,
        NOW + 30 * MINUTE
    );
    // A trigger while one waits is refused; after it, a trigger's
    // occurrence is above the last one recorded, even with a clock behind.
    assert!(matches!(
        store.trigger("s-1", NOW + 61 * MINUTE),
        Err(ScheduleError::Refused(ScheduleRefusal {
            code: SCHEDULE_WAITING_CODE,
            ..
        }))
    ));
    store
        .record(
            "s-1",
            NOW + 30 * MINUTE,
            ScheduleOutcome::Refused,
            Some("An Away loop is running this thread."),
            NOW + 61 * MINUTE,
        )
        .unwrap();
    let triggered = store.trigger("s-1", NOW).unwrap();
    assert_eq!(triggered, NOW + 30 * MINUTE + 1);
}

#[test]
fn the_gate_delivers_to_an_idle_thread_waits_for_a_busy_one_and_drops_the_rest() {
    let idle = GateFacts {
        known: true,
        ..GateFacts::default()
    };
    assert_eq!(gate(idle), Gate::Deliver);
    assert_eq!(gate(GateFacts { busy: true, ..idle }), Gate::Wait);
    let table: [(GateFacts, ScheduleOutcome, &str); 8] = [
        (
            GateFacts::default(),
            ScheduleOutcome::Refused,
            "no longer in Threads",
        ),
        (
            GateFacts {
                archived: true,
                ..idle
            },
            ScheduleOutcome::Refused,
            "archived",
        ),
        (
            GateFacts {
                settled: true,
                busy: true,
                ..idle
            },
            ScheduleOutcome::Skipped,
            "settled",
        ),
        (
            GateFacts {
                away: true,
                busy: true,
                ..idle
            },
            ScheduleOutcome::Refused,
            "Away loop",
        ),
        (
            GateFacts { pane: true, ..idle },
            ScheduleOutcome::Refused,
            "Code pane",
        ),
        (
            GateFacts {
                no_runtime: true,
                ..idle
            },
            ScheduleOutcome::Refused,
            "could not send",
        ),
        (
            GateFacts {
                busy: true,
                waited_ms: LATE_LIMIT_MS + 1,
                ..idle
            },
            ScheduleOutcome::Skipped,
            "more than a day",
        ),
        (
            GateFacts {
                away: true,
                pane: true,
                ..idle
            },
            ScheduleOutcome::Refused,
            "Away loop",
        ),
    ];
    for (facts, outcome, said) in table {
        match gate(facts) {
            Gate::Drop {
                outcome: got,
                detail,
            } => {
                assert_eq!(got, outcome, "{facts:?}");
                assert!(detail.contains(said), "{facts:?}: {detail}");
            }
            other => panic!("{facts:?}: {other:?}"),
        }
    }
}

#[test]
fn validation_refuses_the_past_short_intervals_bad_calendars_and_big_prompts() {
    let ok = |when: &ScheduleWhen| validate("Check the build", Some("Build"), when, NOW);
    assert_eq!(ok(&ScheduleWhen::Once { at: NOW + 1 }), Ok(NOW + 1));
    assert_eq!(
        ok(&ScheduleWhen::Interval { minutes: 5 }),
        Ok(NOW + 5 * MINUTE)
    );
    assert!(ok(&calendar(7, 30, vec![1, 5])).is_ok());
    for when in [
        ScheduleWhen::Once { at: NOW },
        ScheduleWhen::Interval { minutes: 4 },
        ScheduleWhen::Interval {
            minutes: MAX_INTERVAL_MINUTES + 1,
        },
        calendar(24, 0, vec![]),
        calendar(9, 0, vec![8]),
        ScheduleWhen::Calendar {
            calendar: ScheduleCalendar {
                timezone: "Not/AZone".into(),
                weekdays: vec![],
                hour: 9,
                minute: 0,
            },
        },
    ] {
        assert_eq!(
            ok(&when).unwrap_err().code,
            SCHEDULE_INVALID_CODE,
            "{when:?}"
        );
    }
    let once = ScheduleWhen::Once { at: NOW + 1 };
    assert!(validate(" ", None, &once, NOW).is_err());
    assert!(validate(&"x".repeat(MAX_PROMPT_BYTES + 1), None, &once, NOW).is_err());
    assert!(validate("p", Some("two\nlines"), &once, NOW).is_err());
    assert!(validate("p", Some(&"n".repeat(MAX_NAME_CHARS + 1)), &once, NOW).is_err());
    assert!(validate(&"x".repeat(MAX_PROMPT_BYTES), None, &once, NOW).is_ok());
}

#[test]
fn a_scheduled_prompt_starts_with_one_line_naming_it() {
    assert_eq!(
        scheduled_prompt(Some("Nightly check"), "Check the build."),
        "[Interface] Scheduled prompt \"Nightly check\":\nCheck the build."
    );
    assert_eq!(
        scheduled_prompt(None, "\n  Look   at the queue again\nand say what is stuck"),
        "[Interface] Scheduled prompt \"Look at the queue again\":\n\n  Look   at the queue again\nand say what is stuck"
    );
    assert_eq!(
        schedule_label(
            None,
            "Read every open issue in the tracker and summarize them"
        ),
        "Read every open issue in the tracker and…"
    );
    assert!(scheduled_prompt(None, "x").starts_with(SCHEDULED_PREFIX));
    assert_eq!(detail("a\n  b"), "a b");
}

#[test]
fn a_watch_remembers_on_its_first_look_and_finds_only_new_failures_and_comments() {
    let check = check_id(
        Some("CI"),
        "test",
        Some("https://github.com/o/r/actions/runs/11/job/22"),
    );
    assert_eq!(check, "CI / test@11/22");
    assert_eq!(check_id(None, "lint", None), "lint");
    // First look, nothing failing: only remembered.
    let first = observe(
        None,
        [],
        ["https://github.com/o/r/pull/1#discussion_r1".to_owned()],
    );
    assert!(!first.prompts());
    // First look while a check fails: sent once.
    let failing = observe(None, [check.clone()], []);
    assert!(failing.new_failure);
    // The same failure again: nothing.
    let again = observe(Some(&failing.fingerprint), [check.clone()], []);
    assert!(!again.prompts());
    // A re-run that fails again is a new failure.
    let rerun = observe(
        Some(&again.fingerprint),
        [check_id(
            Some("CI"),
            "test",
            Some("https://github.com/o/r/actions/runs/12/job/23"),
        )],
        [],
    );
    assert!(rerun.new_failure);
    // A check that passes, then a new comment: only the comment is new.
    let comment = observe(
        Some(&rerun.fingerprint),
        [],
        ["u1".to_owned(), "u2".to_owned()],
    );
    assert!(!comment.new_failure);
    assert_eq!(comment.new_reviews, ["u1", "u2"]);
    let resolved = observe(Some(&comment.fingerprint), [], ["u2".to_owned()]);
    assert!(!resolved.prompts());
    assert_eq!(resolved.fingerprint.review_threads, ["u2"]);
}

fn review(path: &str, end: Option<u32>, comment: &str) -> ReviewComment {
    ReviewComment {
        path: path.into(),
        start_line: end.map(|end| end - 1),
        end_line: end,
        side: DiffSide::New,
        code: "+let a = 1;\n+let b = 2;".into(),
        comment: comment.into(),
        url: format!("https://github.com/o/r/pull/1#discussion_{path}"),
        outdated: false,
    }
}

/// The review text is what "Bring in review comments" puts in an empty
/// draft (`thread-ship.ts` `appendReviewComments`), and the watch prompt
/// joins it with the failing-checks prompt under one line.
#[test]
fn the_watch_prompt_carries_the_ship_bar_texts() {
    let mut outdated = review("src/b.rs", Some(4), "@ann: why?");
    outdated.outdated = true;
    outdated.side = DiffSide::Old;
    outdated.code = "x ``` y".into();
    let text = review_comments_text(&[
        review("src/a.rs", Some(8), "@bob: rename this"),
        review("README.md", None, "@cy: say more"),
        outdated,
    ]);
    assert_eq!(
        text,
        "src/a.rs:7-8\n```diff\n+let a = 1;\n+let b = 2;\n```\n@bob: rename this\n\nREADME.md\n@cy: say more\n\nsrc/b.rs:3-4 (removed)\n````diff\nx ``` y\n````\n@ann: why? (on lines that have changed since)\n\n"
    );
    let both = watch_prompt(
        42,
        "https://github.com/o/r/pull/42",
        Some("The checks are failing:\n- CI / test\n"),
        Some(&text),
    );
    assert!(both.starts_with(
        "[Interface] Watching pull request #42 (https://github.com/o/r/pull/42): checks are failing and there are new review comments.\n\nThe checks are failing:\n- CI / test\n\nNew review comments:\n\nsrc/a.rs:7-8"
    ));
    assert!(both.ends_with("(on lines that have changed since)\n\nAddress these review comments."));
    assert_eq!(
        watch_prompt(7, "u", Some("fix"), None),
        "[Interface] Watching pull request #7 (u): checks are failing.\n\nfix"
    );
}

#[test]
fn the_store_keeps_its_bounds_and_quarantines_what_it_cannot_read() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_SCHEDULES_FILE_NAME);
    // Filled to the bound in one write, not 256 commits.
    let schedules = (0..MAX_SCHEDULES)
        .map(|index| {
            json!({"id": format!("s-{index}"), "threadKey": "claude#a", "prompt": "p",
                "when": {"kind": "interval", "minutes": 60}, "enabled": true, "createdAt": 1})
        })
        .collect::<Vec<_>>();
    let watches = (0..MAX_WATCHES)
        .map(|index| json!({"threadKey": format!("claude#w{index}"), "kind": "pullRequest", "enabled": true}))
        .collect::<Vec<_>>();
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"version": 2, "schedules": schedules, "watches": watches}))
            .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let mut store = ThreadScheduleStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    assert_eq!(store.schedules().len(), MAX_SCHEDULES);
    assert!(matches!(
        store.add(
            new(
                "one-more",
                "claude#a",
                ScheduleWhen::Interval { minutes: 5 }
            ),
            NOW,
            NOW
        ),
        Err(ScheduleError::Refused(ScheduleRefusal {
            code: SCHEDULE_FULL_CODE,
            ..
        }))
    ));
    assert!(matches!(
        store.set_watch("claude#new", true),
        Err(ScheduleError::Refused(ScheduleRefusal {
            code: SCHEDULE_FULL_CODE,
            ..
        }))
    ));
    drop(store);
    for bytes in [
        b"{".to_vec(),
        br#"{"version":1,"schedules":[],"watches":[]}"#.to_vec(),
        br#"{"version":2,"schedules":[],"watches":[],"extra":1}"#.to_vec(),
        br#"{"version":2,"schedules":[{"id":"a b","threadKey":"k","prompt":"p","when":{"kind":"interval","minutes":5},"enabled":true,"createdAt":1}],"watches":[]}"#.to_vec(),
        br#"{"version":2,"schedules":[],"watches":[{"threadKey":"k","kind":"pullRequest","enabled":true},{"threadKey":"k","kind":"pullRequest","enabled":false}]}"#.to_vec(),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let store = ThreadScheduleStore::open(&file).unwrap();
        assert!(store.quarantined_artifact().is_some(), "{}", String::from_utf8_lossy(&bytes));
        assert!(store.schedules().is_empty() && store.watches().is_empty());
        drop(store);
        let _ = std::fs::remove_file(&file);
    }
}

/// Deleting, archiving or forgetting a thread removes its schedules and
/// watch; a switch moves them to the new head; settling pauses them (a
/// firing while settled is skipped, and the schedule goes on).
#[test]
fn schedules_go_with_their_thread_follow_its_head_and_pause_while_it_is_settled() {
    let temp = private_temp();
    let mut store =
        ThreadScheduleStore::open(&temp.path().join(THREAD_SCHEDULES_FILE_NAME)).unwrap();
    for (id, key) in [("s-a", "claude#a"), ("s-b", "claude#b")] {
        store
            .add(
                new(id, key, ScheduleWhen::Interval { minutes: 10 }),
                NOW,
                NOW - 10 * MINUTE,
            )
            .unwrap();
        store.set_watch(key, true).unwrap();
    }
    let advanced = store.advance(NOW, |key| key == "claude#a").unwrap();
    assert_eq!(
        advanced,
        [
            Advanced::Skipped {
                id: "s-a".into(),
                occurrence: NOW
            },
            Advanced::Waiting {
                id: "s-b".into(),
                occurrence: NOW
            },
        ]
    );
    let paused = store.view("claude#a", true).unwrap();
    assert!(paused.schedules[0].paused);
    assert!(
        paused.schedules[0]
            .last_run
            .as_ref()
            .unwrap()
            .detail
            .as_deref()
            .unwrap()
            .contains("settled")
    );
    assert_eq!(paused.schedules[0].next_at, Some(NOW + 10 * MINUTE));
    // Un-settled, it fires at its next time.
    assert_eq!(
        store.advance(NOW + 10 * MINUTE, |_| false).unwrap()[0],
        Advanced::Waiting {
            id: "s-a".into(),
            occurrence: NOW + 10 * MINUTE
        }
    );
    // A switch moves the thread's schedules and watch to the new head.
    store.follow("claude#b", "codex#b2").unwrap();
    assert!(store.view("claude#b", false).is_none());
    let followed = store.view("codex#b2", false).unwrap();
    assert_eq!(followed.schedules[0].id, "s-b");
    assert_eq!(followed.schedules[0].waiting, Some(NOW));
    assert!(followed.watch.unwrap().enabled);
    // Removing the thread removes both.
    store.remove_thread("claude#a").unwrap();
    assert!(store.view("claude#a", false).is_none());
    assert!(store.schedule("s-a").is_none() && store.watch("claude#a").is_none());
    assert!(store.schedule("s-b").is_some());
}

#[test]
fn a_watch_keeps_one_prompt_waiting_and_turns_off_when_told() {
    let temp = private_temp();
    let mut store =
        ThreadScheduleStore::open(&temp.path().join(THREAD_SCHEDULES_FILE_NAME)).unwrap();
    store.set_watch("claude#a", true).unwrap();
    let seen = observe(None, ["CI / test@1".to_owned()], []);
    assert!(
        store
            .observed(
                "claude#a",
                seen.fingerprint.clone(),
                Some("fix it".into()),
                NOW
            )
            .unwrap()
    );
    // While that prompt waits, a later look changes nothing.
    let later = observe(Some(&seen.fingerprint), ["CI / test@2".to_owned()], []);
    assert!(
        !store
            .observed("claude#a", later.fingerprint, Some("again".into()), NOW + 1)
            .unwrap()
    );
    assert_eq!(
        store.watch("claude#a").unwrap().fingerprint.as_ref(),
        Some(&seen.fingerprint)
    );
    let pending = store
        .begin_watch_attempt("claude#a", NOW + 2)
        .unwrap()
        .unwrap();
    assert_eq!(
        (pending.occurrence, pending.prompt.as_str()),
        (NOW, "fix it")
    );
    assert!(
        store
            .begin_watch_attempt("claude#a", NOW + 3)
            .unwrap()
            .is_none()
    );
    store
        .record_watch("claude#a", NOW, ScheduleOutcome::Sent, None, NOW + 4)
        .unwrap();
    let view = store.view("claude#a", false).unwrap().watch.unwrap();
    assert_eq!((view.failing_checks, view.waiting), (1, false));
    assert_eq!(view.last_run.unwrap().outcome, ScheduleOutcome::Sent);
    store.set_needs_gh("claude#a", true).unwrap();
    assert!(
        store
            .view("claude#a", false)
            .unwrap()
            .watch
            .unwrap()
            .needs_gh
    );
    store
        .watch_off("claude#a", "#42 was merged.", NOW + 5)
        .unwrap();
    let off = store.watch("claude#a").unwrap();
    assert!(!off.enabled);
    assert_eq!(
        off.last_run.as_ref().unwrap().detail.as_deref(),
        Some("#42 was merged.")
    );
    // Turning it on again starts from scratch.
    store.set_watch("claude#a", true).unwrap();
    assert!(store.watch("claude#a").unwrap().fingerprint.is_none());
}

// --- Resume at reset (T3 section 10) ------------------------------------------------

fn resume(id: &str, key: &str, at: u64, receipt: Option<&str>) -> NewSchedule {
    NewSchedule {
        id: id.into(),
        thread_key: key.into(),
        name: None,
        prompt: DEFAULT_RESUME_PROMPT.into(),
        when: ScheduleWhen::Once { at },
        set_by: ScheduleSetBy::Owner,
        kind: ScheduleKind::Resume,
        receipt: receipt.map(str::to_owned),
    }
}

fn receipt(delivery: PromptDelivery, text: &str, context: bool) -> PromptRecoveryReceipt {
    serde_json::from_value(json!({
        "id": "a".repeat(64),
        "key": "claude#a",
        "input": {
            "text": text,
            "context": if context {
                json!([{"id": "c1", "kind": "file", "label": "notes.md", "value": "/p/notes.md"}])
            } else {
                json!([])
            },
        },
        "serialized": text,
        "delivery": delivery,
        "at": NOW,
    }))
    .unwrap()
}

/// The reset a resume waits for: a notice's or a usage window's ISO time
/// in ms; from the windows, the latest reset among those at or over their
/// limit and still ahead (the provider takes prompts once every full
/// window has reset), none when no full window says; and the resume is
/// due a minute after it, never sooner than a minute from now.
#[test]
fn the_reset_comes_from_full_usage_windows_and_the_resume_is_a_minute_after_it() {
    assert_eq!(
        iso_millis("2026-11-15T21:10:00.000Z"),
        Some(millis("2026-11-15T21:10:00Z"))
    );
    assert_eq!(
        iso_millis("2026-11-15T21:10:00.25Z"),
        Some(millis("2026-11-15T21:10:00Z") + 250)
    );
    assert_eq!(
        iso_millis("2026-11-15T21:10:00Z"),
        Some(millis("2026-11-15T21:10:00Z"))
    );
    for bad in [
        "",
        "soon",
        "2026-11-15T21:10:00+01:00",
        "2026-13-15T21:10:00Z",
        "2026-11-15T25:10:00Z",
        "2026-11-15 21:10:00Z",
        "2026-11-15T21:10Z",
    ] {
        assert_eq!(iso_millis(bad), None, "{bad}");
    }
    let window = |label: &str, used: Option<f64>, resets: Option<&str>| UsageWindow {
        label: label.into(),
        utilization: used,
        resets_at: resets.map(str::to_owned),
    };
    let now = millis("2026-11-15T20:00:00Z");
    let windows = [
        window("5-hour", Some(100.0), Some("2026-11-15T21:10:00.000Z")),
        window("7-day", Some(64.0), Some("2026-11-18T09:00:00.000Z")),
    ];
    assert_eq!(
        usage_windows_reset(&windows, now).as_deref(),
        Some("2026-11-15T21:10:00.000Z")
    );
    let both = [
        window("5-hour", Some(100.0), Some("2026-11-15T21:10:00.000Z")),
        window("7-day", Some(101.0), Some("2026-11-18T09:00:00.000Z")),
        window("past", Some(100.0), Some("2026-11-15T19:00:00.000Z")),
    ];
    assert_eq!(
        usage_windows_reset(&both, now).as_deref(),
        Some("2026-11-18T09:00:00.000Z")
    );
    for none in [
        vec![window(
            "5-hour",
            Some(99.0),
            Some("2026-11-15T21:10:00.000Z"),
        )],
        vec![window("5-hour", Some(100.0), None)],
        vec![window("5-hour", None, Some("2026-11-15T21:10:00.000Z"))],
        vec![window(
            "5-hour",
            Some(100.0),
            Some("2026-11-15T19:00:00.000Z"),
        )],
        vec![],
    ] {
        assert_eq!(usage_windows_reset(&none, now), None, "{none:?}");
    }
    let reset = millis("2026-11-15T21:10:00Z");
    assert_eq!(resume_at(reset, now), reset + MINUTE);
    assert_eq!(resume_at(now - HOUR, now), now + MINUTE);
}

/// What a resume sends: the owner's queued follow-up first; else the
/// limit-refused prompt the recovery receipt holds, only when it is
/// definitely unsent and has no attachments (and then its receipt is
/// consumed by the send); never a prompt whose delivery is unknown or that
/// reached the provider; otherwise "Continue from where you stopped."
#[test]
fn a_resume_reuses_only_a_prompt_that_never_reached_the_provider() {
    let unsent = receipt(PromptDelivery::Unsent, "Add the export button.", false);
    assert_eq!(
        resume_prompt_choice(Some("Then run the tests."), Some(&unsent)),
        ("Then run the tests.".to_owned(), None)
    );
    assert_eq!(
        resume_prompt_choice(None, Some(&unsent)),
        ("Add the export button.".to_owned(), Some("a".repeat(64)))
    );
    assert_eq!(
        resume_prompt_choice(Some("   "), Some(&unsent)).0,
        "Add the export button."
    );
    for kept in [
        receipt(PromptDelivery::Unknown, "Delete the old branch.", false),
        receipt(PromptDelivery::Accepted, "Add the export button.", false),
        receipt(PromptDelivery::Unsent, "Read these notes.", true),
    ] {
        assert_eq!(
            resume_prompt_choice(None, Some(&kept)),
            (DEFAULT_RESUME_PROMPT.to_owned(), None),
            "{kept:?}"
        );
    }
    assert_eq!(
        resume_prompt_choice(Some(&"x".repeat(MAX_PROMPT_BYTES + 1)), None),
        (DEFAULT_RESUME_PROMPT.to_owned(), None)
    );
    assert_eq!(
        resume_prompt("Continue from where you stopped."),
        "[Interface] Resumed after the usage limit reset:\nContinue from where you stopped."
    );
}

/// One resume per thread: choosing it again replaces the earlier one (any
/// other schedule of the thread stays), it goes through the same due pass
/// and one attempt as any schedule, the view marks it, a switch drops it
/// (the limit was the old provider's) while the thread's other schedules
/// follow, and it survives a reopen.
#[test]
fn a_thread_keeps_one_resume_that_replaces_the_last_and_goes_with_a_switch() {
    let temp = private_temp();
    let file = temp.path().join(THREAD_SCHEDULES_FILE_NAME);
    let mut store = ThreadScheduleStore::open(&file).unwrap();
    store
        .add(
            new(
                "s-other",
                "claude#a",
                ScheduleWhen::Interval { minutes: 60 },
            ),
            NOW + HOUR,
            NOW,
        )
        .unwrap();
    store
        .set_resume(
            resume("s-r1", "claude#a", NOW + MINUTE, None),
            NOW + MINUTE,
            NOW,
        )
        .unwrap();
    store
        .set_resume(
            resume("s-r2", "claude#a", NOW + 2 * MINUTE, Some(&"b".repeat(64))),
            NOW + 2 * MINUTE,
            NOW,
        )
        .unwrap();
    store
        .set_resume(
            resume("s-rb", "claude#b", NOW + MINUTE, None),
            NOW + MINUTE,
            NOW,
        )
        .unwrap();
    assert!(store.schedule("s-r1").is_none(), "replaced");
    assert_eq!(
        store.schedule("s-r2").unwrap().receipt,
        Some("b".repeat(64))
    );
    assert!(store.schedule("s-other").is_some() && store.schedule("s-rb").is_some());
    let view = store.view("claude#a", false).unwrap();
    let kinds = view
        .schedules
        .iter()
        .map(|schedule| (schedule.id.as_str(), schedule.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            ("s-r2", ScheduleKind::Resume),
            ("s-other", ScheduleKind::Prompt)
        ]
    );
    // Only a one-time resume is a resume.
    let mut wrong = resume("s-bad", "claude#a", NOW + MINUTE, None);
    wrong.when = ScheduleWhen::Interval { minutes: 60 };
    assert!(store.set_resume(wrong, NOW + MINUTE, NOW).is_err());
    assert!(
        store.schedule("s-r2").is_some(),
        "a refusal changes nothing"
    );
    // Due like any one-time schedule: fires once, recorded before its send.
    assert_eq!(
        store.advance(NOW + 2 * MINUTE, |_| false).unwrap(),
        [
            Advanced::Waiting {
                id: "s-r2".into(),
                occurrence: NOW + 2 * MINUTE
            },
            Advanced::Waiting {
                id: "s-rb".into(),
                occurrence: NOW + MINUTE
            },
        ]
    );
    let attempt = store
        .begin_attempt("s-r2", NOW + 2 * MINUTE, NOW + 2 * MINUTE)
        .unwrap()
        .unwrap();
    assert_eq!(attempt.kind, ScheduleKind::Resume);
    assert!(
        store
            .begin_attempt("s-r2", NOW + 2 * MINUTE, NOW + 2 * MINUTE)
            .unwrap()
            .is_none()
    );
    drop(store);
    let mut store = ThreadScheduleStore::open(&file).unwrap();
    assert_eq!(store.schedule("s-rb").unwrap().kind, ScheduleKind::Resume);
    store.follow("claude#b", "codex#b").unwrap();
    assert!(
        store.schedule("s-rb").is_none(),
        "a switch drops the resume"
    );
    store.follow("claude#a", "codex#a").unwrap();
    assert_eq!(store.schedule("s-other").unwrap().thread_key, "codex#a");
}

/// The version 1 store (protocol 62) is read once and carried over as
/// version 2, unchanged on disk; a version 2 store is never confused with
/// it, and a version 1 file that cannot be read is left alone.
#[test]
fn a_v1_schedules_store_is_carried_over_once_and_left_unchanged() {
    let temp = private_temp();
    let older = temp.path().join(THREAD_SCHEDULES_V1_FILE_NAME);
    let bytes = serde_json::to_vec(&json!({
        "version": 1,
        "schedules": [{"id": "s-1", "threadKey": "claude#a", "prompt": "Check the build.",
            "when": {"kind": "interval", "minutes": 60}, "enabled": true, "createdAt": 1,
            "nextAt": NOW, "setBy": "agent"}],
        "watches": [{"threadKey": "claude#a", "kind": "pullRequest", "enabled": true}],
    }))
    .unwrap();
    std::fs::write(&older, &bytes).unwrap();
    std::fs::set_permissions(&older, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let file = temp.path().join(THREAD_SCHEDULES_FILE_NAME);
    let store = ThreadScheduleStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    let carried = store.schedule("s-1").unwrap();
    assert_eq!(carried.set_by, ScheduleSetBy::Agent);
    assert_eq!(carried.kind, ScheduleKind::Prompt);
    assert!(store.watch("claude#a").is_some());
    drop(store);
    assert_eq!(
        std::fs::read(&older).unwrap(),
        bytes,
        "the v1 file is left as it was"
    );
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written["version"], 2);
    // Read once: a change made since is not overwritten by the v1 file.
    let mut store = ThreadScheduleStore::open(&file).unwrap();
    store.delete("s-1").unwrap();
    drop(store);
    let store = ThreadScheduleStore::open(&file).unwrap();
    assert!(store.schedule("s-1").is_none());
    drop(store);
    // An unreadable v1 file is left for the build that wrote it.
    let other = private_temp();
    let older = other.path().join(THREAD_SCHEDULES_V1_FILE_NAME);
    std::fs::write(&older, b"{").unwrap();
    std::fs::set_permissions(&older, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let store = ThreadScheduleStore::open(&other.path().join(THREAD_SCHEDULES_FILE_NAME)).unwrap();
    assert!(store.schedules().is_empty() && store.quarantined_artifact().is_none());
    assert_eq!(std::fs::read(&older).unwrap(), b"{");
}

/// A firing waiting for its thread is what happens next, so the view lists it
/// first (the pane's "Next:" line shows the view's first schedule), ahead of
/// a schedule whose next time is still to come.
#[test]
fn a_firing_waiting_for_its_thread_comes_first_in_the_view() {
    let temp = private_temp();
    let mut store =
        ThreadScheduleStore::open(&temp.path().join(THREAD_SCHEDULES_FILE_NAME)).unwrap();
    store
        .add(
            new("s-once", "claude#a", ScheduleWhen::Once { at: NOW }),
            NOW,
            NOW - MINUTE,
        )
        .unwrap();
    store
        .add(
            new(
                "s-later",
                "claude#a",
                ScheduleWhen::Interval { minutes: 60 },
            ),
            NOW + HOUR,
            NOW,
        )
        .unwrap();
    store.advance(NOW, |_| false).unwrap();
    let view = store.view("claude#a", false).unwrap();
    assert_eq!(view.schedules[0].id, "s-once");
    assert_eq!(view.schedules[0].waiting, Some(NOW));
    assert_eq!(view.schedules[1].id, "s-later");
}
