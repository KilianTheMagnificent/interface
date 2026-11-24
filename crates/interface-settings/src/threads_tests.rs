use interface_contracts::threads::{
    LineageRelation, PullRequestState, RunOutcome, SettledReason, ThreadRun, ThreadSection,
    ThreadSettings, ThreadWorkspace, TitleSource, TurnState, WorkspaceKept, WorkspaceMode,
};

use super::*;

const NOW: u64 = 1_791_300_000_000;
const HOUR: u64 = 3_600_000;

fn document(keys: &[&str]) -> ThreadsDocument {
    let mut document = ThreadsDocument::default();
    for (offset, key) in (0_u64..).zip(keys) {
        document
            .records
            .push(ThreadRecord::new(*key, NOW - 10 * HOUR + offset));
    }
    document.seeded = true;
    document
}

fn section(document: &ThreadsDocument, key: &str, now: u64) -> ThreadSection {
    document.record(key).unwrap().section(now)
}

fn order(document: &ThreadsDocument, section: ThreadSection, now: u64) -> Vec<String> {
    document
        .ordered(section, now)
        .into_iter()
        .map(|index| document.records[index].key.clone())
        .collect()
}

fn worktree(path: &str, owned: bool) -> ThreadWorkspace {
    ThreadWorkspace {
        mode: WorkspaceMode::Worktree,
        project_root: "/repo".into(),
        worktree_path: Some(path.into()),
        branch: Some("iface/fix-login-3f9a2c".into()),
        base_ref: Some("main".into()),
        owned,
        missing: None,
        kept: None,
    }
}

#[test]
fn view_checks_each_shared_worktree_once_and_refreshes_on_the_next_read() {
    use std::cell::Cell;
    let mut document = document(&["a", "b", "c"]);
    for record in &mut document.records {
        record.workspace = Some(worktree("/shared", false));
    }
    let checks = Cell::new(0);
    let view = document.view(NOW, |_| {
        checks.set(checks.get() + 1);
        false
    });
    assert_eq!(checks.get(), 1);
    assert!(
        view.threads
            .iter()
            .all(|thread| thread.workspace.as_ref().unwrap().missing == Some(false))
    );
    let next = document.view(NOW + 1, |_| {
        checks.set(checks.get() + 1);
        true
    });
    assert_eq!(checks.get(), 2);
    assert!(
        next.threads
            .iter()
            .all(|thread| thread.workspace.as_ref().unwrap().missing == Some(true))
    );
}

#[test]
fn seeding_a_full_library_keeps_newest_first_across_order_key_spreads() {
    let mut document = document(&[]);
    let sessions = (0..MAX_RECORDS)
        .map(|index| ObservedSession {
            first_seen: Some(NOW + index as u64),
            ..session(&format!("claude#{index:04}"), SeedOrigin::App)
        })
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    document.reconcile(&sessions, |_| false, true, NOW + MAX_RECORDS as u64);
    eprintln!("seed {} threads: {:?}", MAX_RECORDS, started.elapsed());
    let expected = sessions
        .iter()
        .rev()
        .map(|row| row.key.clone())
        .collect::<Vec<_>>();
    assert_eq!(order(&document, ThreadSection::Active, NOW), expected);
    assert!(document_valid(&document));
    let before = document.clone();
    document.reconcile(&sessions, |_| false, true, NOW + MAX_RECORDS as u64);
    assert_eq!(document, before);
}

fn session(key: &str, origin: SeedOrigin) -> ObservedSession {
    ObservedSession {
        key: key.into(),
        origin,
        first_seen: Some(NOW - 20 * DAY_MS),
        last_activity: Some(NOW - HOUR),
        busy: false,
        archived: false,
        listed: true,
    }
}

#[allow(clippy::unnecessary_wraps)] // Compared and assigned as `last_run`.
fn ended(started_at: u64, ended_at: u64, outcome: RunOutcome) -> Option<ThreadRun> {
    Some(ThreadRun {
        started_at,
        ended_at: Some(ended_at),
        outcome: Some(outcome),
    })
}

#[allow(clippy::unnecessary_wraps)]
fn open_run(started_at: u64) -> Option<ThreadRun> {
    Some(ThreadRun {
        started_at,
        ended_at: None,
        outcome: None,
    })
}

#[test]
fn store_derivation_table_covers_sections_woke_and_unread() {
    let mut record = ThreadRecord::new("claude#a", NOW - DAY_MS);
    assert_eq!(record.section(NOW), ThreadSection::Active);
    assert!(!record.woke(NOW) && !record.unread());

    record.pinned_at = Some(NOW - HOUR);
    assert_eq!(record.section(NOW), ThreadSection::Pinned);
    record.snoozed_until = Some(NOW + HOUR);
    assert_eq!(
        record.section(NOW),
        ThreadSection::Snoozed,
        "snooze outranks pin"
    );
    assert!(!record.woke(NOW));
    // The snooze runs out: back to pinned, and woke until visited.
    assert_eq!(record.section(NOW + 2 * HOUR), ThreadSection::Pinned);
    assert!(record.woke(NOW + 2 * HOUR));
    record.last_visited_at = Some(NOW + HOUR + 1);
    assert!(!record.woke(NOW + 2 * HOUR));
    record.settled_at = Some(NOW);
    record.settled_reason = Some(SettledReason::Manual);
    assert_eq!(
        record.section(NOW),
        ThreadSection::Settled,
        "settle outranks all"
    );

    // Unread: a finished run after the last visit, or a mark.
    let mut record = ThreadRecord::new("claude#b", NOW - DAY_MS);
    record.last_run = ended(NOW - 2 * HOUR, NOW - HOUR, RunOutcome::Completed);
    assert!(record.unread(), "never visited");
    record.last_visited_at = Some(NOW - HOUR / 2);
    assert!(!record.unread(), "visited after it ended");
    record.last_run = ended(NOW - 2 * HOUR, NOW, RunOutcome::Failed);
    assert!(record.unread(), "a failure after the visit");
    record.marked_unread = true;
    record.last_run = None;
    assert!(record.unread(), "marked");
}

#[test]
fn unread_rules_ignore_interrupted_and_running_runs() {
    let mut record = ThreadRecord::new("codex#a", NOW - DAY_MS);
    record.last_run = ended(NOW - 2 * HOUR, NOW - HOUR, RunOutcome::Interrupted);
    assert!(!record.unread(), "an interrupt never makes a thread unread");
    record.marked_unread = true;
    record.last_run = open_run(NOW);
    assert!(record.running());
    assert!(!record.unread(), "a running thread is never unread");
}

#[test]
fn order_keys_sort_as_strings_and_leave_room_everywhere() {
    assert_eq!(order_key_between(None, None), "V");
    let mut first = order_key_between(None, None);
    // Inserting at the top again and again keeps strictly decreasing keys.
    for _ in 0..200 {
        let next = order_key_between(None, Some(&first));
        assert!(next < first, "{next} < {first}");
        assert!(order_key_valid(&next), "{next}");
        first = next;
    }
    let mut last = order_key_between(None, None);
    for _ in 0..200 {
        let next = order_key_between(Some(&last), None);
        assert!(next > last && order_key_valid(&next));
        last = next;
    }
    let (low, high) = ("a1", "a2");
    let middle = order_key_between(Some(low), Some(high));
    assert!(low < middle.as_str() && middle.as_str() < high, "{middle}");
    let middle = order_key_between(Some("0V"), Some("1"));
    assert!("0V" < middle.as_str() && middle.as_str() < "1", "{middle}");
    // An inverted pair is treated as an open end, never a panic.
    assert!(order_key_between(Some("b"), Some("a")).as_str() > "b");

    let spread = spread_keys(5000);
    assert_eq!(spread.len(), 5000);
    assert!(spread.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        spread
            .iter()
            .all(|key| order_key_valid(key) && key.len() <= 3)
    );
}

#[test]
fn new_and_unsettled_threads_go_on_top_and_activity_never_reorders() {
    let mut document = document(&["a", "b", "c"]);
    // Unkeyed threads sort newest first, after keyed ones.
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["c", "b", "a"]
    );
    document.settle("a", SettledReason::Manual, NOW).unwrap();
    document.unsettle("a", NOW + 1).unwrap();
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["a", "c", "b"]
    );
    assert_eq!(document.record("a").unwrap().unsettled_at, Some(NOW + 1));
    // A run on "b" does not move it.
    document.records[1].last_run = ended(NOW, NOW + 5, RunOutcome::Completed);
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["a", "c", "b"]
    );
}

#[test]
fn settling_clears_pin_and_snooze_and_pinning_unsettles() {
    let mut document = document(&["a", "b"]);
    document.pin("a", NOW).unwrap();
    document.snooze("a", NOW + HOUR, NOW).unwrap();
    assert_eq!(section(&document, "a", NOW), ThreadSection::Snoozed);
    assert!(
        document.record("a").unwrap().pin_order.is_some(),
        "snooze keeps the pin"
    );
    document
        .settle("a", SettledReason::Manual, NOW + 1)
        .unwrap();
    let record = document.record("a").unwrap();
    assert_eq!(record.section(NOW), ThreadSection::Settled);
    assert!(record.pinned_at.is_none() && record.pin_order.is_none());
    assert!(record.snoozed_until.is_none() && record.snoozed_at.is_none());
    assert_eq!(record.settled_reason, Some(SettledReason::Manual));
    // A second settle keeps the first.
    document.settle("a", SettledReason::Idle, NOW + 2).unwrap();
    assert_eq!(document.record("a").unwrap().settled_at, Some(NOW + 1));

    document.pin("a", NOW + 3).unwrap();
    let record = document.record("a").unwrap();
    assert_eq!(record.section(NOW + 3), ThreadSection::Pinned);
    assert_eq!(record.unsettled_at, Some(NOW + 3));
    assert!(record.settled_reason.is_none());
    document.pin("b", NOW + 4).unwrap();
    assert_eq!(order(&document, ThreadSection::Pinned, NOW + 4), ["b", "a"]);
    document.unpin("b", NOW + 5).unwrap();
    assert_eq!(order(&document, ThreadSection::Pinned, NOW + 5), ["a"]);
    assert_eq!(order(&document, ThreadSection::Active, NOW + 5), ["b"]);
    assert_eq!(
        document.pin("missing", NOW).unwrap_err().code,
        THREADS_UNKNOWN_CODE
    );
}

#[test]
fn snooze_keeps_its_place_wakes_and_clears_once_seen() {
    let mut document = document(&["a"]);
    assert_eq!(
        document.snooze("a", NOW, NOW).unwrap_err().code,
        THREADS_INVALID_CODE
    );
    document.settle("a", SettledReason::Idle, NOW).unwrap();
    document.snooze("a", NOW + HOUR, NOW).unwrap();
    assert_eq!(
        section(&document, "a", NOW),
        ThreadSection::Snoozed,
        "snoozing un-settles"
    );
    assert_eq!(document.next_snooze_wake(NOW), Some(NOW + HOUR));
    assert_eq!(document.next_snooze_wake(NOW + HOUR), None);
    // Woke: kept through writes until visited, then cleared.
    document.normalize(NOW + 2 * HOUR);
    assert!(document.record("a").unwrap().woke(NOW + 2 * HOUR));
    document.visit("a", NOW + 3 * HOUR).unwrap();
    document.normalize(NOW + 3 * HOUR);
    let record = document.record("a").unwrap();
    assert!(record.snoozed_until.is_none() && !record.woke(NOW + 3 * HOUR));
    document
        .snooze("a", NOW + 5 * HOUR, NOW + 4 * HOUR)
        .unwrap();
    document.unsnooze("a").unwrap();
    assert_eq!(
        section(&document, "a", NOW + 4 * HOUR),
        ThreadSection::Active
    );
}

#[test]
fn reorder_places_before_a_key_moves_across_sections_and_spreads_long_keys() {
    let mut document = document(&["a", "b", "c", "d"]);
    document
        .reorder("a", ThreadSection::Active, Some("c"), NOW)
        .unwrap();
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["d", "a", "c", "b"]
    );
    document
        .reorder("d", ThreadSection::Active, None, NOW)
        .unwrap();
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["a", "c", "b", "d"]
    );
    // Into Pinned pins; back into Active unpins.
    document
        .reorder("b", ThreadSection::Pinned, None, NOW)
        .unwrap();
    assert_eq!(order(&document, ThreadSection::Pinned, NOW), ["b"]);
    document
        .reorder("c", ThreadSection::Pinned, Some("b"), NOW)
        .unwrap();
    assert_eq!(order(&document, ThreadSection::Pinned, NOW), ["c", "b"]);
    document
        .reorder("c", ThreadSection::Active, Some("a"), NOW)
        .unwrap();
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["c", "a", "d"]
    );
    assert_eq!(
        document
            .reorder("a", ThreadSection::Settled, None, NOW)
            .unwrap_err()
            .code,
        THREADS_INVALID_CODE
    );
    // Dropping between two neighbours again and again spreads the section
    // instead of growing keys without bound.
    for _ in 0..400 {
        document
            .reorder("d", ThreadSection::Active, Some("a"), NOW)
            .unwrap();
        document
            .reorder("a", ThreadSection::Active, Some("d"), NOW)
            .unwrap();
    }
    assert_eq!(
        order(&document, ThreadSection::Active, NOW),
        ["c", "a", "d"]
    );
    assert!(
        document
            .records
            .iter()
            .filter_map(|record| record.active_order.as_deref())
            .all(|key| key.len() <= ORDER_KEY_SPREAD_AT)
    );
    assert!(document_valid(&document));
}

