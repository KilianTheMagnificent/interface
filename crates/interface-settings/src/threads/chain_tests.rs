//! Store v5 (protocol 59): switching a thread's provider.

use std::os::unix::fs::PermissionsExt;

use interface_contracts::threads::{
    LineageRelation, PullRequestState, RunOutcome, SettledReason, ThreadLineage, ThreadPullRequest,
    ThreadRun, ThreadSection, ThreadWorkspace, TitleSource, TurnState, WorkspaceMode,
};

use super::super::*;

const NOW: u64 = 1_791_300_000_000;
const HOUR: u64 = 3_600_000;

fn local() -> ThreadWorkspace {
    ThreadWorkspace {
        mode: WorkspaceMode::Local,
        project_root: "/repo".into(),
        worktree_path: None,
        branch: Some("main".into()),
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    }
}

fn owned_worktree() -> ThreadWorkspace {
    ThreadWorkspace {
        mode: WorkspaceMode::Worktree,
        project_root: "/repo".into(),
        worktree_path: Some("/repo-wt/fix".into()),
        branch: Some("iface/fix".into()),
        base_ref: Some("main".into()),
        owned: true,
        missing: None,
        kept: None,
    }
}

fn finished_turn(ordinal: u32, prompt: &str, at: u64, touched: &[&str]) -> TurnRecord {
    let mut turn = TurnRecord::new(ordinal, prompt, at, Some(turn_ref("k", ordinal, false)));
    turn.state = TurnState::Done;
    turn.ended_at = Some(at + 1_000);
    turn.touched = Some(touched.iter().map(|path| (*path).to_owned()).collect());
    turn
}

/// A thread on Claude with every piece of owner-facing state set, and one
/// other thread above it in Active.
fn project_threads() -> ThreadsDocument {
    let mut document = ThreadsDocument {
        seeded: true,
        ..ThreadsDocument::default()
    };
    let mut other = ThreadRecord::new("codex#other", NOW - 9 * HOUR);
    other.workspace = Some(local());
    other.active_order = Some("A".into());
    let mut thread = ThreadRecord::new("claude#a", NOW - 10 * HOUR);
    thread.active_order = Some("B".into());
    thread.snoozed_until = Some(NOW + HOUR);
    thread.snoozed_at = Some(NOW - HOUR);
    thread.auto_settle_disabled = true;
    thread.last_visited_at = Some(NOW - 2 * HOUR);
    thread.marked_unread = true;
    thread.workspace = Some(owned_worktree());
    thread.git_root = Some("/repo".into());
    thread.lineage = Some(ThreadLineage {
        parent: "codex#other".into(),
        relation: LineageRelation::Child,
        kept_turns: 0,
        at: NOW - 10 * HOUR,
    });
    thread.pull_request = Some(ThreadPullRequest {
        url: "https://github.com/o/r/pull/7".into(),
        number: 7,
        state: PullRequestState::Open,
        checked_at: NOW - HOUR,
    });
    thread.pull_request_checked_at = Some(NOW - HOUR);
    thread.pull_request_tracked = true;
    thread.title_source = Some(TitleSource::User);
    thread.last_run = Some(ThreadRun {
        started_at: NOW - 3 * HOUR,
        ended_at: Some(NOW - 3 * HOUR + 1_000),
        outcome: Some(RunOutcome::Completed),
    });
    thread.turns = vec![finished_turn(
        0,
        "Fix the login retry",
        NOW - 3 * HOUR,
        &["src/a.ts"],
    )];
    document.records = vec![other, thread];
    document
}

fn listed(document: &ThreadsDocument) -> Vec<String> {
    document
        .view(NOW, |_| false)
        .threads
        .into_iter()
        .map(|thread| thread.key)
        .collect()
}

fn private_dir() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