#[test]
fn prompt_digest_and_ref_names_follow_the_frozen_rules() {
    // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e…
    assert_eq!(prompt_digest("hello"), "2cf24dba5fb0a30e");
    assert_eq!(prompt_digest("  hello\r\n"), "2cf24dba5fb0a30e");
    assert_eq!(normalized_prompt(" a\r\nb \n"), "a\nb");
    assert_ne!(prompt_digest("a\r\nb"), prompt_digest("a b"));
    let namespace = thread_ref_namespace("claude#a");
    assert!(namespace.starts_with("refs/interface/threads/"));
    assert_eq!(namespace.len(), "refs/interface/threads/".len() + 16);
    assert_eq!(
        turn_ref("claude#a", 3, false),
        format!("{namespace}/3/start")
    );
    assert_eq!(turn_ref("claude#a", 3, true), format!("{namespace}/3/end"));
    assert_eq!(
        recovery_ref("claude#a", 7),
        format!("{namespace}/recovery/7")
    );
    assert_eq!(
        pending_baseline_ref("prep-1"),
        "refs/interface/threads/pending/prep-1"
    );
    let long = "x".repeat(250);
    let turn = TurnRecord::new(0, &format!(" {long} "), NOW, None);
    assert_eq!(turn.preview.chars().count(), PREVIEW_CHARS);
    assert!(turn.preview_truncated && turn.prompt().is_none());
    let turn = TurnRecord::new(0, "Fix the flaky login test\r\n", NOW, None);
    assert_eq!(turn.prompt(), Some("Fix the flaky login test"));
}

#[test]
fn seeding_rules_settle_store_and_stale_sessions_on_first_reconcile_only() {
    let mut document = ThreadsDocument::default();
    let mut stale = session("claude#stale", SeedOrigin::App);
    stale.last_activity = Some(NOW - 4 * DAY_MS);
    let fresh = session("claude#fresh", SeedOrigin::App);
    let store = session("codex#cli", SeedOrigin::Store);
    let mut working_store = session("codex#busy", SeedOrigin::Store);
    working_store.busy = true;
    let mut agent = session("claude#agent", SeedOrigin::App);
    agent.listed = false;
    let mut archived = session("claude#archived", SeedOrigin::App);
    archived.archived = true;
    let sessions = [stale, fresh, store, working_store, agent, archived];
    let reconciled = document.reconcile(&sessions, |_| false, true, NOW);
    assert_eq!(reconciled, Reconciled::default());
    assert!(document.seeded);
    let reason = |key: &str| document.record(key).unwrap().settled_reason;
    assert_eq!(reason("claude#stale"), Some(SettledReason::Idle));
    assert_eq!(reason("claude#fresh"), None);
    assert_eq!(reason("codex#cli"), Some(SettledReason::External));
    assert_eq!(
        reason("codex#busy"),
        None,
        "a working CLI session stays active"
    );
    assert!(document.record("claude#agent").is_none());
    assert!(document.record("claude#archived").is_none());
    let fresh = document.record("claude#fresh").unwrap();
    assert_eq!(fresh.created_at, NOW - 20 * DAY_MS, "createdAt = firstSeen");
    assert!(
        fresh.active_order.is_none(),
        "first seeding sorts by creation"
    );

    // Later: a stale app session is a new thread at the top; store stays external.
    let mut later = session("claude#later", SeedOrigin::App);
    later.last_activity = Some(NOW - 10 * DAY_MS);
    later.first_seen = None;
    let later_store = session("codex#later", SeedOrigin::Store);
    let mut all = sessions.to_vec();
    all.extend([later, later_store]);
    document.reconcile(&all, |_| false, true, NOW + 1);
    let record = document.record("claude#later").unwrap();
    assert_eq!(record.section(NOW), ThreadSection::Active);
    assert_eq!(record.created_at, NOW + 1, "no firstSeen: now");
    assert_eq!(
        order(&document, ThreadSection::Active, NOW)[0],
        "claude#later"
    );
    assert_eq!(
        document.record("codex#later").unwrap().settled_reason,
        Some(SettledReason::External)
    );
}

/// Review C1: a session a Code pane or the CLI started shows under From
/// terminals until the owner brings it into Threads; settling it by hand later
/// puts it in Settled, never back under From terminals.
#[test]
fn terminal_sessions_sit_under_from_terminals_until_brought_into_threads() {
    let mut document = ThreadsDocument::default();
    let sessions = [
        session("codex#cli", SeedOrigin::Store),
        session("claude#app", SeedOrigin::App),
    ];
    document.reconcile(&sessions, |_| false, true, NOW);
    assert_eq!(
        section(&document, "codex#cli", NOW),
        ThreadSection::FromTerminals
    );
    assert_eq!(section(&document, "claude#app", NOW), ThreadSection::Active);
    assert_eq!(
        order(&document, ThreadSection::FromTerminals, NOW),
        ["codex#cli"]
    );
    assert!(order(&document, ThreadSection::Settled, NOW).is_empty());
    let view = document.view(NOW, |_| false);
    assert_eq!(
        view.threads.last().map(|thread| thread.section),
        Some(ThreadSection::FromTerminals),
        "From terminals is drawn last"
    );
    assert!(
        document
            .reorder("codex#cli", ThreadSection::FromTerminals, None, NOW)
            .is_err()
    );

    // Continue as thread: un-settled to the top of Active, and visited.
    document.unsettle("codex#cli", NOW + 1).unwrap();
    document.visit("codex#cli", NOW + 1).unwrap();
    assert_eq!(
        section(&document, "codex#cli", NOW + 1),
        ThreadSection::Active
    );
    assert_eq!(
        order(&document, ThreadSection::Active, NOW + 1)[0],
        "codex#cli"
    );
    document
        .settle("codex#cli", SettledReason::Manual, NOW + 2)
        .unwrap();
    assert_eq!(
        section(&document, "codex#cli", NOW + 2),
        ThreadSection::Settled
    );
}

#[test]
fn reconcile_marks_gone_and_hidden_sessions_missing_keeps_archived_and_closes_orphan_runs() {
    let mut document = document(&["claude#gone", "claude#kept", "claude#hidden", "claude#arch"]);
    document.records[3].workspace = Some(worktree("/repo-wt/one", true));
    document.records[1].last_run = open_run(NOW - HOUR);
    document.records[1]
        .turns
        .push(TurnRecord::new(0, "go", NOW - HOUR, None));
    let mut hidden = session("claude#hidden", SeedOrigin::App);
    hidden.listed = false;
    let mut archived = session("claude#arch", SeedOrigin::App);
    archived.archived = true;
    let sessions = [session("claude#kept", SeedOrigin::App), hidden, archived];
    let reconciled = document.reconcile(&sessions, |_| false, true, NOW);
    assert!(reconciled.dropped.is_empty(), "missing, not yet dropped");
    for key in ["claude#gone", "claude#hidden"] {
        assert_eq!(document.record(key).unwrap().missing_since, Some(NOW));
    }
    assert_eq!(reconciled.to_release, ["claude#arch"]);
    assert!(
        document.record("claude#arch").is_some(),
        "archived keeps its record"
    );
    let kept = document.record("claude#kept").unwrap();
    assert_eq!(
        kept.last_run.as_ref().unwrap().outcome,
        Some(RunOutcome::Interrupted)
    );
    assert_eq!(kept.turns[0].state, TurnState::Interrupted);
    // Released once; restoring the session clears the release so a later
    // archive releases again.
    document.record_release("claude#arch", ReleaseOutcome::Removed, NOW);
    assert!(
        document
            .reconcile(&sessions, |_| false, true, NOW)
            .to_release
            .is_empty()
    );
    let mut restored = sessions.clone();
    restored[2].archived = false;
    document.reconcile(&restored, |_| false, true, NOW);
    assert!(document.record("claude#arch").unwrap().release.is_none());
    // A run the host still tracks is left open, and so is a busy one.
    let kept = document.index("claude#kept").unwrap();
    document.records[kept].last_run = open_run(NOW);
    document.reconcile(&restored, |key| key == "claude#kept", true, NOW + 1);
    assert!(document.record("claude#kept").unwrap().running());
    restored[0].busy = true;
    document.reconcile(&restored, |_| false, true, NOW + 2);
    assert!(document.record("claude#kept").unwrap().running());
}

/// 6 Oct 2026 (Threads review): an archive that kept a thread's worktree
/// said nothing. The view says why it stayed, while the thread still owns
/// it, until a later release succeeds or the session is unarchived.
#[test]
fn a_kept_release_shows_as_workspace_kept_until_a_later_release_or_unarchive() {
    let kept_of = |document: &ThreadsDocument| {
        document
            .view(NOW, |_| false)
            .threads
            .into_iter()
            .find(|thread| thread.key == "codex#wt")
            .and_then(|thread| thread.workspace)
            .and_then(|workspace| workspace.kept)
    };
    let mut document = document(&["codex#wt"]);
    document.records[0].workspace = Some(worktree("/repo-wt/one", true));
    assert_eq!(kept_of(&document), None);
    for (outcome, kept) in [
        (ReleaseOutcome::KeptDirty, Some(WorkspaceKept::Dirty)),
        (ReleaseOutcome::KeptIgnored, Some(WorkspaceKept::Ignored)),
        (ReleaseOutcome::InUse, Some(WorkspaceKept::InUse)),
        (ReleaseOutcome::Removed, None),
        (ReleaseOutcome::Missing, None),
        (ReleaseOutcome::Failed, None),
    ] {
        document.record_release("codex#wt", outcome, NOW);
        assert_eq!(kept_of(&document), kept, "{outcome:?}");
    }
    // Never stored: the record's workspace stays as it was bound.
    document.record_release("codex#wt", ReleaseOutcome::KeptIgnored, NOW);
    assert_eq!(document.records[0].workspace.as_ref().unwrap().kept, None);
    // Handed on to another thread, it is no longer this one's to report.
    document.records[0].workspace.as_mut().unwrap().owned = false;
    assert_eq!(kept_of(&document), None);
    document.records[0].workspace.as_mut().unwrap().owned = true;
    // The owner's own release clears it once it goes, and only a kept one.
    document.record_owner_release("codex#wt", ReleaseOutcome::Removed, NOW + 1);
    assert_eq!(kept_of(&document), None);
    document.records[0].release = None;
    document.record_owner_release("codex#wt", ReleaseOutcome::Removed, NOW + 2);
    assert_eq!(document.records[0].release, None);
    // Unarchiving clears it.
    document.record_release("codex#wt", ReleaseOutcome::KeptDirty, NOW);
    let mut archived = session("codex#wt", SeedOrigin::App);
    archived.archived = true;
    document.reconcile(&[archived.clone()], |_| false, true, NOW);
    assert_eq!(kept_of(&document), Some(WorkspaceKept::Dirty));
    archived.archived = false;
    document.reconcile(&[archived], |_| false, true, NOW);
    assert_eq!(kept_of(&document), None);
}

#[test]
fn auto_settle_is_blocked_by_busy_pinned_snoozed_disabled_and_recent_activity() {
    let keys = [
        "idle", "busy", "unknown", "pinned", "snoozed", "disabled", "recent",
    ];
    let mut document = document(&keys);
    for record in &mut document.records {
        record.created_at = NOW - 10 * DAY_MS;
    }
    let later = NOW + 10 * DAY_MS;
    document.pin("pinned", NOW - 10 * DAY_MS).unwrap();
    document.snooze("snoozed", later + HOUR, NOW).unwrap();
    document.set_auto_settle("disabled", false).unwrap();
    document.records[6].last_run = ended(later - DAY_MS, later - DAY_MS, RunOutcome::Completed);
    let settled = document.auto_settle(later, |key| match key {
        "busy" => Some(true),
        "unknown" => None,
        _ => Some(false),
    });
    assert_eq!(settled, ["idle"]);
    assert_eq!(
        document.record("idle").unwrap().settled_reason,
        Some(SettledReason::Idle)
    );
    // autoSettleDays 0 never settles for age.
    let mut never = self::document(&["a"]);
    never.records[0].created_at = 0;
    never.settings.auto_settle_days = 0;
    assert!(never.auto_settle(later, |_| Some(false)).is_empty());
}

#[test]
fn merged_pull_requests_settle_unless_the_thread_ran_after_the_merge() {
    let mut document = document(&["merged", "reran", "open"]);
    for key in ["merged", "reran", "open"] {
        let index = document.index(key).unwrap();
        document.records[index].workspace = Some(worktree(&format!("/wt/{key}"), true));
        document.records[index].created_at = NOW;
    }
    let found = |state| FoundPullRequest {
        url: "https://github.com/owner/atlas/pull/482".into(),
        number: 482,
        state,
    };
    assert_eq!(document.pull_requests_due(NOW, None).len(), 3);
    document.record_pull_request("merged", Some(found(PullRequestState::Merged)), NOW);
    document.record_pull_request("reran", Some(found(PullRequestState::Merged)), NOW);
    document.record_pull_request("open", Some(found(PullRequestState::Open)), NOW);
    assert!(
        document.pull_requests_due(NOW + 1, None).is_empty(),
        "rate-limited"
    );
    assert_eq!(document.pull_requests_due(NOW + 1, Some("open")).len(), 1);
    assert_eq!(
        document
            .pull_requests_due(NOW + PULL_REQUEST_INTERVAL_MS, None)
            .len(),
        3
    );
    let reran = document.index("reran").unwrap();
    document.records[reran].last_run = ended(NOW + 5, NOW + 6, RunOutcome::Completed);
    let settled = document.auto_settle(NOW + 10, |_| Some(false));
    assert_eq!(settled, ["merged"]);
    assert_eq!(
        document.record("merged").unwrap().settled_reason,
        Some(SettledReason::Merged)
    );
    document.settings.track_pull_requests = false;
    assert!(document.pull_requests_due(NOW + DAY_MS, None).is_empty());
}

fn uses(entries: &[(&str, &str, bool)]) -> Vec<SessionUse> {
    entries
        .iter()
        .map(|(key, cwd, live)| SessionUse {
            key: (*key).into(),
            cwd: (*cwd).into(),
            live: *live,
            archived: false,
            title: format!("Title {key}"),
        })
        .collect()
}