fn write_private(file: &std::path::Path, bytes: &[u8]) {
    std::fs::write(file, bytes).unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// Store v5: switching moves the thread's place, pin, snooze, auto-settle,
/// visit and unread marks, workspace with its worktree ownership, lineage,
/// pull request and title source to the new head in one commit; the old
/// head keeps only what a segment needs, and the inbox lists one row in the
/// same place.
#[test]
fn a_switch_moves_the_threads_owner_facing_state_to_the_new_head_in_one_commit() {
    let temp = private_dir();
    let file = temp.path().join(THREADS_FILE_NAME);
    let mut store = ThreadStore::open(&file, NOW).unwrap();
    let seeded = project_threads();
    store
        .apply(NOW, |document| {
            *document = seeded;
            Ok(())
        })
        .unwrap();
    let before = store.document().clone();
    let revision = before.revision;
    let ((), committed) = store
        .apply(NOW, |document| {
            document.switch_provider("claude#a", "codex#b", None, NOW)
        })
        .unwrap();
    assert!(committed);
    let document = store.document();
    assert_eq!(document.revision, revision + 1, "one commit");
    assert_eq!(
        listed(document),
        ["codex#other", "codex#b"],
        "same row, same place"
    );

    let old = before.record("claude#a").unwrap();
    let head = document.record("codex#b").unwrap();
    assert_eq!(head.active_order, old.active_order);
    assert_eq!(head.snoozed_until, old.snoozed_until);
    assert_eq!(head.snoozed_at, old.snoozed_at);
    assert!(head.auto_settle_disabled);
    assert_eq!(head.last_visited_at, old.last_visited_at);
    assert!(head.marked_unread);
    assert_eq!(head.workspace, old.workspace, "the worktree, still owned");
    assert_eq!(head.git_root, old.git_root);
    assert_eq!(head.lineage, old.lineage);
    assert_eq!(head.pull_request, old.pull_request);
    assert!(head.pull_request_tracked);
    assert_eq!(head.title_source, Some(TitleSource::User));
    assert!(
        head.turns.is_empty() && head.last_run.is_none(),
        "bound with no turn"
    );
    assert_eq!(
        head.chain,
        Some(ChainLink {
            id: "claude#a".into(),
            order: 1
        })
    );
    assert_eq!(head.segment_of, None);

    let segment = document.record("claude#a").unwrap();
    let mut kept = ThreadRecord::new("claude#a", old.created_at);
    kept.chain = Some(ChainLink {
        id: "claude#a".into(),
        order: 0,
    });
    kept.segment_of = Some("codex#b".into());
    kept.turns.clone_from(&old.turns);
    kept.last_run.clone_from(&old.last_run);
    kept.git_root.clone_from(&old.git_root);
    assert_eq!(
        *segment, kept,
        "a segment keeps only its key, creation, chain, turns and refs"
    );

    let view = document.view(NOW, |_| false);
    let thread = view
        .threads
        .iter()
        .find(|thread| thread.key == "codex#b")
        .unwrap();
    assert_eq!(thread.created_at, old.created_at, "as old as the thread");
    let segments = &thread.chain.as_ref().unwrap().segments;
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.key.as_str(), segment.engine.as_str(), segment.order))
            .collect::<Vec<_>>(),
        [("claude#a", "claude", 0), ("codex#b", "codex", 1)]
    );
    assert_eq!(segments[0].first_at, NOW - 10 * HOUR);
    assert_eq!(segments[0].last_at, NOW - 3 * HOUR + 1_000);
    assert!(view.threads.iter().all(|thread| thread.key != "claude#a"));
    assert!(document_valid(document));

    // The commit reached the file.
    drop(store);
    let reopened = ThreadStore::open(&file, NOW).unwrap();
    assert_eq!(reopened.document(), &before_switch_applied(&before));
}

fn before_switch_applied(before: &ThreadsDocument) -> ThreadsDocument {
    let mut expected = before.clone();
    expected
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    expected.normalize(NOW);
    expected.revision = before.revision + 1;
    expected
}

/// A pinned thread stays pinned in its place; a settled one stays settled;
/// a session reconcile seeded a moment ago is taken over whole.
#[test]
fn a_switch_keeps_a_pin_and_takes_over_a_freshly_seeded_record() {
    let mut document = project_threads();
    document.unsnooze("claude#a").unwrap();
    document.pin("claude#a", NOW).unwrap();
    let pinned = document.record("claude#a").unwrap().clone();
    // Reconcile saw the new session first and seeded it on top of Active.
    let mut seeded = ThreadRecord::new("codex#b", NOW);
    seeded.active_order = Some("0".into());
    document.records.push(seeded);
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    let head = document.record("codex#b").unwrap();
    assert_eq!(head.pinned_at, pinned.pinned_at);
    assert_eq!(head.pin_order, pinned.pin_order);
    assert_eq!(
        head.active_order, pinned.active_order,
        "the seed's place is gone"
    );
    assert_eq!(head.created_at, NOW);
    assert_eq!(order_of(&document, ThreadSection::Pinned), ["codex#b"]);
    assert_eq!(order_of(&document, ThreadSection::Active), ["codex#other"]);

    let mut settled = project_threads();
    settled
        .settle("claude#a", SettledReason::Manual, NOW)
        .unwrap();
    settled
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    assert_eq!(
        settled.record("codex#b").unwrap().section(NOW),
        ThreadSection::Settled
    );
}

fn order_of(document: &ThreadsDocument, section: ThreadSection) -> Vec<String> {
    document
        .ordered(section, NOW)
        .into_iter()
        .map(|index| document.records[index].key.clone())
        .collect()
}

/// A segment is never listed or placed, never auto-settled however idle,
/// never seeded again while its session is in the registry, and is not a
/// user of the thread's folder.
#[test]
fn segments_are_never_listed_auto_settled_or_seeded_again() {
    let mut document = project_threads();
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    // Long idle: the head is kept from settling only by its owner's switch.
    document.set_auto_settle("codex#b", true).unwrap();
    let later = NOW + 30 * DAY_MS;
    let settled = document.auto_settle(later, |_| Some(false));
    assert_eq!(
        settled,
        ["codex#other", "codex#b"],
        "the segment is not settled"
    );
    assert!(document.record("claude#a").unwrap().settled_at.is_none());

    let sessions = ["claude#a", "codex#b", "codex#other"]
        .into_iter()
        .map(|key| ObservedSession {
            key: key.into(),
            origin: SeedOrigin::App,
            first_seen: Some(NOW),
            last_activity: Some(NOW),
            busy: false,
            archived: false,
            listed: true,
        })
        .collect::<Vec<_>>();
    let records = document.records.len();
    document.reconcile(&sessions, |_| false, true, later);
    assert_eq!(document.records.len(), records, "nothing seeded");
    assert!(listed(&document).iter().all(|key| key != "claude#a"));
    assert_eq!(document.head_of("claude#a"), Some("codex#b"));
    assert_eq!(document.head_of("codex#b"), Some("codex#b"));
    assert_eq!(document.head_of("codex#other"), Some("codex#other"));

    // The folder's one user is the head: a segment holds no workspace.
    assert!(document.record("claude#a").unwrap().workspace.is_none());
    assert_eq!(
        document.record("codex#b").unwrap().owned_worktree(),
        Some("/repo-wt/fix")
    );
}

/// Switching back makes the earlier segment the head again with the next
/// activation, so the chain reads oldest activation first, the head last,
/// and every member points at it.
#[test]
fn switching_back_reorders_the_chain_and_points_every_member_at_the_head() {
    let mut document = project_threads();
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    assert_eq!(
        document.switch_target("codex#b", "claude", |_| true),
        Some("claude#a".into())
    );
    assert_eq!(
        document.switch_target("codex#b", "claude", |_| false),
        None,
        "its session is gone"
    );
    assert_eq!(
        document.switch_target("codex#b", "acp:grok", |_| true),
        None
    );
    document
        .switch_provider("codex#b", "acp:grok#c", Some("handoff"), NOW + 1)
        .unwrap();
    let grok = document.record("acp:grok#c").unwrap();
    assert_eq!(
        grok.turns.len(),
        1,
        "an ACP launch records its handoff as turn 0"
    );
    assert!(grok.running());
    document
        .switch_provider("acp:grok#c", "claude#a", None, NOW + 2)
        .unwrap();
    let parts = document.chain_parts("codex#b");
    assert_eq!(
        parts
            .iter()
            .map(|part| (part.key.as_str(), part.order, part.head))
            .collect::<Vec<_>>(),
        [
            ("codex#b", 1, false),
            ("acp:grok#c", 2, false),
            ("claude#a", 3, true)
        ]
    );
    for key in ["codex#b", "acp:grok#c"] {
        assert_eq!(
            document.record(key).unwrap().segment_of.as_deref(),
            Some("claude#a")
        );
    }
    let head = document.record("claude#a").unwrap();
    assert_eq!(
        head.workspace,
        Some(owned_worktree()),
        "the worktree came back"
    );
    assert_eq!(head.title_source, Some(TitleSource::User));
    assert_eq!(listed(&document), ["codex#other", "claude#a"]);
    assert!(document_valid(&document));
}

fn measured(mut turn: TurnRecord, added: Option<u32>, removed: Option<u32>) -> TurnRecord {
    turn.added = added;
    turn.removed = removed;
    turn
}

fn push_turn(document: &mut ThreadsDocument, key: &str, turn: TurnRecord) {
    let record = document
        .records
        .iter_mut()
        .find(|record| record.key == key)
        .unwrap();
    record.turns.push(turn);
}