#[test]
fn restore_refusals_name_shared_unowned_missing_busy_and_uncheckpointed() {
    let mut document = document(&["owner", "local", "other"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    document.records[0].turns.push(TurnRecord::new(
        0,
        "a",
        NOW,
        Some(turn_ref("owner", 0, false)),
    ));
    document.records[0]
        .turns
        .push(TurnRecord::new(1, "b", NOW, None));
    document.records[1].workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo".into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    });
    let base = [
        ("owner", "/wt/one", true),
        ("local", "/repo", false),
        ("other", "/x", false),
    ];
    let plan = document
        .restore_plan("owner", 0, false, true, &uses(&base), NOW)
        .unwrap();
    assert_eq!(plan.dir, "/wt/one");
    assert_eq!(plan.start_ref, turn_ref("owner", 0, false));
    assert_eq!(plan.recovery_ref, recovery_ref("owner", NOW));
    let code = |result: Result<RestorePlan, ThreadRefusal>| result.unwrap_err().code;
    assert_eq!(
        code(document.restore_plan("local", 0, false, true, &uses(&base), NOW)),
        THREADS_NOT_OWNED_CODE
    );
    assert_eq!(
        code(document.restore_plan("owner", 0, false, false, &uses(&base), NOW)),
        THREADS_WORKTREE_MISSING_CODE
    );
    assert_eq!(
        code(document.restore_plan("owner", 1, false, true, &uses(&base), NOW)),
        THREADS_NO_CHECKPOINT_CODE
    );
    assert_eq!(
        code(document.restore_plan("owner", 0, true, true, &uses(&base), NOW)),
        THREADS_BUSY_CODE
    );
    // A live session inside the worktree shares it.
    let live = [("owner", "/wt/one", true), ("other", "/wt/one/src", true)];
    assert_eq!(
        code(document.restore_plan("owner", 0, false, true, &uses(&live), NOW)),
        THREADS_SHARED_CODE
    );
    // An unsettled thread forked into it shares it too; once settled it does not.
    document.records[2].workspace = Some(worktree("/wt/one", false));
    assert_eq!(
        code(document.restore_plan("owner", 0, false, true, &uses(&base), NOW)),
        THREADS_SHARED_CODE
    );
    document
        .settle("other", SettledReason::Manual, NOW)
        .unwrap();
    assert!(
        document
            .restore_plan("owner", 0, false, true, &uses(&base), NOW)
            .is_ok()
    );
    // Release checks: owned, own session stopped, nobody else in it — a
    // settled fork in it still counts.
    assert_eq!(
        document
            .release_plan("owner", false, &uses(&base))
            .unwrap_err()
            .code,
        THREADS_SHARED_CODE
    );
    document.records[2].workspace = None;
    assert_eq!(
        document.release_plan("owner", false, &uses(&base)).unwrap(),
        ReleasePlan {
            git_root: "/repo".into(),
            worktree_path: "/wt/one".into(),
        }
    );
    assert_eq!(
        document
            .release_plan("owner", true, &uses(&base))
            .unwrap_err()
            .code,
        THREADS_BUSY_CODE
    );
    assert_eq!(
        document
            .release_plan("local", false, &uses(&base))
            .unwrap_err()
            .code,
        THREADS_NOT_OWNED_CODE
    );
}

/// 7 Oct 2026: two Threads ran at once in one checkout, and one turn's card
/// counted the other thread's README edit as its own. A shared checkout
/// measures a turn over the files its own edits named, and over nothing when
/// those are unknown or none.
#[test]
fn a_turn_in_a_shared_checkout_is_measured_over_its_own_files_only() {
    let local = |root: &str| ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: root.into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    };
    let mut document = document(&["mine", "theirs", "elsewhere", "fork"]);
    document.records[0].workspace = Some(local("/repo"));
    document.records[0].git_root = Some("/repo".into());
    let touched = BTreeSet::from(["/repo/src/session.ts".to_owned()]);
    let own = TurnMeasure::Files(vec!["/repo/src/session.ts".into()]);
    let alone = uses(&[
        ("mine", "/repo", true),
        ("theirs", "/x", false),
        ("elsewhere", "/y", false),
        ("fork", "/z", false),
    ]);
    let measure =
        |document: &ThreadsDocument, uses: &[SessionUse], touched: Option<&BTreeSet<String>>| {
            document.turn_measure("mine", Some(uses), touched)
        };
    // Alone in its checkout: every change there is its own, shell edits too.
    assert_eq!(
        measure(&document, &alone, Some(&touched)),
        TurnMeasure::Checkout
    );
    assert_eq!(measure(&document, &alone, None), TurnMeasure::Checkout);

    // Another unsettled Local thread in the same project.
    document.records[1].workspace = Some(local("/repo"));
    document.records[1].git_root = Some("/repo".into());
    assert_eq!(measure(&document, &alone, Some(&touched)), own);
    assert_eq!(measure(&document, &alone, None), TurnMeasure::Unknown);
    assert_eq!(
        measure(&document, &alone, Some(&BTreeSet::new())),
        TurnMeasure::Unknown
    );
    // Archived or settled, it no longer works there.
    let mut archived = alone.clone();
    archived[1].archived = true;
    assert_eq!(measure(&document, &archived, None), TurnMeasure::Checkout);
    document
        .settle("theirs", SettledReason::Manual, NOW)
        .unwrap();
    assert_eq!(measure(&document, &alone, None), TurnMeasure::Checkout);

    // A live session anywhere inside the repository shares it.
    let mut live = alone.clone();
    live[2] = uses(&[("elsewhere", "/repo/packages/web", true)]).remove(0);
    assert_eq!(measure(&document, &live, Some(&touched)), own);
    // So does a Local thread in another folder of the same repository: its
    // checkpoints cover all of it, whatever folder it names.
    document.records[2].workspace = Some(local("/repo-link/packages/web"));
    document.records[2].git_root = Some("/repo".into());
    assert_eq!(measure(&document, &alone, Some(&touched)), own);
    document.records[2].workspace = None;

    // A fork working in the same worktree shares it.
    document.records[0].workspace = Some(worktree("/wt/one", true));
    assert_eq!(measure(&document, &alone, None), TurnMeasure::Checkout);
    document.records[3].workspace = Some(worktree("/wt/one", false));
    assert_eq!(measure(&document, &alone, Some(&touched)), own);

    // An unreadable registry, an unknown thread: shared, fail closed.
    assert_eq!(document.turn_measure("mine", None, Some(&touched)), own);
    assert_eq!(
        document.turn_measure("ghost", Some(&alone), None),
        TurnMeasure::Unknown
    );
}

/// 6 Oct 2026: a restore after Edit from here was refused with "Settle or
/// close it" naming the edited original by its key. It was settled already;
/// only its open session held the worktree.
#[test]
fn worktree_refusals_name_the_session_and_say_what_frees_the_folder() {
    let mut document = document(&["owner", "fork"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    document.records[0].turns.push(TurnRecord::new(
        0,
        "a",
        NOW,
        Some(turn_ref("owner", 0, false)),
    ));
    document.records[1].workspace = Some(worktree("/wt/one", false));
    let restore = |document: &ThreadsDocument, uses: &[SessionUse]| {
        document
            .restore_plan("owner", 0, false, true, uses, NOW)
            .unwrap_err()
    };
    let release = |document: &ThreadsDocument, uses: &[SessionUse]| {
        document.release_plan("owner", false, uses).unwrap_err()
    };
    let idle = uses(&[("owner", "/wt/one", false), ("fork", "/wt/one", false)]);
    let refusal = restore(&document, &idle);
    assert_eq!(refusal.code, THREADS_SHARED_CODE);
    assert_eq!(
        refusal.message,
        "“Title fork” also works in this worktree. Settle it, then try again."
    );

    document.settle("fork", SettledReason::Edited, NOW).unwrap();
    // Settled and closed: a restore goes ahead; a release still waits.
    assert!(
        document
            .restore_plan("owner", 0, false, true, &idle, NOW)
            .is_ok()
    );
    assert_eq!(
        release(&document, &idle).message,
        "“Title fork” also uses this worktree. Archive it, then try again."
    );
    // Settled but its session still open: settling again cannot help.
    let open = uses(&[("owner", "/wt/one", false), ("fork", "/wt/one", true)]);
    for refusal in [restore(&document, &open), release(&document, &open)] {
        assert_eq!(refusal.code, THREADS_SHARED_CODE);
        assert_eq!(
            refusal.message,
            "“Title fork” is still open in this worktree. Archive it to close it, then try again."
        );
    }
    // A session with no thread record.
    let other = uses(&[("owner", "/wt/one", false), ("plain", "/wt/one/src", true)]);
    let refusal = restore(&document, &other);
    assert_eq!(refusal.code, THREADS_SHARED_CODE);
    assert_eq!(
        refusal.message,
        "Another session, “Title plain”, is still open in this worktree. Close that session, then try again."
    );
    assert!(!refusal.message.contains("Settle"));
}

fn prepared(token: &str) -> PreparedRecord {
    PreparedRecord {
        token: token.into(),
        cwd: "/wt/one".into(),
        workspace: worktree("/wt/one", true),
        git_root: Some("/repo".into()),
        created: true,
        baseline_ref: Some(pending_baseline_ref(token)),
        created_at: NOW,
        lineage: None,
    }
}

/// A launch awaits its bind while a prepared workspace for the session's
/// folder is unbound and the session has no record, or only a seeded one;
/// the bind ends the wait, and a session nothing prepared a folder for (a
/// plain chat) never awaits one.
#[test]
fn a_launch_awaits_its_bind_only_while_its_prepared_folder_is_unbound() {
    let mut document = document(&["claude#chat"]);
    assert!(
        !document.awaits_bind("codex#new", "/wt/one"),
        "nothing prepared"
    );
    document.add_prepared(prepared("prep-1")).unwrap();
    assert!(document.awaits_bind("codex#new", "/wt/one"));
    assert!(
        document.awaits_bind("codex#new", "/wt/./one/"),
        "the same folder"
    );
    assert!(!document.awaits_bind("codex#new", "/wt/two"));
    assert!(!document.awaits_bind("codex#new", ""));
    // A record a reconcile seeded (no workspace, no turn) still awaits it.
    document.records.push(ThreadRecord::new("codex#new", NOW));
    assert!(document.awaits_bind("codex#new", "/wt/one"));
    document
        .bind(
            ThreadBind {
                key: "codex#new".into(),
                token: Some("prep-1".into()),
                prompt: "team start x".into(),
                title_source: None,
                start_ref: None,
            },
            NOW,
        )
        .unwrap();
    assert!(!document.awaits_bind("codex#new", "/wt/one"), "bound");
    // A thread that already ran is never taken for a launch.
    document.add_prepared(prepared("prep-2")).unwrap();
    assert!(!document.awaits_bind("codex#new", "/wt/one"));
}

/// An Away loop's work session is created with no prompt (review B3, 14
/// Oct 2026): it binds its worktree with no turn and no run, and its first
/// send records turn 0.
#[test]
fn an_empty_session_binds_its_worktree_with_no_turn_until_its_first_send() {
    let mut document = document(&["claude#old"]);
    document.add_prepared(prepared("prep-away")).unwrap();
    let baseline = document
        .bind_empty("codex#away", "prep-away", TitleSource::User, NOW)
        .unwrap();
    assert_eq!(
        baseline,
        Some(("/repo".to_owned(), pending_baseline_ref("prep-away"))),
        "the unused baseline is handed back to delete"
    );
    assert!(document.prepared.is_empty());
    let record = document.record("codex#away").unwrap();
    assert_eq!(record.workspace, Some(worktree("/wt/one", true)));
    assert_eq!(record.git_root.as_deref(), Some("/repo"));
    assert_eq!(record.title_source, Some(TitleSource::User));
    assert!(record.turns.is_empty() && record.last_run.is_none());
    assert!(!record.running() && !record.unread());
    assert_eq!(
        order(&document, ThreadSection::Active, NOW)[0],
        "codex#away"
    );
    let begun = document
        .begin_turn(
            "codex#away",
            "Away work",
            Some(turn_ref("codex#away", 0, false)),
            NOW + 1,
        )
        .unwrap();
    assert_eq!(begun.ordinal, 0);
    assert!(document.record("codex#away").unwrap().running());
    assert_eq!(
        document
            .bind_empty("codex#other", "prep-gone", TitleSource::User, NOW)
            .unwrap_err()
            .code,
        THREADS_UNKNOWN_WORKSPACE_CODE
    );
    assert!(document_valid(&document));
}

#[test]
fn bind_attaches_workspace_and_turn_zero_on_the_baseline_at_the_top() {
    let mut document = document(&["claude#old"]);
    document.add_prepared(prepared("prep-1")).unwrap();
    assert_eq!(
        document.add_prepared(prepared("prep-1")).unwrap_err().code,
        THREADS_INVALID_CODE
    );
    let finished = document
        .bind(
            ThreadBind {
                key: "codex#new".into(),
                token: Some("prep-1".into()),
                prompt: "Fix the flaky login test".into(),
                title_source: None,
                start_ref: Some(turn_ref("codex#new", 0, false)),
            },
            NOW,
        )
        .unwrap();
    assert!(finished.is_none(), "the launch run is still going");
    assert!(document.prepared.is_empty());
    let record = document.record("codex#new").unwrap();
    assert_eq!(record.workspace, Some(worktree("/wt/one", true)));
    assert_eq!(record.git_root.as_deref(), Some("/repo"));
    assert_eq!(record.title_source, Some(TitleSource::Prompt));
    assert!(record.lineage.is_none() && record.running());
    assert_eq!(record.turns.len(), 1);
    assert_eq!(record.turns[0].ordinal, 0);
    assert_eq!(
        record.turns[0].prompt_digest,
        prompt_digest("Fix the flaky login test")
    );
    assert_eq!(
        record.turns[0].start_ref,
        Some(turn_ref("codex#new", 0, false))
    );
    assert_eq!(order(&document, ThreadSection::Active, NOW)[0], "codex#new");
    assert_eq!(
        document
            .bind(
                ThreadBind {
                    key: "codex#other".into(),
                    token: Some("prep-gone".into()),
                    ..ThreadBind::default()
                },
                NOW,
            )
            .unwrap_err()
            .code,
        THREADS_UNKNOWN_WORKSPACE_CODE
    );

    // The pending baseline is kept when it could not move.
    document.add_prepared(prepared("prep-2")).unwrap();
    document
        .bind(
            ThreadBind {
                key: "codex#kept-baseline".into(),
                token: Some("prep-2".into()),
                prompt: "x".into(),
                ..ThreadBind::default()
            },
            NOW,
        )
        .unwrap();
    assert_eq!(
        document.record("codex#kept-baseline").unwrap().turns[0].start_ref,
        Some(pending_baseline_ref("prep-2"))
    );

    // A plain chat whose launch run already finished: turn 0 finishes at once.
    document.records.push(ThreadRecord::new("claude#chat", NOW));
    document.records.last_mut().unwrap().last_run = ended(NOW, NOW + 1, RunOutcome::Completed);
    let finished = document
        .bind(
            ThreadBind {
                key: "claude#chat".into(),
                prompt: "hi".into(),
                title_source: Some(TitleSource::User),
                ..ThreadBind::default()
            },
            NOW + 2,
        )
        .unwrap();
    assert!(finished.is_none(), "no git, no end checkpoint");
    let chat = document.record("claude#chat").unwrap();
    assert_eq!(chat.workspace.as_ref().unwrap().mode, WorkspaceMode::None);
    assert_eq!(chat.turns[0].state, TurnState::Done);
    assert_eq!(chat.title_source, Some(TitleSource::User));
    assert!(document_valid(&document));
}

/// "Edit from here" on the first prompt restores the original's worktree,
/// settles it, and starts a new thread in that worktree as an existing one.
#[test]
fn binding_an_existing_worktree_takes_ownership_from_its_settled_owner_only() {
    let mut document = document(&["codex#original", "codex#active-owner"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    document.records[1].workspace = Some(worktree("/wt/two", true));
    document.records[1].git_root = Some("/repo".into());
    let existing = |token: &str, path: &str| PreparedRecord {
        cwd: path.into(),
        workspace: worktree(path, false),
        created: false,
        ..prepared(token)
    };
    let bind = |key: &str, token: &str| ThreadBind {
        key: key.into(),
        token: Some(token.into()),
        prompt: "Start over".into(),
        ..ThreadBind::default()
    };

    // An unsettled owner keeps its worktree; the newcomer only shares it.
    document
        .add_prepared(existing("prep-2", "/wt/two"))
        .unwrap();
    document.bind(bind("codex#sharer", "prep-2"), NOW).unwrap();
    assert!(
        document
            .record("codex#active-owner")
            .unwrap()
            .workspace
            .as_ref()
            .unwrap()
            .owned
    );
    assert!(
        !document
            .record("codex#sharer")
            .unwrap()
            .workspace
            .as_ref()
            .unwrap()
            .owned
    );

    // A settled owner hands it over.
    document
        .settle("codex#original", SettledReason::Manual, NOW)
        .unwrap();
    document
        .add_prepared(existing("prep-1", "/wt/one/"))
        .unwrap();
    document
        .bind(bind("codex#restart", "prep-1"), NOW + 1)
        .unwrap();
    let workspace = |key: &str| document.record(key).unwrap().workspace.clone().unwrap();
    assert!(workspace("codex#restart").owned);
    assert_eq!(workspace("codex#restart").mode, WorkspaceMode::Worktree);
    assert!(!workspace("codex#original").owned);
    assert_eq!(
        document.record("codex#original").unwrap().owned_worktree(),
        None
    );
    assert_eq!(
        document.record("codex#restart").unwrap().owned_worktree(),
        Some("/wt/one/")
    );
    assert!(document_valid(&document));
}

/// 6 Oct 2026 (Threads review): a fork counted the provider's prompts from
/// the start; the host now names the kept turn by its digest and how many
/// later recorded turns repeat it.
#[test]
fn fork_point_names_the_kept_turn_by_digest_and_later_repeats() {
    let mut document = document(&["codex#a"]);
    for (ordinal, prompt) in ["again", "other", " again\r\n", "last"]
        .into_iter()
        .enumerate()
    {
        let ordinal = u32::try_from(ordinal).unwrap();
        document.records[0]
            .turns
            .push(TurnRecord::new(ordinal, prompt, NOW, None));
    }
    let point = |keep| document.fork_point("codex#a", keep);
    assert_eq!(
        point(1).unwrap(),
        ForkPoint {
            prompt_digest: prompt_digest("again"),
            later_same_digest: 1
        }
    );
    assert_eq!(point(3).unwrap().later_same_digest, 0);
    assert_eq!(point(2).unwrap().prompt_digest, prompt_digest("other"));
    for missing in [0, 5] {
        assert_eq!(point(missing).unwrap_err().code, THREADS_UNKNOWN_TURN_CODE);
    }
    // Trimmed turns are gone from the record, so they cannot be named.
    document.records[0].turns.remove(0);
    assert_eq!(
        document.fork_point("codex#a", 1).unwrap_err().code,
        THREADS_UNKNOWN_TURN_CODE
    );
    assert_eq!(
        document.fork_point("codex#none", 1).unwrap_err().code,
        THREADS_UNKNOWN_CODE
    );
}

#[test]
fn fork_and_edit_copy_lineage_and_move_worktree_ownership() {
    let mut document = document(&["codex#parent"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    for ordinal in 0..3 {
        document.records[0].turns.push(TurnRecord::new(
            ordinal,
            &format!("turn {ordinal}"),
            NOW,
            Some(turn_ref("codex#parent", ordinal, false)),
        ));
    }
    document
        .bind_fork(
            &ThreadFork {
                parent: "codex#parent".into(),
                child: "codex#fork".into(),
                relation: LineageRelation::Fork,
                kept_turns: 2,
            },
            NOW,
        )
        .unwrap();
    let fork = document.record("codex#fork").unwrap();
    let lineage = fork.lineage.as_ref().unwrap();
    assert_eq!(
        (
            lineage.relation,
            lineage.kept_turns,
            lineage.parent.as_str()
        ),
        (LineageRelation::Fork, 2, "codex#parent")
    );
    assert!(
        !fork.workspace.as_ref().unwrap().owned,
        "a fork shares the worktree"
    );
    assert_eq!(fork.turns.len(), 2);
    let parent = document.record("codex#parent").unwrap();
    assert!(parent.workspace.as_ref().unwrap().owned);
    assert_eq!(parent.section(NOW), ThreadSection::Active);

    document
        .bind_fork(
            &ThreadFork {
                parent: "codex#parent".into(),
                child: "codex#edit".into(),
                relation: LineageRelation::Edit,
                kept_turns: 1,
            },
            NOW + 1,
        )
        .unwrap();
    assert!(
        document
            .record("codex#edit")
            .unwrap()
            .workspace
            .as_ref()
            .unwrap()
            .owned
    );
    let parent = document.record("codex#parent").unwrap();
    assert!(!parent.workspace.as_ref().unwrap().owned);
    assert_eq!(parent.settled_reason, Some(SettledReason::Edited));
    assert_eq!(
        order(&document, ThreadSection::Active, NOW + 1),
        ["codex#edit", "codex#fork"]
    );
    assert_eq!(
        document
            .bind_fork(
                &ThreadFork {
                    parent: "codex#none".into(),
                    child: "codex#x".into(),
                    relation: LineageRelation::Fork,
                    kept_turns: 1,
                },
                NOW,
            )
            .unwrap_err()
            .code,
        THREADS_UNKNOWN_CODE
    );
}

#[test]
fn begin_turn_records_the_digest_wakes_the_thread_and_cancels_cleanly() {
    let mut document = document(&["claude#a"]);
    document
        .bind(
            ThreadBind {
                key: "claude#a".into(),
                prompt: "first".into(),
                ..ThreadBind::default()
            },
            NOW,
        )
        .unwrap();
    document.run_ended("claude#a", NOW + 1, RunOutcome::Completed);
    document
        .settle("claude#a", SettledReason::Manual, NOW + 2)
        .unwrap();
    let begun = document
        .begin_turn(
            "claude#a",
            "  second\r\n",
            Some("refs/interface/threads/x/1/start".into()),
            NOW + 3,
        )
        .unwrap();
    assert_eq!(begun.ordinal, 1);
    let record = document.record("claude#a").unwrap();
    assert_eq!(
        record.section(NOW + 3),
        ThreadSection::Active,
        "a send un-settles"
    );
    assert_eq!(record.turns[1].prompt_digest, prompt_digest("second"));
    assert_eq!(record.turns[1].state, TurnState::Running);
    assert_eq!(record.last_run, open_run(NOW + 3));
    document.cancel_turn("claude#a", &begun, NOW + 3);
    let record = document.record("claude#a").unwrap();
    assert_eq!(record.turns.len(), 1);
    assert_eq!(record.last_run, ended(NOW, NOW + 1, RunOutcome::Completed));
    // A send taken back after its run was already seen ending (a provider's
    // own error and idle beat its refusal) leaves no run of its own either.
    let begun = document
        .begin_turn("claude#a", "third", None, NOW + 4)
        .unwrap();
    document.run_ended("claude#a", NOW + 5, RunOutcome::Completed);
    document.cancel_turn("claude#a", &begun, NOW + 4);
    let record = document.record("claude#a").unwrap();
    assert_eq!(record.turns.len(), 1);
    assert_eq!(record.last_run, ended(NOW, NOW + 1, RunOutcome::Completed));
    assert_eq!(
        document
            .begin_turn("claude#none", "x", None, NOW)
            .unwrap_err()
            .code,
        THREADS_UNKNOWN_CODE
    );
}

#[test]
fn run_end_finishes_the_running_turn_and_answers_its_end_checkpoint() {
    let mut document = document(&["codex#a"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    assert!(
        document
            .run_ended("codex#a", NOW, RunOutcome::Completed)
            .is_none()
    );
    assert!(document.run_started("codex#a", NOW));
    assert!(!document.run_started("codex#a", NOW + 1), "already running");
    document.run_ended("codex#a", NOW + 2, RunOutcome::Completed);
    let start = turn_ref("codex#a", 0, false);
    document
        .begin_turn("codex#a", "go", Some(start.clone()), NOW + 3)
        .unwrap();
    let finished = document
        .run_ended("codex#a", NOW + 4, RunOutcome::Failed)
        .unwrap();
    assert_eq!(
        finished,
        FinishedTurn {
            key: "codex#a".into(),
            ordinal: 0,
            dir: "/wt/one".into(),
            start_ref: Some(start),
            end_ref: turn_ref("codex#a", 0, true),
        }
    );
    let record = document.record("codex#a").unwrap();
    assert_eq!(record.last_run, ended(NOW + 3, NOW + 4, RunOutcome::Failed));
    assert_eq!(record.turns[0].state, TurnState::Failed);
    document.record_turn_end(
        "codex#a",
        0,
        TurnEnd {
            end_ref: Some(finished.end_ref.clone()),
            counts: Some((2, 10, 3)),
            ..TurnEnd::default()
        },
    );
    let view = document.view(NOW + 5, |_| false);
    let turn = &view.threads[0].turns[0];
    assert!(turn.has_start && turn.has_end);
    assert_eq!(turn.ended_at, Some(NOW + 4));
    assert_eq!(
        (turn.files_changed, turn.added, turn.removed),
        (Some(2), Some(10), Some(3))
    );
    assert_eq!(
        view.threads[0].workspace.as_ref().unwrap().missing,
        Some(false)
    );
    assert!(view.threads[0].unread);
    let view = document.view(NOW + 5, |_| true);
    assert_eq!(
        view.threads[0].workspace.as_ref().unwrap().missing,
        Some(true)
    );
}

#[test]
fn a_generated_title_is_wanted_once_after_any_threads_first_completed_turn() {
    let mut document = document(&["codex#a", "claude#chat"]);
    document.settings.generated_titles = true;
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].title_source = Some(TitleSource::Prompt);
    document.records[1].title_source = Some(TitleSource::Prompt);
    document.records[1].workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::None,
        project_root: String::new(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    });
    for key in ["codex#a", "claude#chat"] {
        document.begin_turn(key, "first", None, NOW).unwrap();
        assert!(!document.wants_generated_title(key), "still running");
        document.run_ended(key, NOW + 1, RunOutcome::Failed);
        assert!(!document.wants_generated_title(key), "failed");
        document.begin_turn(key, "again", None, NOW + 2).unwrap();
        document.run_ended(key, NOW + 3, RunOutcome::Completed);
    }
    assert!(document.wants_generated_title("codex#a"));
    assert!(
        document.wants_generated_title("claude#chat"),
        "a chat with no project is named too"
    );
    document.settings.generated_titles = false;
    assert!(
        !document.wants_generated_title("codex#a"),
        "the setting is off"
    );
    document.settings.generated_titles = true;

    // A second completed turn is not the first.
    document
        .begin_turn("codex#a", "more", None, NOW + 4)
        .unwrap();
    document.run_ended("codex#a", NOW + 5, RunOutcome::Completed);
    assert!(!document.wants_generated_title("codex#a"));
    document.records[0].turns.pop();

    // Only a title still seeded from the prompt is replaced.
    for source in [TitleSource::Generated, TitleSource::User] {
        assert!(document.set_title_source("codex#a", source));
        assert!(!document.set_title_source("codex#a", source), "unchanged");
        assert!(!document.wants_generated_title("codex#a"), "{source:?}");
    }
    assert!(!document.set_title_source("codex#unknown", TitleSource::User));
    assert!(!document.wants_generated_title("codex#unknown"));
}

#[test]
fn a_generated_title_is_wanted_as_the_first_prompt_goes_out_for_any_thread() {
    let mut document = document(&["codex#project", "claude#chat", "codex#owned"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[1].workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::None,
        project_root: String::new(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    });
    document.records[0].title_source = Some(TitleSource::Prompt);
    document.records[1].title_source = Some(TitleSource::Prompt);
    document.records[2].title_source = Some(TitleSource::User);
    assert!(
        document.settings.generated_titles,
        "generated titles are on by default"
    );
    for key in ["codex#project", "claude#chat", "codex#owned"] {
        assert!(!document.wants_title_at_first_prompt(key), "no prompt yet");
        document.begin_turn(key, "first", None, NOW).unwrap();
    }
    // At once, before the run ends; a chat with no project included.
    assert!(document.wants_title_at_first_prompt("codex#project"));
    assert!(document.wants_title_at_first_prompt("claude#chat"));
    assert!(
        !document.wants_title_at_first_prompt("codex#owned"),
        "the owner's title is never replaced"
    );
    document.settings.generated_titles = false;
    assert!(
        !document.wants_title_at_first_prompt("codex#project"),
        "off"
    );
    document.settings.generated_titles = true;

    // Once the first turn ends, the next prompt is not the first.
    document.run_ended("codex#project", NOW + 1, RunOutcome::Completed);
    assert!(!document.wants_title_at_first_prompt("codex#project"));
    document
        .begin_turn("codex#project", "second", None, NOW + 2)
        .unwrap();
    assert!(!document.wants_title_at_first_prompt("codex#project"));

    // A generated title is never asked for again.
    assert!(document.set_title_source("claude#chat", TitleSource::Generated));
    assert!(!document.wants_title_at_first_prompt("claude#chat"));
}

/// Protocol 71: "Pane tools without asking" lives beside the store, never in
/// it, so the store's bytes are the same whichever way it is set and an older
/// build still reads it. Missing or anything but `on` reads as off.
#[test]
fn the_pane_tools_switch_lives_beside_the_store_and_anything_but_on_means_ask() {
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join(THREADS_FILE_NAME);
    let marker = state.path().join(PANE_TOOLS_MARKER);
    let mut store = ThreadStore::open(&file, NOW).unwrap();
    store.bump(NOW).unwrap();
    let before = std::fs::read(&file).unwrap();
    assert!(!store.pane_tools_without_asking());
    assert!(!pane_tools_without_asking_at(state.path()));

    store.set_pane_tools_without_asking(true).unwrap();
    assert!(store.pane_tools_without_asking());
    assert!(pane_tools_without_asking_at(state.path()));
    assert_eq!(std::fs::read(&marker).unwrap(), PANE_TOOLS_ON);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "the store is untouched"
    );

    store.set_pane_tools_without_asking(false).unwrap();
    assert!(!store.pane_tools_without_asking());
    assert!(!pane_tools_without_asking_at(state.path()));
    assert_eq!(std::fs::read(&file).unwrap(), before);

    // The document never carries it, whatever Settings sends.
    let mut settings = store.document().settings.clone();
    settings.pane_tools_without_asking = true;
    store
        .apply(NOW + 1, |document| document.set_settings(settings))
        .unwrap();
    assert!(!store.document().settings.pane_tools_without_asking);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(written["settings"].get("paneToolsWithoutAsking").is_none());
    drop(store);

    // Anything but exactly `on`, or an unreadable marker, is off.
    write_private(&marker, b"on");
    assert!(!pane_tools_without_asking_at(state.path()));
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(state.path().join("elsewhere"), &marker).unwrap();
    std::fs::write(state.path().join("elsewhere"), PANE_TOOLS_ON).unwrap();
    assert!(!pane_tools_without_asking_at(state.path()));
    assert!(!pane_tools_without_asking_at(&state.path().join("missing")));
}

#[test]
fn generated_titles_are_turned_on_once_and_an_owners_later_off_stays_off() {
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join(THREADS_FILE_NAME);
    let marker = state.path().join(GENERATED_TITLES_ON_MARKER);
    // A store written while titles were off by default holds that `false`.
    let mut old = ThreadsDocument {
        revision: 7,
        ..ThreadsDocument::default()
    };
    old.settings.generated_titles = false;
    write_private(&file, &serde_json::to_vec(&old).unwrap());

    let mut store = ThreadStore::open(&file, NOW).unwrap();
    assert!(!store.document().settings.generated_titles);
    assert!(store.turn_generated_titles_on_once(NOW).unwrap());
    assert!(store.document().settings.generated_titles);
    assert!(marker.exists());
    assert!(
        !store.turn_generated_titles_on_once(NOW).unwrap(),
        "it runs once"
    );
    // The owner turns them off; the next launch leaves that alone.
    store
        .apply(NOW + 1, |document| {
            document.settings.generated_titles = false;
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = ThreadStore::open(&file, NOW + 2).unwrap();
    assert!(!store.turn_generated_titles_on_once(NOW + 2).unwrap());
    assert!(!store.document().settings.generated_titles, "still off");
    drop(store);
    // The document keeps its accepted shape: only the setting changed.
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written["version"], THREADS_DOCUMENT_VERSION);
    assert_eq!(written["settings"]["generatedTitles"], false);

    // A store that already has them on is left alone, and marked.
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join(THREADS_FILE_NAME);
    write_private(
        &file,
        &serde_json::to_vec(&ThreadsDocument::default()).unwrap(),
    );
    let mut store = ThreadStore::open(&file, NOW).unwrap();
    let revision = store.document().revision;
    assert!(!store.turn_generated_titles_on_once(NOW).unwrap());
    assert!(store.document().settings.generated_titles);
    assert_eq!(store.document().revision, revision, "nothing written");
    assert!(state.path().join(GENERATED_TITLES_ON_MARKER).exists());

    // A new store starts with them on.
    let state = tempfile::tempdir().unwrap();
    let mut store = ThreadStore::open(&state.path().join(THREADS_FILE_NAME), NOW).unwrap();
    assert!(store.document().settings.generated_titles);
    assert!(!store.turn_generated_titles_on_once(NOW).unwrap());
}

#[test]
fn turns_keep_the_newest_two_hundred_and_full_stores_evict_only_missing_records() {
    let mut document = document(&["claude#a"]);
    for offset in 0..205_u64 {
        document
            .begin_turn("claude#a", "again", None, NOW + offset)
            .unwrap();
    }
    document.normalize(NOW);
    let turns = &document.record("claude#a").unwrap().turns;
    assert_eq!(turns.len(), MAX_TURNS);
    assert_eq!(turns[0].ordinal, 5);

    // Full: claude#0 and claude#1 are missing from the registry (0 for
    // longer), claude#2 is settled but listed.
    let mut full = ThreadsDocument::default();
    for index in 0..MAX_RECORDS {
        let mut record = ThreadRecord::new(format!("claude#{index}"), NOW);
        match index {
            0 => record.missing_since = Some(NOW - 2 * HOUR),
            1 => record.missing_since = Some(NOW - HOUR),
            2 => record.settle(SettledReason::External, NOW - DAY_MS),
            _ => {}
        }
        full.records.push(record);
    }
    full.seeded = true;
    let sessions = (2..MAX_RECORDS)
        .map(|index| session(&format!("claude#{index}"), SeedOrigin::App))
        .chain([
            session("claude#new", SeedOrigin::App),
            session("codex#cli", SeedOrigin::Store),
        ])
        .collect::<Vec<_>>();
    full.reconcile(&sessions, |_| false, true, NOW);
    assert_eq!(full.records.len(), MAX_RECORDS);
    assert!(full.record("claude#new").is_some(), "an active seed evicts");
    assert!(
        full.record("claude#0").is_none(),
        "the longest missing goes"
    );
    assert!(full.record("claude#1").is_some());
    assert!(
        full.record("claude#2").is_some(),
        "a listed record is never evicted, settled or not: it would seed again as active"
    );
    assert!(
        full.record("codex#cli").is_none(),
        "a settled seed never evicts"
    );

    // With nothing missing, nothing is evicted and a new record is refused.
    for record in &mut full.records {
        record.missing_since = None;
    }
    let refused = full
        .bind(
            ThreadBind {
                key: "claude#refused".into(),
                prompt: "hi".into(),
                ..ThreadBind::default()
            },
            NOW,
        )
        .unwrap_err();
    assert_eq!(refused.code, THREADS_FULL_CODE);
}

/// 6 Oct 2026 (Threads review): a registry rebuilt from provider listings
/// (its cache lost) dropped every thread whose provider had not listed yet,
/// and released their worktrees.
#[test]
fn a_missing_session_keeps_its_record_for_a_day_and_an_incomplete_registry_marks_nothing() {
    let mut document = document(&["claude#here", "claude#away"]);
    let here = [session("claude#here", SeedOrigin::App)];
    let both = [
        session("claude#here", SeedOrigin::App),
        session("claude#away", SeedOrigin::App),
    ];
    // Rebuilding: an absent session proves nothing.
    let reconciled = document.reconcile(&here, |_| false, false, NOW);
    assert!(reconciled.dropped.is_empty());
    assert_eq!(document.record("claude#away").unwrap().missing_since, None);
    // Complete: missing from now, kept for a day.
    document.reconcile(&here, |_| false, true, NOW);
    let reconciled = document.reconcile(&here, |_| false, true, NOW + DAY_MS - 1);
    assert!(reconciled.dropped.is_empty());
    assert_eq!(
        document.record("claude#away").unwrap().missing_since,
        Some(NOW)
    );
    // Back in the registry: no longer missing, and not seeded again.
    document.reconcile(&both, |_| false, true, NOW + DAY_MS);
    assert_eq!(document.record("claude#away").unwrap().missing_since, None);
    assert_eq!(document.records.len(), 2);
    // Missing again for a whole day: dropped (its worktree is the host's
    // business, and the host keeps it).
    document.reconcile(&here, |_| false, true, NOW + 2 * DAY_MS);
    let reconciled = document.reconcile(&here, |_| false, true, NOW + 3 * DAY_MS);
    assert_eq!(
        reconciled
            .dropped
            .iter()
            .map(|record| record.key.as_str())
            .collect::<Vec<_>>(),
        ["claude#away"]
    );
    assert!(reconciled.to_release.is_empty());
    assert!(document.record("claude#away").is_none());
    assert!(document_valid(&document));
}

/// 6 Oct 2026 (Threads review): a write refused at the byte cap dropped
/// whole settled threads' turns first, and the record cap evicted settled
/// records whose sessions were still listed, which then seeded again as new
/// active threads.
#[test]
fn making_room_trims_the_idlest_turns_then_evicts_missing_records_never_listed_ones() {
    let mut document = document(&["claude#idle", "claude#missing", "claude#recent"]);
    let fill = |document: &mut ThreadsDocument, key: &str, count: u32, at: u64| {
        let index = document.index(key).unwrap();
        for ordinal in 0..count {
            document.records[index].turns.push(TurnRecord::new(
                ordinal,
                &format!("{key} {ordinal}"),
                at + u64::from(ordinal),
                None,
            ));
        }
    };
    fill(&mut document, "claude#idle", 50, NOW - 3 * DAY_MS);
    fill(&mut document, "claude#missing", 30, NOW - 2 * DAY_MS);
    fill(&mut document, "claude#recent", 100, NOW);
    document.records[1].missing_since = Some(NOW - HOUR);
    let turns = |document: &ThreadsDocument, key: &str| document.record(key).unwrap().turns.len();

    // A little room: only the idlest record is cut, to its newest 20.
    assert!(document.make_room(1));
    assert_eq!(turns(&document, "claude#idle"), TRIM_FLOOR);
    assert_eq!(
        document.record("claude#idle").unwrap().turns[0].ordinal,
        30,
        "the newest turns stay"
    );
    assert_eq!(turns(&document, "claude#missing"), 30);
    assert_eq!(turns(&document, "claude#recent"), 100);

    // All the room there is: every record to 20, the missing record goes,
    // then the listed records lose their turns but stay.
    assert!(document.make_room(usize::MAX));
    assert!(document.record("claude#missing").is_none());
    assert_eq!(turns(&document, "claude#idle"), 0);
    assert_eq!(turns(&document, "claude#recent"), 0);
    assert_eq!(document.records.len(), 2);
    assert!(!document.make_room(usize::MAX), "nothing left to give");
}

#[test]
fn a_store_over_the_byte_cap_commits_after_trimming_turns() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join(THREADS_FILE_NAME);
    let mut store = ThreadStore::open(&file, NOW).unwrap();
    let long = "x".repeat(PREVIEW_CHARS);
    let ((), committed) = store
        .apply(NOW, |document| {
            for index in 0..80_u32 {
                let mut record = ThreadRecord::new(format!("claude#{index}"), NOW);
                for ordinal in 0..u32::try_from(MAX_TURNS).unwrap() {
                    record.turns.push(TurnRecord::new(
                        ordinal,
                        &format!("{long} {index} {ordinal}"),
                        NOW + u64::from(index),
                        None,
                    ));
                }
                document.records.push(record);
            }
            Ok(())
        })
        .unwrap();
    assert!(committed);
    let document = store.document();
    assert_eq!(document.records.len(), 80, "no listed record is dropped");
    assert!(
        document
            .records
            .iter()
            .any(|record| record.turns.len() == TRIM_FLOOR),
        "the idlest threads were cut to their newest turns"
    );
    assert_eq!(
        document.record("claude#79").unwrap().turns.len(),
        MAX_TURNS,
        "the most recent thread keeps its turns"
    );
    assert!(std::fs::metadata(&file).unwrap().len() <= 4 * 1024 * 1024);
}

#[test]
fn settings_and_prepared_bounds_refuse_without_changes() {
    let mut document = ThreadsDocument::default();
    let mut settings = ThreadSettings {
        auto_settle_days: MAX_AUTO_SETTLE_DAYS + 1,
        ..ThreadSettings::default()
    };
    assert_eq!(
        document.set_settings(settings.clone()).unwrap_err().code,
        THREADS_INVALID_CODE
    );
    settings.auto_settle_days = 0;
    document.set_settings(settings).unwrap();
    assert_eq!(document.settings.auto_settle_days, 0);
    for index in 0..MAX_PREPARED {
        document
            .add_prepared(prepared(&format!("prep-{index}")))
            .unwrap();
    }
    assert_eq!(
        document.ensure_prepared_room().unwrap_err().code,
        THREADS_PREPARED_FULL_CODE
    );
    assert_eq!(
        document.add_prepared(prepared("prep-x")).unwrap_err().code,
        THREADS_PREPARED_FULL_CODE
    );
    document.prepared[0].created_at = NOW - 2 * HOUR;
    assert_eq!(document.stale_prepared(NOW), ["prep-0"]);
    assert_eq!(
        document.prepared.len(),
        MAX_PREPARED,
        "only named, not taken"
    );
}

#[test]
fn store_commits_revisions_reopens_and_refuses_a_second_writer() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join(THREADS_FILE_NAME);
    // A fresh store starts its revision at the time it opened, so a view
    // from before it (a quarantined file's) is older.
    let mut store = ThreadStore::open(&file, NOW).unwrap();
    assert!(!store.quarantined());
    assert_eq!(store.document().revision, NOW);
    assert!(ThreadStore::open(&file, NOW).is_err(), "one writer");
    let ((), committed) = store
        .apply(NOW, |document| {
            document.records.push(ThreadRecord::new("claude#a", NOW));
            Ok(())
        })
        .unwrap();
    assert!(committed);
    assert_eq!(store.document().revision, NOW + 1);
    let ((), committed) = store.apply(NOW, |_| Ok(())).unwrap();
    assert!(!committed, "nothing changed, nothing written");
    assert_eq!(store.document().revision, NOW + 1);
    let refused = store
        .apply(NOW, |document| document.pin("claude#none", NOW))
        .unwrap_err();
    assert!(
        matches!(refused, ThreadStoreError::Refused(ref refusal) if refusal.code == THREADS_UNKNOWN_CODE)
    );
    store.bump(NOW).unwrap();
    assert_eq!(store.document().revision, NOW + 2);
    drop(store);
    let store = ThreadStore::open(&file, NOW + DAY_MS).unwrap();
    assert_eq!(
        store.document().revision,
        NOW + 2,
        "a stored revision is kept"
    );
    assert_eq!(store.document().records.len(), 1);
}

#[test]
fn corrupt_unknown_and_out_of_bounds_files_are_quarantined_and_start_empty() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join(THREADS_FILE_NAME);
    let mut unknown_version = serde_json::to_value(ThreadsDocument::default()).unwrap();
    unknown_version["version"] = 6.into();
    // A version 2 document belongs in `threads.v2.json`, never in this file.
    let mut legacy_here = serde_json::to_value(ThreadsDocument::default()).unwrap();
    legacy_here["version"] = 2.into();
    let mut bad_key = serde_json::to_value(document(&["claude#a"])).unwrap();
    bad_key["records"][0]["activeOrder"] = "not a key!".into();
    for bytes in [
        b"{".to_vec(),
        b"{\"version\":3,\"extra\":true}".to_vec(),
        serde_json::to_vec(&unknown_version).unwrap(),
        serde_json::to_vec(&legacy_here).unwrap(),
        serde_json::to_vec(&bad_key).unwrap(),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = ThreadStore::open(&file, NOW).unwrap();
        assert!(store.quarantined(), "{}", String::from_utf8_lossy(&bytes));
        assert_eq!(
            *store.document(),
            ThreadsDocument {
                revision: NOW,
                ..ThreadsDocument::default()
            }
        );
        assert!(!file.exists(), "moved aside");
    }
    let quarantined = std::fs::read_dir(temp.path().join(".quarantine"))
        .unwrap()
        .count();
    assert_eq!(quarantined, 5);
}

/// Store v2: a finished turn keeps when its run ended, the files its own
/// edits named and whether others shared its checkout; a turn in a shared
/// checkout diffs only those files, and refuses when they are unknown.
#[test]
fn a_turn_keeps_its_end_its_files_and_whether_its_checkout_was_shared() {
    let mut document = document(&["claude#local", "claude#chat"]);
    document.records[0].workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo/app".into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    });
    document.records[0].git_root = Some("/repo".into());
    let start = turn_ref("claude#local", 0, false);
    document
        .begin_turn("claude#local", "first", Some(start.clone()), NOW)
        .unwrap();
    let finished = document
        .run_ended("claude#local", NOW + 41_000, RunOutcome::Completed)
        .unwrap();
    assert_eq!(finished.dir, "/repo/app");
    assert_eq!(finished.start_ref.as_deref(), Some(start.as_str()));
    let turn = &document.record("claude#local").unwrap().turns[0];
    assert_eq!(turn.ended_at, Some(NOW + 41_000));
    assert_eq!((turn.touched.as_ref(), turn.shared), (None, false));
    assert_eq!(turn.diff_paths(), Ok(None), "its whole checkout until told");

    document.record_turn_end(
        "claude#local",
        0,
        TurnEnd {
            touched: Some(vec![
                "src/b.ts".into(),
                "src/a.ts".into(),
                "src/b.ts".into(),
            ]),
            shared: true,
            ..TurnEnd::default()
        },
    );
    let turn = &document.record("claude#local").unwrap().turns[0];
    assert_eq!(
        turn.touched.as_deref(),
        Some(&["src/a.ts".to_owned(), "src/b.ts".to_owned()][..])
    );
    assert!(turn.shared);
    assert_eq!(
        turn.diff_paths().unwrap(),
        Some(&["src/a.ts".to_owned(), "src/b.ts".to_owned()][..])
    );
    let view = document.view(NOW + 42_000, |_| false);
    let local = view
        .threads
        .iter()
        .find(|thread| thread.key == "claude#local");
    assert_eq!(
        local.unwrap().turns[0].ended_at,
        Some(NOW + 41_000),
        "the view carries the end"
    );

    // Files that are not repository-relative, or too many, are unknown;
    // then a shared turn's own changes cannot be told apart.
    let too_many = (0..=MAX_TURN_PATHS)
        .map(|index| format!("src/{index}.ts"))
        .collect::<Vec<_>>();
    for touched in [
        Some(vec!["/repo/app/src/a.ts".to_owned()]),
        Some(vec!["../escape".to_owned()]),
        Some(vec!["src/./a.ts".to_owned()]),
        Some(too_many),
        Some(Vec::new()),
        None,
    ] {
        document.record_turn_end(
            "claude#local",
            0,
            TurnEnd {
                touched: touched.clone(),
                shared: true,
                ..TurnEnd::default()
            },
        );
        let turn = &document.record("claude#local").unwrap().turns[0];
        assert!(
            turn.touched.as_ref().is_none_or(Vec::is_empty),
            "{touched:?}"
        );
        assert_eq!(
            turn.diff_paths().unwrap_err().code,
            THREADS_TURN_FILES_UNKNOWN_CODE,
            "{touched:?}"
        );
    }
    document.record_turn_end("claude#local", 0, TurnEnd::default());
    assert_eq!(
        document.record("claude#local").unwrap().turns[0].diff_paths(),
        Ok(None),
        "nobody else in the checkout: the whole checkout is its own"
    );
    assert!(document_valid(&ThreadsDocument {
        version: THREADS_DOCUMENT_VERSION,
        ..document.clone()
    }));
}

/// A turn's end is stamped when its run ends, never invented: a git turn
/// without a start checkpoint still answers for its files, a newer send
/// closes a running turn with no end, and reconcile's interrupt stamps one.
#[test]
fn turn_ends_are_stamped_by_their_run_and_never_invented() {
    let mut document = document(&["claude#local", "claude#chat"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    document
        .begin_turn("claude#local", "first", None, NOW)
        .unwrap();
    document.run_ended("claude#local", NOW + 1_000, RunOutcome::Completed);

    // A git turn without a start checkpoint still answers, for its files;
    // a plain chat's turn ends with nothing to capture.
    document
        .begin_turn("claude#local", "second", None, NOW + 50_000)
        .unwrap();
    let finished = document
        .run_ended("claude#local", NOW + 51_000, RunOutcome::Failed)
        .unwrap();
    assert_eq!((finished.ordinal, finished.start_ref), (1, None));
    document.begin_turn("claude#chat", "hi", None, NOW).unwrap();
    assert!(
        document
            .run_ended("claude#chat", NOW + 2_000, RunOutcome::Completed)
            .is_none()
    );
    assert_eq!(
        document.record("claude#chat").unwrap().turns[0].ended_at,
        Some(NOW + 2_000)
    );

    // A newer send closes a still-running turn without inventing its end;
    // a run reconcile finds gone ends its turn when it ends the run.
    document
        .begin_turn("claude#chat", "two", None, NOW + 3_000)
        .unwrap();
    document
        .begin_turn("claude#chat", "three", None, NOW + 4_000)
        .unwrap();
    let record = document.record("claude#chat").unwrap();
    assert_eq!(record.turns[1].state, TurnState::Done);
    assert_eq!(record.turns[1].ended_at, None);
    document.reconcile(
        &[
            session("claude#chat", SeedOrigin::App),
            session("claude#local", SeedOrigin::App),
        ],
        |_| false,
        true,
        NOW + 9_000,
    );
    let record = document.record("claude#chat").unwrap();
    assert_eq!(record.turns[2].state, TurnState::Interrupted);
    assert_eq!(record.turns[2].ended_at, Some(NOW + 9_000));
}

/// A store v1 turn and a store v2 turn, byte for byte: the v2 fields are
/// absent on a v1 turn (unknown, never invented) and named on a v2 one.
#[test]
fn turn_records_have_golden_json_for_store_v1_and_v2() {
    let v1 = r#"{"ordinal":0,"sentAt":1791300000000,"promptDigest":"971ad67d98611a46","state":"done","startRef":"refs/interface/threads/0123456789abcdef/0/start","filesChanged":2,"added":14,"removed":3,"preview":"Fix the login test","previewTruncated":false}"#;
    let turn: TurnRecord = serde_json::from_str(v1).unwrap();
    assert_eq!(
        (turn.ended_at, turn.touched.as_ref(), turn.shared),
        (None, None, false)
    );
    assert_eq!(
        serde_json::to_string(&turn).unwrap(),
        v1,
        "a v1 turn writes back unchanged"
    );

    let v2 = r#"{"ordinal":1,"sentAt":1791300050000,"promptDigest":"6def2534bbfcef51","state":"done","startRef":"refs/interface/threads/0123456789abcdef/1/start","endRef":"refs/interface/threads/0123456789abcdef/1/end","filesChanged":1,"added":2,"removed":8,"preview":"Rewrite the session","previewTruncated":false,"endedAt":1791300091000,"touched":["src/lib/session.ts","src/a.ts"],"shared":true}"#;
    let turn: TurnRecord = serde_json::from_str(v2).unwrap();
    assert_eq!(turn.ended_at, Some(1_791_300_091_000));
    assert!(turn.shared);
    assert_eq!(serde_json::to_string(&turn).unwrap(), v2);
    let mut unsorted = ThreadsDocument::default();
    let mut record = ThreadRecord::new("claude#a", NOW);
    record.turns.push(turn);
    unsorted.records.push(record);
    assert!(
        !document_valid(&unsorted),
        "a stored list of files is sorted, so it compares as written"
    );
    unsorted.records[0].turns[0].touched =
        Some(vec!["src/a.ts".into(), "src/lib/session.ts".into()]);
    assert!(document_valid(&unsorted));
    unsorted.records[0].turns[0].state = TurnState::Running;
    assert!(!document_valid(&unsorted), "a running turn has not ended");
}

const LEGACY_V1: &str = r#"{"version":1,"revision":7,"seeded":true,"settings":{"autoSettleDays":3,"settleOnMerge":true,"generatedTitles":false,"trackPullRequests":true},"records":[{"key":"claude#a","createdAt":1791290000000,"activeOrder":"V","autoSettleDisabled":false,"markedUnread":false,"lastRun":{"startedAt":1791300000000,"endedAt":1791300041000,"outcome":"completed"},"turns":[{"ordinal":0,"sentAt":1791300000000,"promptDigest":"971ad67d98611a46","state":"done","preview":"Fix the login test","previewTruncated":false}]}],"prepared":[]}"#;

const MIGRATED_V3: &str = r#"{
  "version": 3,
  "revision": 8,
  "seeded": true,
  "settings": {
    "autoSettleDays": 3,
    "settleOnMerge": true,
    "generatedTitles": false,
    "trackPullRequests": true
  },
  "records": [
    {
      "key": "claude#a",
      "createdAt": 1791290000000,
      "activeOrder": "V",
      "autoSettleDisabled": false,
      "markedUnread": false,
      "lastRun": {
        "startedAt": 1791300000000,
        "endedAt": 1791300041000,
        "outcome": "completed"
      },
      "turns": [
        {
          "ordinal": 0,
          "sentAt": 1791300000000,
          "promptDigest": "971ad67d98611a46",
          "state": "done",
          "preview": "Fix the login test",
          "previewTruncated": false
        }
      ]
    }
  ],
  "prepared": []
}
"#;

/// The carried-over file as this store writes it now (store v5).
fn migrated() -> String {
    MIGRATED_V3.replace(r#""version": 3"#, r#""version": 5"#)
}

fn write_private(file: &std::path::Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(file, bytes).unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// The v1 store is read once, never changed, and carried over to
/// `threads.v5.json` as version 5 (store v5, protocol 59) with its turns'
/// newer fields absent; after that only the v5 file is read.
#[test]
fn a_v1_store_is_carried_over_once_and_left_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let legacy = temp.path().join(THREADS_LEGACY_FILE_NAME);
    let file = temp.path().join(THREADS_FILE_NAME);
    write_private(&legacy, LEGACY_V1.as_bytes());

    let store = ThreadStore::open(&file, NOW).unwrap();
    assert!(!store.quarantined());
    let document = store.document();
    assert_eq!((document.version, document.revision), (5, 8));
    let turn = &document.record("claude#a").unwrap().turns[0];
    assert_eq!(
        (turn.ended_at, turn.touched.as_ref(), turn.shared),
        (None, None, false)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), migrated());
    assert_eq!(
        std::fs::read_to_string(&legacy).unwrap(),
        LEGACY_V1,
        "never changed"
    );
    drop(store);

    // Once carried over, the v1 file is not read again.
    write_private(
        &legacy,
        LEGACY_V1.replace("claude#a", "claude#b").as_bytes(),
    );
    let store = ThreadStore::open(&file, NOW).unwrap();
    assert!(store.document().record("claude#a").is_some());
    assert!(store.document().record("claude#b").is_none());
}

/// A legacy file that does not decode, is not version 1 or already holds
/// version 2's fields is left alone for the build that wrote it; the store
/// starts empty and nothing is quarantined.
#[test]
fn an_unusable_v1_store_is_left_alone_and_the_store_starts_empty() {
    use std::os::unix::fs::PermissionsExt;

    let with_end = LEGACY_V1.replace(
        r#""previewTruncated":false}"#,
        r#""previewTruncated":false,"endedAt":1791300041000}"#,
    );
    assert_ne!(with_end, LEGACY_V1);
    for bytes in [
        "{".to_owned(),
        LEGACY_V1.replace(r#""version":1"#, r#""version":2"#),
        LEGACY_V1.replace(r#""seeded":true"#, r#""seeded":true,"extra":1"#),
        with_end,
    ] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let legacy = temp.path().join(THREADS_LEGACY_FILE_NAME);
        write_private(&legacy, bytes.as_bytes());
        let store = ThreadStore::open(&temp.path().join(THREADS_FILE_NAME), NOW).unwrap();
        assert!(!store.quarantined(), "{bytes}");
        assert_eq!(
            *store.document(),
            ThreadsDocument {
                revision: NOW,
                ..ThreadsDocument::default()
            },
            "{bytes}"
        );
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), bytes);
        assert!(!temp.path().join(".quarantine").exists());
    }
}

/// Store v3 (protocol 46): a v2 store is read once, never changed, and
/// carried over (now to `threads.v5.json` as version 5); a v1 file beside it is
/// older news and is not read. An unusable v2 file is left for the build
/// that wrote it and the store starts empty, never falling back to v1.
#[test]
fn a_v2_store_is_carried_over_once_and_wins_over_v1() {
    use std::os::unix::fs::PermissionsExt;

    let v2 = MIGRATED_V3.replace(r#""version": 3"#, r#""version": 2"#);
    for (v2_bytes, carried) in [(v2.clone(), true), ("{".to_owned(), false)] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let older = temp.path().join(THREADS_V2_FILE_NAME);
        let legacy = temp.path().join(THREADS_LEGACY_FILE_NAME);
        let file = temp.path().join(THREADS_FILE_NAME);
        write_private(&older, v2_bytes.as_bytes());
        write_private(
            &legacy,
            LEGACY_V1.replace("claude#a", "claude#v1").as_bytes(),
        );

        let store = ThreadStore::open(&file, NOW).unwrap();
        assert!(!store.quarantined());
        let document = store.document();
        assert!(document.record("claude#v1").is_none(), "v1 is never read");
        if carried {
            assert_eq!((document.version, document.revision), (5, 9));
            assert!(document.record("claude#a").is_some());
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                migrated().replace(r#""revision": 8"#, r#""revision": 9"#)
            );
        } else {
            assert_eq!(
                *document,
                ThreadsDocument {
                    revision: NOW,
                    ..ThreadsDocument::default()
                }
            );
        }
        assert_eq!(
            std::fs::read_to_string(&older).unwrap(),
            v2_bytes,
            "never changed"
        );
    }
}

/// Store v4 (protocol 57, team tools): a v3 store is read once, never
/// changed, and carried over (now to `threads.v5.json` as version 5), winning over
/// a v2 file beside it; its threads keep their lineage and team tools read
/// as on. A v3 file holding a `child` lineage or team tools off (version 4's
/// fields) is left for the build that wrote it and the store starts empty.
#[test]
fn a_v3_store_is_carried_over_once_and_wins_over_v2() {
    use std::os::unix::fs::PermissionsExt;

    let handoff = r#""lineage": {
        "parent": "claude#p",
        "relation": "handoff",
        "keptTurns": 0,
        "at": 1791290000000
      },
      "turns": ["#;
    let v3 = MIGRATED_V3.replace(r#""turns": ["#, handoff);
    assert_ne!(v3, MIGRATED_V3);
    let child = v3.replace(r#""relation": "handoff""#, r#""relation": "child""#);
    let team_off = v3.replace(
        r#""trackPullRequests": true"#,
        r#""trackPullRequests": true,
    "teamTools": false"#,
    );
    for (v3_bytes, carried) in [(v3.clone(), true), (child, false), (team_off, false)] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let older = temp.path().join(THREADS_V3_FILE_NAME);
        let v2 = temp.path().join(THREADS_V2_FILE_NAME);
        let file = temp.path().join(THREADS_FILE_NAME);
        write_private(&older, v3_bytes.as_bytes());
        write_private(
            &v2,
            MIGRATED_V3
                .replace(r#""version": 3"#, r#""version": 2"#)
                .replace("claude#a", "claude#v2")
                .as_bytes(),
        );
        let store = ThreadStore::open(&file, NOW).unwrap();
        assert!(!store.quarantined());
        let document = store.document();
        assert!(document.record("claude#v2").is_none(), "v2 is never read");
        if carried {
            assert_eq!((document.version, document.revision), (5, 9));
            let record = document.record("claude#a").unwrap();
            assert_eq!(
                record.lineage.as_ref().map(|lineage| lineage.relation),
                Some(LineageRelation::Handoff)
            );
            assert!(document.settings.team_tools);
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                v3.replace(r#""version": 3"#, r#""version": 5"#)
                    .replace(r#""revision": 8"#, r#""revision": 9"#)
            );
        } else {
            assert_eq!(
                *document,
                ThreadsDocument {
                    revision: NOW,
                    ..ThreadsDocument::default()
                },
                "{v3_bytes}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&older).unwrap(),
            v3_bytes,
            "never changed"
        );
        drop(store);
        // Once carried over, the v3 file is not read again.
        if carried {
            write_private(&older, v3.replace("claude#a", "claude#again").as_bytes());
            let store = ThreadStore::open(&file, NOW).unwrap();
            assert!(store.document().record("claude#again").is_none());
        }
    }
}

/// Store v4 (protocol 57): a thread team tools started is bound with no turn
/// and its prepared `child` lineage, stamped at the bind; only a version 4
/// document holds one, and team tools off is version 4's too.
#[test]
fn a_started_thread_binds_with_its_child_lineage_and_only_v4_holds_one() {
    let mut document = document(&["claude#parent"]);
    let mut prepared = prepared("prep-child");
    prepared.lineage = Some(ThreadLineage {
        parent: "claude#parent".into(),
        relation: LineageRelation::Child,
        kept_turns: 0,
        at: 0,
    });
    document.add_prepared(prepared).unwrap();
    let mut v3 = document.clone();
    v3.version = THREADS_V3_VERSION;
    assert!(!v3_document_valid(&v3), "a prepared child is v4 only");
    assert!(document_valid(&document));
    document
        .bind_empty("codex#child", "prep-child", TitleSource::Prompt, NOW + 5)
        .unwrap();
    let record = document.record("codex#child").unwrap();
    assert_eq!(
        record.lineage,
        Some(ThreadLineage {
            parent: "claude#parent".into(),
            relation: LineageRelation::Child,
            kept_turns: 0,
            at: NOW + 5,
        })
    );
    assert!(record.turns.is_empty() && record.last_run.is_none());
    assert!(document_valid(&document));
    let mut v3 = document.clone();
    v3.version = THREADS_V3_VERSION;
    assert!(!v3_document_valid(&v3), "a child lineage is v4 only");
    let mut off = ThreadsDocument::default();
    off.settings.team_tools = false;
    assert!(document_valid(&off));
    off.version = THREADS_V3_VERSION;
    assert!(!v3_document_valid(&off), "team tools off is v4 only");
    off.settings.team_tools = true;
    assert!(v3_document_valid(&off));
}

/// Store v3: a workspace Continue with prepared hands its `handoff` lineage
/// to the thread bound to it, stamped at the bind; only a version 3
/// document may hold one.
#[test]
fn a_handoff_bind_records_its_lineage_and_only_v3_holds_one() {
    let mut document = document(&["claude#parent"]);
    let lineage = ThreadLineage {
        parent: "claude#parent".into(),
        relation: LineageRelation::Handoff,
        kept_turns: 0,
        at: 0,
    };
    document
        .add_prepared(PreparedRecord {
            lineage: Some(lineage.clone()),
            ..prepared("prep-handoff")
        })
        .unwrap();
    let mut v2 = document.clone();
    v2.version = THREADS_V2_VERSION;
    assert!(!v2_document_valid(&v2), "a prepared handoff is v3 only");
    assert!(document_valid(&document));
    let stored = serde_json::to_value(&document.prepared[0]).unwrap();
    assert_eq!(
        stored["lineage"],
        serde_json::json!({"parent": "claude#parent", "relation": "handoff", "keptTurns": 0, "at": 0})
    );
    assert!(
        serde_json::to_value(prepared("prep-plain"))
            .unwrap()
            .get("lineage")
            .is_none(),
        "absent unless set"
    );

    document
        .bind(
            ThreadBind {
                key: "codex#child".into(),
                token: Some("prep-handoff".into()),
                prompt: "You are taking over a thread from Claude.".into(),
                ..ThreadBind::default()
            },
            NOW + 5,
        )
        .unwrap();
    let child = document.record("codex#child").unwrap();
    assert_eq!(
        child.lineage,
        Some(ThreadLineage {
            at: NOW + 5,
            ..lineage
        })
    );
    assert_eq!(child.turns.len(), 1, "the launch prompt is turn 0");
    assert!(document.prepared.is_empty());
    let view = document.view(NOW + 5, |_| false);
    let thread = view
        .threads
        .iter()
        .find(|thread| thread.key == "codex#child")
        .unwrap();
    assert_eq!(
        thread.lineage.as_ref().map(|lineage| lineage.relation),
        Some(LineageRelation::Handoff)
    );
    assert_eq!(thread.section, ThreadSection::Active);

    let mut v2 = document.clone();
    v2.version = THREADS_V2_VERSION;
    assert!(!v2_document_valid(&v2), "a handoff lineage is v3 only");
    assert!(document_valid(&document));
    let mut forked = PreparedRecord {
        lineage: Some(ThreadLineage {
            relation: LineageRelation::Fork,
            ..ThreadLineage {
                parent: "claude#parent".into(),
                relation: LineageRelation::Handoff,
                kept_turns: 0,
                at: 0,
            }
        }),
        ..prepared("prep-fork")
    };
    let mut refused = document.clone();
    refused.prepared.push(forked.clone());
    assert!(
        !document_valid(&refused),
        "a prepared workspace carries only a handoff"
    );
    forked.lineage = None;
    refused.prepared[0] = forked;
    assert!(document_valid(&refused));
}

/// Protocol 37: a worktree thread ships its whole worktree; a Local thread
/// only the files its own finished turns named, and says when some turn's
/// files are unknown. A plain chat or a folder outside git ships nothing.
#[test]
fn ship_scope_covers_a_worktree_whole_and_a_local_thread_by_its_own_files() {
    let mut document = document(&["codex#wt", "claude#local", "claude#chat", "claude#nogit"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    let local = ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo".into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    };
    document.records[1].workspace = Some(local.clone());
    document.records[1].git_root = Some("/repo".into());
    document.records[2].workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::None,
        project_root: String::new(),
        ..local.clone()
    });
    document.records[3].workspace = Some(local);
    for (ordinal, prompt) in ["Fix the login test", "Also the session", "Go"]
        .into_iter()
        .enumerate()
    {
        let ordinal = u32::try_from(ordinal).unwrap();
        for key in ["codex#wt", "claude#local"] {
            let index = document.index(key).unwrap();
            let mut turn = TurnRecord::new(ordinal, prompt, NOW, None);
            turn.state = if ordinal == 2 {
                TurnState::Running
            } else {
                TurnState::Done
            };
            document.records[index].turns.push(turn);
        }
    }
    document.records[1].turns[0].touched = Some(vec!["src/a.ts".into(), "src/b.ts".into()]);
    document.records[1].turns[2].touched = None;

    let wt = document.ship_scope("codex#wt").unwrap();
    assert_eq!((wt.dir.as_str(), wt.worktree), ("/wt/one", true));
    assert_eq!((wt.touched, wt.touched_unknown), (None, false));
    assert_eq!(wt.prompts.len(), 3);

    let local = document.ship_scope("claude#local").unwrap();
    assert_eq!((local.dir.as_str(), local.worktree), ("/repo", false));
    assert_eq!(
        local.touched.unwrap().into_iter().collect::<Vec<_>>(),
        ["src/a.ts", "src/b.ts"]
    );
    assert!(
        local.touched_unknown,
        "turn 1 named no files it can vouch for"
    );
    for key in ["claude#chat", "claude#nogit"] {
        assert_eq!(
            document.ship_scope(key).unwrap_err().code,
            THREADS_NOT_SHIPPABLE_CODE
        );
    }
    assert_eq!(
        document.ship_scope("claude#none").unwrap_err().code,
        THREADS_UNKNOWN_CODE
    );
}

#[test]
fn owning_a_worktree_does_not_attribute_files_without_finished_turn_evidence() {
    let mut document = document(&["codex#wt"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    document.records[0].git_root = Some("/repo".into());
    let mut turn = TurnRecord::new(0, "Edit one file", NOW, None);
    turn.state = TurnState::Done;
    turn.touched = Some(vec!["one.rs".into()]);
    document.records[0].turns.push(turn);
    let scope = document.ship_scope("codex#wt").unwrap();
    assert!(
        scope.touched.is_none(),
        "Legacy worktree commit scope stays whole"
    );
    assert_eq!(scope.attributed, BTreeSet::from(["one.rs".into()]));
}

#[test]
fn a_commit_message_is_the_title_then_the_prompts() {
    let prompts = [
        "Fix the login test\nwith more detail".to_owned(),
        "Also the session".to_owned(),
        "Also the session".to_owned(),
        String::new(),
    ];
    assert_eq!(
        commit_message("Login test fixed", &prompts),
        "Login test fixed\n\n- Fix the login test\n- Also the session"
    );
    assert_eq!(
        commit_message("  ", &prompts[..1]),
        "Fix the login test",
        "no title: the first prompt, said once"
    );
    assert_eq!(commit_message("", &[]), "Changes from Interface");
    let long = "x".repeat(100);
    let subject = commit_message(&long, &[]);
    assert_eq!(subject.chars().count(), COMMIT_SUBJECT_CHARS);
    assert!(subject.ends_with('…'));
}

/// A Local thread whose pull request Interface opened is tracked by that
/// pull request's number; a worktree thread still by its branch.
#[test]
fn a_local_thread_with_a_pull_request_is_tracked_by_its_number() {
    let mut document = document(&["codex#wt", "claude#local", "claude#plain"]);
    document.records[0].workspace = Some(worktree("/wt/one", true));
    let local = ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo".into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    };
    document.records[1].workspace = Some(local.clone());
    document.records[1].git_root = Some("/repo".into());
    document.records[2].workspace = Some(local);
    assert_eq!(
        document.pull_requests_due(NOW, None).len(),
        1,
        "the worktree only"
    );
    document.record_pull_request(
        "claude#local",
        Some(FoundPullRequest {
            url: "https://github.com/owner/atlas/pull/42".into(),
            number: 42,
            state: PullRequestState::Open,
        }),
        NOW,
    );
    let due = document.pull_requests_due(NOW + PULL_REQUEST_INTERVAL_MS, None);
    assert_eq!(
        due.iter()
            .map(|check| (check.key.as_str(), check.project_root.as_str(), &check.by))
            .collect::<Vec<_>>(),
        [
            (
                "codex#wt",
                "/repo",
                &PullRequestLookupBy::Branch("iface/fix-login-3f9a2c".into())
            ),
            ("claude#local", "/repo", &PullRequestLookupBy::Number(42)),
        ]
    );
}

/// A pull request Interface opened for a thread is tracked by its number
/// even while `trackPullRequests` is off; others only while it is on. The
/// flag is store v2's (added before release), never in a v1 document.
#[test]
fn a_pull_request_interface_opened_is_tracked_with_tracking_off() {
    let mut document = document(&["claude#opened", "codex#wt"]);
    let local = ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo".into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    };
    document.records[0].workspace = Some(local);
    document.records[0].git_root = Some("/repo".into());
    document.records[1].workspace = Some(worktree("/wt/one", true));
    document.settings.track_pull_requests = false;
    assert!(document.pull_requests_due(NOW, None).is_empty());
    let found = FoundPullRequest {
        url: "https://github.com/owner/atlas/pull/42".into(),
        number: 42,
        state: PullRequestState::Open,
    };
    document.record_pull_request("codex#wt", Some(found.clone()), NOW);
    document.record_opened_pull_request("claude#opened", found, NOW);
    assert!(
        document
            .record("claude#opened")
            .unwrap()
            .pull_request_tracked
    );
    let due = document.pull_requests_due(NOW + PULL_REQUEST_INTERVAL_MS, None);
    assert_eq!(
        due.iter()
            .map(|check| (check.key.as_str(), &check.by))
            .collect::<Vec<_>>(),
        [("claude#opened", &PullRequestLookupBy::Number(42))],
        "the worktree thread's PR waits for tracking to be on"
    );
    assert_eq!(
        document.pull_requests_due(NOW, Some("claude#opened")).len(),
        1
    );

    let json = serde_json::to_value(document.record("claude#opened").unwrap()).unwrap();
    assert_eq!(json["pullRequestTracked"], true);
    assert!(
        serde_json::to_value(document.record("codex#wt").unwrap())
            .unwrap()
            .get("pullRequestTracked")
            .is_none(),
        "absent unless set"
    );
    let mut legacy = document.clone();
    legacy.version = THREADS_LEGACY_VERSION;
    assert!(!legacy_document_valid(&legacy));
    assert!(document_valid(&ThreadsDocument {
        version: THREADS_DOCUMENT_VERSION,
        ..document
    }));
}