/// Protocol 69: a switched thread's view carries each segment's own line
/// totals, summed over the turns that measured both counts (the renderer's
/// rule for a thread's totals), so the head can total the whole thread; a
/// segment with no measured turn carries neither count, and a thread that
/// was never switched has no chain at all.
#[test]
fn a_switched_threads_view_carries_every_segments_line_totals() {
    let mut document = project_threads();
    let claude = &mut document.records[1];
    claude.turns[0].added = Some(4);
    claude.turns[0].removed = Some(1);
    push_turn(
        &mut document,
        "claude#a",
        finished_turn(1, "Check the tests", NOW - 2 * HOUR, &[]),
    );
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    push_turn(
        &mut document,
        "codex#b",
        measured(
            finished_turn(0, "Add the retry", NOW + 1, &["src/b.ts"]),
            Some(2),
            Some(0),
        ),
    );
    // Half a count is not a count.
    push_turn(
        &mut document,
        "codex#b",
        measured(
            finished_turn(1, "And log it", NOW + 2, &["src/b.ts"]),
            Some(9),
            None,
        ),
    );
    document
        .switch_provider("codex#b", "acp:grok#c", Some("handoff"), NOW + 3)
        .unwrap();
    document
        .switch_provider("acp:grok#c", "claude#a", None, NOW + 4)
        .unwrap();
    push_turn(
        &mut document,
        "claude#a",
        measured(
            finished_turn(2, "Ship it", NOW + 5, &["src/c.ts"]),
            Some(1),
            Some(0),
        ),
    );
    let view = document.view(NOW + 6, |_| false);
    let head = view
        .threads
        .iter()
        .find(|thread| thread.key == "claude#a")
        .unwrap();
    let totals = head
        .chain
        .as_ref()
        .unwrap()
        .segments
        .iter()
        .map(|segment| (segment.key.as_str(), segment.added, segment.removed))
        .collect::<Vec<_>>();
    assert_eq!(
        totals,
        [
            ("codex#b", Some(2), Some(0)),
            ("acp:grok#c", None, None),
            ("claude#a", Some(5), Some(1)),
        ]
    );
    let other = view
        .threads
        .iter()
        .find(|thread| thread.key == "codex#other")
        .unwrap();
    assert!(other.chain.is_none());
}

/// The store refuses every switch it can see is wrong, and changes nothing.
#[test]
fn the_store_refuses_a_wrong_switch_and_changes_nothing() {
    let mut document = project_threads();
    let mut chat = ThreadRecord::new("claude#chat", NOW);
    chat.workspace = Some(ThreadWorkspace {
        mode: WorkspaceMode::None,
        project_root: String::new(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    });
    document.records.push(chat);
    let mut elsewhere = document.clone();
    elsewhere
        .switch_provider("codex#other", "claude#z", None, NOW)
        .unwrap();
    for (document, head, target, code) in [
        (
            document.clone(),
            "claude#nope",
            "codex#b",
            THREADS_UNKNOWN_CODE,
        ),
        (
            document.clone(),
            "claude#chat",
            "codex#b",
            THREADS_SWITCH_PLAIN_CODE,
        ),
        (
            document.clone(),
            "claude#a",
            "claude#a",
            THREADS_SWITCH_SAME_CODE,
        ),
        (
            elsewhere.clone(),
            "codex#other",
            "claude#y",
            THREADS_INVALID_CODE,
        ),
        (
            elsewhere.clone(),
            "claude#a",
            "codex#other",
            THREADS_INVALID_CODE,
        ),
    ] {
        let mut changed = document.clone();
        let refusal = changed
            .switch_provider(head, target, None, NOW)
            .unwrap_err();
        assert_eq!(refusal.code, code, "{head} -> {target}");
        assert_eq!(changed, document, "{head} -> {target} changed nothing");
    }
}

/// A chain whose head went (missing for a day) gets its most recent segment
/// as the head; a document with two heads in one chain is refused.
#[test]
fn a_chain_without_its_head_gets_its_newest_segment_back_as_head() {
    let mut document = project_threads();
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    document
        .switch_provider("codex#b", "acp:grok#c", None, NOW)
        .unwrap();
    document.records.retain(|record| record.key != "acp:grok#c");
    document.normalize(NOW);
    assert_eq!(document.head_of("claude#a"), Some("codex#b"));
    assert!(document_valid(&document));

    let mut two_heads = document.clone();
    for record in &mut two_heads.records {
        record.segment_of = None;
    }
    assert!(!document_valid(&two_heads));
    let mut stray = ThreadsDocument::default();
    let mut record = ThreadRecord::new("claude#x", NOW);
    record.segment_of = Some("claude#y".into());
    stray.records.push(record);
    assert!(!document_valid(&stray), "segmentOf needs a chain");
}

/// Commit offers what every segment's finished turns named, unknown when
/// any segment's are; the message uses the whole chain's prompts but never a
/// handoff.
#[test]
fn ship_scope_covers_every_segments_files_and_prompts() {
    let mut document = project_threads();
    let record = document
        .records
        .iter_mut()
        .find(|record| record.key == "claude#a")
        .unwrap();
    record.workspace = Some(local());
    document
        .switch_provider("claude#a", "codex#b", None, NOW)
        .unwrap();
    let head = document
        .records
        .iter_mut()
        .find(|record| record.key == "codex#b")
        .unwrap();
    head.turns = vec![
        finished_turn(
            0,
            "You are taking over a thread from Claude in Interface; the owner switched it to Codex.",
            NOW,
            &[],
        ),
        finished_turn(1, "Add the retry test", NOW + 10, &["src/a.test.ts"]),
    ];
    let scope = document.ship_scope("codex#b").unwrap();
    assert_eq!(
        scope.touched.unwrap().into_iter().collect::<Vec<_>>(),
        ["src/a.test.ts", "src/a.ts"]
    );
    assert!(!scope.touched_unknown);
    assert_eq!(scope.prompts, ["Fix the login retry", "Add the retry test"]);

    let segment = document
        .records
        .iter_mut()
        .find(|record| record.key == "claude#a")
        .unwrap();
    segment.turns[0].touched = None;
    assert!(document.ship_scope("codex#b").unwrap().touched_unknown);
}

/// Store v5 (protocol 59): a v4 store is read once, never changed, and
/// carried over to `threads.v5.json` as version 5 with its `child` lineage
/// and team tools setting as they were, winning over a v3 file beside it. A
/// v4 file holding a chain (version 5's fields) is left for the build that
/// wrote it and the store starts empty.
#[test]
fn a_v4_store_is_carried_over_once_and_wins_over_v3() {
    let v4 = r#"{
  "version": 4,
  "revision": 8,
  "seeded": true,
  "settings": {
    "autoSettleDays": 3,
    "settleOnMerge": true,
    "generatedTitles": false,
    "trackPullRequests": true,
    "teamTools": false
  },
  "records": [
    {
      "key": "claude#a",
      "createdAt": 1791290000000,
      "activeOrder": "V",
      "autoSettleDisabled": false,
      "markedUnread": false,
      "lineage": {
        "parent": "claude#p",
        "relation": "child",
        "keptTurns": 0,
        "at": 1791290000000
      },
      "turns": [
        {
          "ordinal": 0,
          "sentAt": 1791300000000,
          "promptDigest": "971ad67d98611a46",
          "state": "done",
          "preview": "Fix the login test",
          "previewTruncated": false,
          "endedAt": 1791300041000
        }
      ]
    }
  ],
  "prepared": []
}
"#;
    let golden = v4
        .replace(r#""version": 4"#, r#""version": 5"#)
        .replace(r#""revision": 8"#, r#""revision": 9"#);
    let chained = v4.replace(
        r#""markedUnread": false,"#,
        r#""markedUnread": false,
      "chain": {
        "id": "claude#a",
        "order": 0
      },"#,
    );
    assert_ne!(chained, v4);
    for (bytes, carried) in [
        (v4.to_owned(), true),
        (chained, false),
        ("{".to_owned(), false),
    ] {
        let temp = private_dir();
        let older = temp.path().join(THREADS_V4_FILE_NAME);
        let v3 = temp.path().join(THREADS_V3_FILE_NAME);
        let file = temp.path().join(THREADS_FILE_NAME);
        write_private(&older, bytes.as_bytes());
        write_private(
            &v3,
            v4.replace(r#""version": 4"#, r#""version": 3"#)
                .replace(r#""teamTools": false"#, r#""teamTools": true"#)
                .replace(r#""relation": "child""#, r#""relation": "handoff""#)
                .replace("claude#a", "claude#v3")
                .as_bytes(),
        );
        let store = ThreadStore::open(&file, NOW).unwrap();
        assert!(!store.quarantined());
        let document = store.document();
        assert!(document.record("claude#v3").is_none(), "v3 is never read");
        if carried {
            assert_eq!((document.version, document.revision), (5, 9));
            assert!(!document.settings.team_tools);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), golden);
        } else {
            assert_eq!(
                *document,
                ThreadsDocument {
                    revision: NOW,
                    ..ThreadsDocument::default()
                },
                "{bytes}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&older).unwrap(),
            bytes,
            "never changed"
        );
        drop(store);
        if carried {
            write_private(&older, v4.replace("claude#a", "claude#again").as_bytes());
            let store = ThreadStore::open(&file, NOW).unwrap();
            assert!(store.document().record("claude#again").is_none());
        }
    }
}
