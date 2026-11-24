use std::{
    fs,
    path::{Path, PathBuf},
};

use interface_contracts::orchestration::{
    AgentCapabilityV1, AgentNodeState, AgentNodeV1, AgentScopeV1, EffectReservationV1,
    MailboxMessageV1, MessageDeliveryReceiptV1, MessageDeliveryState, MessageOrigin,
    OrchestrationActorKind, OrchestrationActorV1, OrchestrationCapabilityFacet,
    OrchestrationCapabilityState, OrchestrationEffectKind, OrchestrationEffectState,
    OrchestrationEffectTargetKind, OrchestrationEntityKind, OrchestrationEventV1,
    OrchestrationMutation, OrchestrationMutationMetaV1, OrchestrationSnapshotV1,
    ReconciledLeaseState, SettledEscrowState, WorktreeEscrowState, WorktreeEscrowV1,
    WorktreeLeaseState, WorktreeLeaseV1, create_empty_orchestration_snapshot,
};
use interface_orchestration::{
    AgentPageInput, EventProjectionInput, InboxProjectionInput,
    ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES, ORCHESTRATION_STORE_DIRECTORY,
    ORCHESTRATION_STORE_FILE, ORCHESTRATION_STORE_QUARANTINE_DIRECTORY,
    ORCHESTRATION_STORE_RECORDS_DIRECTORY, OrchestrationErrorCode, OrchestrationStore,
    TranscriptProjectionInput, agent_state_is_terminal,
    assert_orchestration_event_window_transition, bounded_utf8, is_orchestration_worker,
    orchestration_control_public_bytes, project_orchestration_agents_for_control,
    project_orchestration_events_for_control, project_orchestration_inbox_for_control,
    project_orchestration_snapshot_for_control, project_orchestration_transcript_for_control,
    read_orchestration_store_read_only, reduce_orchestration, worker_session_keys,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn store_path(root: &Path) -> PathBuf {
    root.join(ORCHESTRATION_STORE_DIRECTORY)
        .join(ORCHESTRATION_STORE_RECORDS_DIRECTORY)
        .join(ORCHESTRATION_STORE_FILE)
}

fn records_path(root: &Path) -> PathBuf {
    root.join(ORCHESTRATION_STORE_DIRECTORY)
        .join(ORCHESTRATION_STORE_RECORDS_DIRECTORY)
}

fn quarantine_path(root: &Path) -> PathBuf {
    root.join(ORCHESTRATION_STORE_DIRECTORY)
        .join(ORCHESTRATION_STORE_QUARANTINE_DIRECTORY)
}

fn at(second: u64) -> String {
    format!("2026-08-24T12:00:{second:02}.000Z")
}

fn meta(second: u64, suffix: &str) -> OrchestrationMutationMetaV1 {
    OrchestrationMutationMetaV1 {
        event_id: format!("event_{suffix}"),
        at: at(second),
        actor: OrchestrationActorV1 {
            kind: OrchestrationActorKind::Host,
            actor_id: "host_test".to_owned(),
        },
        summary: suffix.to_owned(),
    }
}

fn empty() -> OrchestrationSnapshotV1 {
    create_empty_orchestration_snapshot("host_test".to_owned(), "epoch_test_1".to_owned(), at(0))
        .unwrap()
}

fn capabilities() -> Vec<AgentCapabilityV1> {
    [
        OrchestrationCapabilityFacet::Reference,
        OrchestrationCapabilityFacet::Transcript,
        OrchestrationCapabilityFacet::Message,
        OrchestrationCapabilityFacet::Wake,
        OrchestrationCapabilityFacet::Spawn,
        OrchestrationCapabilityFacet::Worktree,
    ]
    .into_iter()
    .map(|facet| AgentCapabilityV1 {
        facet,
        state: OrchestrationCapabilityState::Available,
        reason: None,
    })
    .collect()
}

fn agent(id: &str, now: String) -> AgentNodeV1 {
    AgentNodeV1 {
        version: 1,
        agent_id: id.to_owned(),
        name: None,
        engine_id: "codex".to_owned(),
        session_key: Some(format!("codex#{id}")),
        provider_session_id: None,
        runtime_epoch: "epoch_test_1".to_owned(),
        parent_agent_id: None,
        role: None,
        depth: 0,
        state: AgentNodeState::Ready,
        capabilities: capabilities(),
        scope: AgentScopeV1 {
            canonical_cwd: "/tmp/repository".to_owned(),
            canonical_project_root: Some("/tmp/repository".to_owned()),
            access_mode: "workspace-write".to_owned(),
            scope_digest: DIGEST_A.to_owned(),
            worktree_lease_id: None,
        },
        created_at: now.clone(),
        updated_at: now,
    }
}

#[test]
fn reducer_separates_session_identity_and_enforces_runtime_effect_state() {
    let mut snapshot = reduce_orchestration(
        &empty(),
        OrchestrationMutation::RegisterAgent {
            agent: agent("agent_one", at(1)),
            meta: meta(1, "agent"),
        },
    )
    .unwrap();
    let mut duplicate = agent("agent_two", at(2));
    duplicate.session_key = Some("codex#agent_one".to_owned());
    let error = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::RegisterAgent {
            agent: duplicate,
            meta: meta(2, "duplicate"),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::AlreadyExists);
    snapshot = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch: "epoch_test_2".to_owned(),
            meta: meta(2, "rotate"),
        },
    )
    .unwrap();
    assert_eq!(snapshot.revision, 2);
    assert_eq!(
        snapshot.events.last().unwrap().kind,
        "host.runtime-epoch-rotated"
    );
}

#[test]
fn completed_worktree_escrow_moves_the_lease_to_the_target_agent() {
    let mut snapshot = reduce_orchestration(
        &empty(),
        OrchestrationMutation::RegisterAgent {
            agent: agent("agent_source", at(3)),
            meta: meta(3, "source"),
        },
    )
    .unwrap();
    snapshot = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::RegisterAgent {
            agent: agent("agent_target", at(4)),
            meta: meta(4, "target"),
        },
    )
    .unwrap();
    snapshot = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::AcquireWorktreeLease {
            lease: WorktreeLeaseV1 {
                version: 1,
                lease_id: "lease_escrow".to_owned(),
                canonical_path: "/tmp/repository/.worktrees/escrow".to_owned(),
                canonical_cwd: None,
                repository_root: "/tmp/repository".to_owned(),
                branch: None,
                head: None,
                dirty_state_digest: DIGEST_A.to_owned(),
                owner_agent_id: "agent_source".to_owned(),
                owner_revision: 0,
                state: WorktreeLeaseState::Active,
                escrow: None,
                coordinator: None,
                acquired_at: at(9),
                updated_at: at(9),
                released_at: None,
            },
            meta: meta(9, "acquire"),
        },
    )
    .unwrap();
    snapshot = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::BeginWorktreeEscrow {
            lease_id: "lease_escrow".to_owned(),
            escrow: WorktreeEscrowV1 {
                escrow_id: "escrow_transfer".to_owned(),
                source_agent_id: "agent_source".to_owned(),
                target_agent_id: "agent_target".to_owned(),
                source_owner_revision: 0,
                state: WorktreeEscrowState::Prepared,
                prepared_at: at(10),
                settled_at: None,
            },
            meta: meta(10, "escrow"),
        },
    )
    .unwrap();
    snapshot = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::SettleWorktreeEscrow {
            lease_id: "lease_escrow".to_owned(),
            state: SettledEscrowState::Completed,
            meta: meta(11, "settle"),
        },
    )
    .unwrap();
    let target = snapshot
        .agents
        .iter()
        .find(|item| item.agent_id == "agent_target")
        .unwrap();
    assert_eq!(
        target.scope.worktree_lease_id.as_deref(),
        Some("lease_escrow")
    );
    let source = snapshot
        .agents
        .iter()
        .find(|item| item.agent_id == "agent_source")
        .unwrap();
    assert_eq!(source.scope.worktree_lease_id, None);
    let lease = &snapshot.worktree_leases[0];
    assert_eq!(lease.owner_agent_id, "agent_target");
    assert_eq!(lease.owner_revision, 1);
    assert_eq!(lease.state, WorktreeLeaseState::Active);
}

fn message(index: usize) -> MailboxMessageV1 {
    let body = format!("body {index}");
    MailboxMessageV1 {
        version: 1,
        message_id: format!("message_{index}"),
        thread_id: "thread".to_owned(),
        parent_message_id: None,
        origin: MessageOrigin::Human,
        sender_agent_id: None,
        recipient_agent_id: "agent_1".to_owned(),
        body_digest: format!("{:x}", Sha256::digest(body.as_bytes())),
        body_bytes: body.len() as u64,
        body: Some(body),
        expect_reply: true,
        effect_id: None,
        notice_summary: Some("private".to_owned()),
        receipts: vec![MessageDeliveryReceiptV1 {
            version: 1,
            receipt_id: format!("receipt_{index}"),
            state: MessageDeliveryState::Requested,
            at: at(0),
            provider_receipt_id: Some("private".to_owned()),
            error_code: Some("e".repeat(500)),
            detail: Some("secret".to_owned()),
        }],
        created_at: at(0),
        updated_at: at(0),
    }
}

#[test]
fn projections_strip_private_receipt_fields_and_use_stable_cursors() {
    let mut snapshot = empty();
    snapshot.messages = (0..6).map(message).collect();
    let first = project_orchestration_inbox_for_control(
        &snapshot,
        &InboxProjectionInput {
            agent_id: "agent_1".to_owned(),
            thread_id: None,
            before_message_id: None,
            limit: 2,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        first
            .messages
            .iter()
            .map(|item| item.message_id.as_str())
            .collect::<Vec<_>>(),
        ["message_4", "message_5"]
    );
    assert!(
        first
            .messages
            .iter()
            .flat_map(|item| &item.receipts)
            .all(|receipt| receipt.detail.is_none()
                && receipt.provider_receipt_id.is_none()
                && receipt.error_code.as_ref().unwrap().len() <= 256)
    );
    let second = project_orchestration_inbox_for_control(
        &snapshot,
        &InboxProjectionInput {
            agent_id: "agent_1".to_owned(),
            thread_id: None,
            before_message_id: first.next_before_message_id.clone(),
            limit: 2,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        second
            .messages
            .iter()
            .map(|item| item.message_id.as_str())
            .collect::<Vec<_>>(),
        ["message_2", "message_3"]
    );
    assert!(
        orchestration_control_public_bytes(&first).unwrap()
            <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES
    );
}

fn big_event(sequence: u64, summary_bytes: usize) -> OrchestrationEventV1 {
    OrchestrationEventV1 {
        version: 1,
        event_id: format!("event_{sequence}"),
        sequence,
        kind: "host.tick".to_owned(),
        entity_kind: OrchestrationEntityKind::Host,
        entity_id: "host_test".to_owned(),
        actor: OrchestrationActorV1 {
            kind: OrchestrationActorKind::Host,
            actor_id: "host_test".to_owned(),
        },
        at: at(0),
        summary: "s".repeat(summary_bytes),
    }
}

fn big_message(index: usize, body_bytes: usize) -> MailboxMessageV1 {
    let mut message = message(index);
    let body = "b".repeat(body_bytes);
    message.body_digest = format!("{:x}", Sha256::digest(body.as_bytes()));
    message.body_bytes = body.len() as u64;
    message.body = Some(body);
    message
}

#[test]
fn control_agent_pages_share_one_cursor_and_budget_contract() {
    let agents = (0..40)
        .map(|index| agent(&format!("agent_{index:02}"), at(1)))
        .collect::<Vec<_>>();
    let page = |cursor: Option<&str>, limit: usize| {
        project_orchestration_agents_for_control(
            &agents,
            &AgentPageInput {
                after_agent_id: cursor.map(str::to_owned),
                limit,
            },
        )
        .unwrap()
    };
    let first = page(None, 3).unwrap();
    assert_eq!(
        first
            .agents
            .iter()
            .map(|item| item.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["agent_00", "agent_01", "agent_02"]
    );
    assert!(first.has_more);
    assert_eq!(first.after_agent_id, None);
    assert_eq!(first.next_after_agent_id.as_deref(), Some("agent_02"));
    let second = page(first.next_after_agent_id.as_deref(), 3).unwrap();
    assert_eq!(
        second
            .agents
            .iter()
            .map(|item| item.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["agent_03", "agent_04", "agent_05"]
    );
    assert_eq!(second.after_agent_id.as_deref(), Some("agent_02"));
    let capped = page(None, 1_000).unwrap();
    assert_eq!(capped.agents.len(), 32, "the page limit caps the request");
    assert_eq!(capped.next_after_agent_id.as_deref(), Some("agent_31"));
    let tail = page(Some("agent_38"), 8).unwrap();
    assert_eq!(tail.agents.len(), 1);
    assert!(!tail.has_more);
    assert_eq!(tail.next_after_agent_id, None);
    assert!(page(Some("agent_missing"), 3).is_none());
}

#[test]
fn control_pages_stop_at_the_budget_and_refuse_one_oversized_entry() {
    let wide_agent = |index: usize, bytes: usize| {
        let mut item = agent(&format!("agent_{index}"), at(1));
        item.name = Some("n".repeat(bytes));
        item
    };
    let agents = (0..6)
        .map(|index| wide_agent(index, 120 * 1024))
        .collect::<Vec<_>>();
    let page = project_orchestration_agents_for_control(
        &agents,
        &AgentPageInput {
            after_agent_id: None,
            limit: 32,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(page.agents.len(), 4, "the byte budget ends the page early");
    assert!(page.has_more);
    assert_eq!(page.next_after_agent_id.as_deref(), Some("agent_3"));
    assert!(
        orchestration_control_public_bytes(&page).unwrap()
            <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES
    );
    let resumed = project_orchestration_agents_for_control(
        &agents,
        &AgentPageInput {
            after_agent_id: Some("agent_3".to_owned()),
            limit: 32,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(resumed.agents.len(), 2);
    assert!(!resumed.has_more);
    let oversized = vec![wide_agent(0, ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES)];
    let error = project_orchestration_agents_for_control(
        &oversized,
        &AgentPageInput {
            after_agent_id: None,
            limit: 32,
        },
    )
    .unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::PublicBudget);
}

#[test]
fn budgeted_pages_trim_from_their_own_end_and_stop_at_their_own_floor() {
    let transcript = project_orchestration_transcript_for_control(&TranscriptProjectionInput {
        agent_id: "agent_1".to_owned(),
        requested_lines: 64,
        lines: (0..64)
            .map(|index| format!("line-{index:02}-{}", "\u{7}".repeat(4_000)))
            .collect(),
        source_was_truncated: false,
    })
    .unwrap();
    assert_eq!(
        transcript.lines.len(),
        21,
        "oldest transcript lines go first"
    );
    assert!(transcript.lines[0].starts_with("line-43-"));
    assert!(transcript.lines.last().unwrap().starts_with("line-63-"));
    assert!(transcript.truncated);

    let mut snapshot = empty();
    snapshot.messages = (0..3).map(|index| big_message(index, 200 * 1024)).collect();
    let inbox = project_orchestration_inbox_for_control(
        &snapshot,
        &InboxProjectionInput {
            agent_id: "agent_1".to_owned(),
            thread_id: None,
            before_message_id: None,
            limit: 3,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        inbox
            .messages
            .iter()
            .map(|item| item.message_id.as_str())
            .collect::<Vec<_>>(),
        ["message_1", "message_2"],
        "the oldest inbox message is dropped first"
    );
    assert!(inbox.has_more);
    assert_eq!(inbox.next_before_message_id.as_deref(), Some("message_1"));
    snapshot.messages = vec![big_message(0, ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES)];
    let error = project_orchestration_inbox_for_control(
        &snapshot,
        &InboxProjectionInput {
            agent_id: "agent_1".to_owned(),
            thread_id: None,
            before_message_id: None,
            limit: 3,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.code,
        OrchestrationErrorCode::PublicBudget,
        "the inbox floor keeps one message and then fails closed"
    );

    let mut snapshot = empty();
    snapshot.revision = 3;
    snapshot.event_floor_sequence = Some(0);
    snapshot.events = (1..=3)
        .map(|sequence| big_event(sequence, 200 * 1024))
        .collect();
    let events = project_orchestration_events_for_control(
        &snapshot,
        EventProjectionInput {
            sequence: 0,
            limit: 3,
        },
    )
    .unwrap();
    assert_eq!(
        events
            .events
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>(),
        [1, 2],
        "the newest event page entry is dropped first"
    );
    assert!(events.has_more);
    assert_eq!(events.next_sequence, 2);
    snapshot.events = vec![big_event(1, ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES)];
    let error = project_orchestration_events_for_control(
        &snapshot,
        EventProjectionInput {
            sequence: 0,
            limit: 3,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.code,
        OrchestrationErrorCode::PublicBudget,
        "the event floor keeps one event and then fails closed"
    );
}

#[test]
fn retained_event_floor_advances_without_a_mutation_cliff() {
    let mut snapshot = empty();
    snapshot.revision = 50_000;
    snapshot.event_floor_sequence = Some(0);
    snapshot.events = (1..=50_000)
        .map(|sequence| OrchestrationEventV1 {
            version: 1,
            event_id: format!("event_{sequence}"),
            sequence,
            kind: "host.tick".to_owned(),
            entity_kind: OrchestrationEntityKind::Host,
            entity_id: "host_test".to_owned(),
            actor: OrchestrationActorV1 {
                kind: OrchestrationActorKind::Host,
                actor_id: "host_test".to_owned(),
            },
            at: at(0),
            summary: "tick".to_owned(),
        })
        .collect();
    let next = reduce_orchestration(
        &snapshot,
        OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch: "epoch_test_2".to_owned(),
            meta: meta(1, "50001"),
        },
    )
    .unwrap();
    assert_eq!(next.revision, 50_001);
    assert_eq!(next.event_floor_sequence, Some(1));
    assert_eq!(next.events.first().unwrap().sequence, 2);
    let page = project_orchestration_events_for_control(
        &next,
        EventProjectionInput {
            sequence: 0,
            limit: 2,
        },
    )
    .unwrap();
    assert!(page.truncated);
    assert_eq!(page.floor_sequence, 1);
}

#[test]
fn store_is_private_cas_guarded_reopenable_and_read_only_reads_do_not_rewrite() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host_store", "epoch_store_1", at(0)).unwrap();
    let updated = store
        .apply(
            0,
            OrchestrationMutation::RotateRuntimeEpoch {
                runtime_epoch: "epoch_store_2".to_owned(),
                meta: meta(1, "store"),
            },
        )
        .unwrap();
    assert_eq!(updated.revision, 1);
    let stale = store
        .apply(
            0,
            OrchestrationMutation::RotateRuntimeEpoch {
                runtime_epoch: "epoch_store_3".to_owned(),
                meta: meta(2, "stale"),
            },
        )
        .unwrap_err();
    assert_eq!(stale.code, OrchestrationErrorCode::CasConflict);
    assert_eq!(stale.current_revision, Some(1));
    store.close().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = store_path(&root);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let before = fs::metadata(&path).unwrap();
        let bytes_before = fs::read(&path).unwrap();
        let snapshot = read_orchestration_store_read_only(&root).unwrap().unwrap();
        assert_eq!(snapshot.runtime_epoch, "epoch_store_2");
        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mtime(), after.mtime());
        assert_eq!(bytes_before, fs::read(&path).unwrap());
    }

    let mut reopened = OrchestrationStore::open(&root, "ignored", "ignored", at(3)).unwrap();
    assert_eq!(reopened.snapshot().unwrap().revision, 1);
    assert_eq!(reopened.revision(), 1);
    reopened.close().unwrap();
}

#[test]
fn store_refuses_a_repeated_event_id_across_mutations_refusals_and_reopen() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let rotate = |second: u64, suffix: &str| OrchestrationMutation::RotateRuntimeEpoch {
        runtime_epoch: format!("epoch_{second}_{suffix}"),
        meta: meta(second, suffix),
    };
    let mut store = OrchestrationStore::open(&root, "host", "epoch_0", at(0)).unwrap();
    store.apply(0, rotate(1, "first")).unwrap();
    store.apply(1, rotate(2, "second")).unwrap();
    // A refused write leaves the retained events, and their index, as they were.
    let stale = store.apply(1, rotate(3, "stale")).unwrap_err();
    assert_eq!(stale.code, OrchestrationErrorCode::CasConflict);
    for repeated in ["first", "second"] {
        let error = store.apply(2, rotate(3, repeated)).unwrap_err();
        assert_eq!(
            error.code,
            OrchestrationErrorCode::AlreadyExists,
            "{repeated}"
        );
    }
    assert_eq!(store.apply(2, rotate(3, "stale")).unwrap().revision, 3);
    store.close().unwrap();

    let mut reopened = OrchestrationStore::open(&root, "host", "epoch_0", at(4)).unwrap();
    let error = reopened.apply(3, rotate(4, "stale")).unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::AlreadyExists);
    assert_eq!(reopened.apply(3, rotate(4, "fourth")).unwrap().revision, 4);
    reopened.close().unwrap();
}

#[test]
fn compare_and_set_with_still_validates_a_hand_built_replacement() {
    // `apply` skips the store-side validate because the reducer already ran
    // it; any other replacement must still be validated before writing.
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap();
    let bytes_before = fs::read(store_path(&root)).unwrap();
    let error = store
        .compare_and_set_with(0, |current| {
            let mut next = current.clone();
            // A revision with no event for it: the retained window is invalid.
            next.revision = 1;
            Ok(next)
        })
        .unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::InvalidState);
    assert_eq!(fs::read(store_path(&root)).unwrap(), bytes_before);
    assert_eq!(store.revision(), 0);
    store.close().unwrap();
}

#[test]
fn one_writer_and_same_revision_external_drift_fail_closed() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host", "epoch_1", at(0)).unwrap();
    let second = OrchestrationStore::open(&root, "host", "epoch_1", at(0)).unwrap_err();
    assert_eq!(second.code, OrchestrationErrorCode::WriterLeaseLost);

    let path = store_path(&root);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let drifted = serde_json::to_vec(&value).unwrap();
    fs::write(&path, &drifted).unwrap();
    let error = store
        .apply(
            0,
            OrchestrationMutation::RotateRuntimeEpoch {
                runtime_epoch: "epoch_2".to_owned(),
                meta: meta(1, "drift"),
            },
        )
        .unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::CasConflict);
    assert_eq!(
        fs::read(&path).unwrap(),
        drifted,
        "CAS refusal must not overwrite drifted bytes"
    );
    store.close().unwrap();
}

#[cfg(unix)]
#[test]
fn stale_atomic_temporary_is_quarantined_before_reopen() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap();
    store.close().unwrap();
    let stale_name = ".interface-atomic-aaaaaaaaaaaaaaaa-00000000-0000-0000-0000-000000000000.tmp";
    let stale = records_path(&root).join(stale_name);
    fs::write(&stale, b"uncommitted").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();
    let mut reopened = OrchestrationStore::open(&root, "ignored", "ignored", at(1)).unwrap();
    assert!(!stale.exists());
    assert_eq!(fs::read_dir(quarantine_path(&root)).unwrap().count(), 1);
    assert_eq!(reopened.snapshot().unwrap().revision, 0);
    reopened.close().unwrap();
}

#[test]
fn corrupt_legacy_state_is_preserved_and_not_silently_reset() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap();
    store.close().unwrap();
    let path = store_path(&root);
    fs::write(&path, b"{broken\n").unwrap();
    let error = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::StateCorrupt);
    assert_eq!(fs::read(&path).unwrap(), b"{broken\n");
    assert_eq!(fs::read_dir(quarantine_path(&root)).unwrap().count(), 0);
    let retry = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap_err();
    assert_eq!(retry.code, OrchestrationErrorCode::StateCorrupt);
    assert_eq!(fs::read(&path).unwrap(), b"{broken\n");
}

#[cfg(unix)]
#[test]
fn symlink_root_and_hardlinked_authoritative_file_are_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = TempDir::new().unwrap();
    let target = temporary.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let link = temporary.path().join("link");
    symlink(&target, &link).unwrap();
    let error = OrchestrationStore::open(&link, "host", "epoch", at(0)).unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::UnsafeStorage);

    let root = temporary.path().join("state");
    let mut store = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap();
    store.close().unwrap();
    fs::hard_link(store_path(&root), temporary.path().join("alias.json")).unwrap();
    let error = OrchestrationStore::open(&root, "host", "epoch", at(0)).unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::UnsafeStorage);
}

fn tick(sequence: u64) -> OrchestrationEventV1 {
    OrchestrationEventV1 {
        version: 1,
        event_id: format!("event_{sequence}"),
        sequence,
        kind: "host.tick".to_owned(),
        entity_kind: OrchestrationEntityKind::Host,
        entity_id: "host_test".to_owned(),
        actor: OrchestrationActorV1 {
            kind: OrchestrationActorKind::Host,
            actor_id: "host_test".to_owned(),
        },
        at: at(0),
        summary: "tick".to_owned(),
    }
}

#[test]
fn event_window_transition_compares_in_place_without_cloning_the_window() {
    let mut before = empty();
    before.revision = 2;
    before.events = vec![tick(1), tick(2)];
    let mut after = before.clone();
    after.revision = 3;
    after.events.push(tick(3));
    assert_orchestration_event_window_transition(&before, &after).unwrap();

    after.events[0] = tick(9);
    let error = assert_orchestration_event_window_transition(&before, &after).unwrap_err();
    assert_eq!(error.code, OrchestrationErrorCode::InvalidState);
}

#[test]
fn event_page_starts_after_the_requested_sequence() {
    let mut snapshot = empty();
    snapshot.revision = 8;
    snapshot.event_floor_sequence = Some(0);
    snapshot.events = (1..=8).map(tick).collect();
    let page = project_orchestration_events_for_control(
        &snapshot,
        EventProjectionInput {
            sequence: 5,
            limit: 2,
        },
    )
    .unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [6, 7]
    );
    assert!(page.has_more);
    assert_eq!(page.next_sequence, 7);
}

#[test]
fn control_byte_budget_matches_compact_json_length() {
    let snapshot = empty();
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    assert_eq!(
        orchestration_control_public_bytes(&snapshot).unwrap(),
        encoded.len()
    );
}

#[test]
fn bounded_utf8_stops_on_a_char_boundary() {
    assert_eq!(bounded_utf8("hello", 5), "hello");
    assert_eq!(bounded_utf8("hello", 3), "hel");
    assert_eq!(bounded_utf8("éé", 1), "");
    assert_eq!(bounded_utf8("éé", 2), "é");
}

#[test]
fn terminal_state_helpers_match_absorbing_reducer_states() {
    assert!(agent_state_is_terminal(AgentNodeState::Failed));
    assert!(agent_state_is_terminal(AgentNodeState::Unknown));
    assert!(!agent_state_is_terminal(AgentNodeState::Idle));
    assert!(!agent_state_is_terminal(AgentNodeState::Planned));
}

fn reserved_effect(
    id: &str,
    key: &str,
    kind: OrchestrationEffectKind,
    target_kind: OrchestrationEffectTargetKind,
    target_id: &str,
    reserved_at: String,
) -> EffectReservationV1 {
    EffectReservationV1 {
        version: 1,
        effect_id: id.to_owned(),
        idempotency_key: key.to_owned(),
        kind,
        target_kind,
        target_id: target_id.to_owned(),
        state: OrchestrationEffectState::Reserved,
        attempts: 0,
        request_hash: DIGEST_A.to_owned(),
        runtime_epoch: "epoch_test_1".to_owned(),
        reserved_at,
        started_at: None,
        settled_at: None,
        provider_effect_id: None,
        error_code: None,
        reconciliation_note: None,
    }
}

/// A snapshot holding one reserved effect (`effect_existing`, key
/// `key_existing`) so an intent can collide with its id or its key.
fn with_existing_effect() -> OrchestrationSnapshotV1 {
    reduce_orchestration(
        &empty(),
        OrchestrationMutation::ReserveEffect {
            effect: reserved_effect(
                "effect_existing",
                "key_existing",
                OrchestrationEffectKind::ProviderOperation,
                OrchestrationEffectTargetKind::Agent,
                "agent_elsewhere",
                at(1),
            ),
            meta: meta(1, "existing"),
        },
    )
    .unwrap()
}

fn refusal(
    snapshot: &OrchestrationSnapshotV1,
    mutation: OrchestrationMutation,
) -> (OrchestrationErrorCode, String) {
    let error = reduce_orchestration(snapshot, mutation).unwrap_err();
    (error.code, error.message)
}

/// Each clause of an intent's effect check, broken one at a time.
fn broken_intent_effects(valid: &EffectReservationV1) -> Vec<EffectReservationV1> {
    let mut broken = vec![valid.clone(); 6];
    broken[0].kind = OrchestrationEffectKind::ProviderOperation;
    broken[1].target_kind = if valid.target_kind == OrchestrationEffectTargetKind::Message {
        OrchestrationEffectTargetKind::Agent
    } else {
        OrchestrationEffectTargetKind::Message
    };
    "someone_else".clone_into(&mut broken[2].target_id);
    broken[3].state = OrchestrationEffectState::Started;
    broken[4].attempts = 1;
    "epoch_stale".clone_into(&mut broken[5].runtime_epoch);
    broken
}

#[test]
fn agent_intent_admission_keeps_its_error_precedence() {
    let snapshot = with_existing_effect();
    let intent = |agent: AgentNodeV1, effect: EffectReservationV1| {
        OrchestrationMutation::CreateAgentIntent {
            agent,
            effect,
            meta: meta(2, "intent"),
        }
    };
    let mut planned = agent("agent_new", at(2));
    planned.state = AgentNodeState::Planned;
    let valid = reserved_effect(
        "effect_new",
        "key_new",
        OrchestrationEffectKind::SpawnAgent,
        OrchestrationEffectTargetKind::Agent,
        "agent_new",
        at(2),
    );
    let mut ready = planned.clone();
    ready.state = AgentNodeState::Ready;
    let mut everything_wrong = valid.clone();
    everything_wrong.effect_id = "effect_existing".to_owned();
    everything_wrong.idempotency_key = "key_existing".to_owned();
    everything_wrong.kind = OrchestrationEffectKind::ProviderOperation;
    assert_eq!(
        refusal(&snapshot, intent(ready.clone(), everything_wrong.clone())),
        (
            OrchestrationErrorCode::AlreadyExists,
            "effect effect_existing already exists".to_owned()
        )
    );
    everything_wrong.effect_id = "effect_new".to_owned();
    assert_eq!(
        refusal(&snapshot, intent(ready.clone(), everything_wrong.clone())),
        (
            OrchestrationErrorCode::IdempotencyConflict,
            "agent intent idempotency key already exists".to_owned()
        )
    );
    everything_wrong.idempotency_key = "key_new".to_owned();
    assert_eq!(
        refusal(&snapshot, intent(ready, everything_wrong)),
        (
            OrchestrationErrorCode::InvalidMutation,
            "new agent intents require a planned agent in the active epoch".to_owned()
        )
    );
    for mut effect in broken_intent_effects(&valid) {
        effect.reserved_at = at(3);
        assert_eq!(
            refusal(&snapshot, intent(planned.clone(), effect)),
            (
                OrchestrationErrorCode::InvalidMutation,
                "agent intent requires a matching unattempted spawn effect".to_owned()
            )
        );
    }
    let created = reduce_orchestration(&snapshot, intent(planned, valid)).unwrap();
    assert_eq!(created.events.last().unwrap().kind, "agent.intent-created");
}

#[test]
fn message_intent_admission_keeps_its_error_precedence() {
    let snapshot = reduce_orchestration(
        &with_existing_effect(),
        OrchestrationMutation::RegisterAgent {
            agent: agent("agent_1", at(1)),
            meta: meta(1, "recipient"),
        },
    )
    .unwrap();
    let intent = |message: MailboxMessageV1, effect: EffectReservationV1| {
        OrchestrationMutation::EnqueueMessageIntent {
            message,
            effect,
            meta: meta(2, "intent"),
        }
    };
    let mut requested = message(7);
    requested.created_at = at(2);
    requested.updated_at = at(2);
    requested.receipts[0].at = at(2);
    let valid = reserved_effect(
        "effect_new",
        "key_new",
        OrchestrationEffectKind::SendMessage,
        OrchestrationEffectTargetKind::Message,
        "message_7",
        at(2),
    );
    let mut unreceipted = requested.clone();
    unreceipted.receipts.clear();
    let mut everything_wrong = valid.clone();
    everything_wrong.effect_id = "effect_existing".to_owned();
    everything_wrong.idempotency_key = "key_existing".to_owned();
    everything_wrong.kind = OrchestrationEffectKind::ProviderOperation;
    assert_eq!(
        refusal(
            &snapshot,
            intent(unreceipted.clone(), everything_wrong.clone())
        ),
        (
            OrchestrationErrorCode::AlreadyExists,
            "effect effect_existing already exists".to_owned()
        )
    );
    everything_wrong.effect_id = "effect_new".to_owned();
    assert_eq!(
        refusal(
            &snapshot,
            intent(unreceipted.clone(), everything_wrong.clone())
        ),
        (
            OrchestrationErrorCode::IdempotencyConflict,
            "message intent idempotency key already exists".to_owned()
        )
    );
    everything_wrong.idempotency_key = "key_new".to_owned();
    assert_eq!(
        refusal(&snapshot, intent(unreceipted, everything_wrong)),
        (
            OrchestrationErrorCode::InvalidMutation,
            "message intent requires one requested receipt and matching body".to_owned()
        )
    );
    for mut effect in broken_intent_effects(&valid) {
        effect.reserved_at = at(3);
        assert_eq!(
            refusal(&snapshot, intent(requested.clone(), effect)),
            (
                OrchestrationErrorCode::InvalidMutation,
                "message intent requires a matching unattempted send effect".to_owned()
            )
        );
    }
    let created = reduce_orchestration(&snapshot, intent(requested, valid)).unwrap();
    assert_eq!(
        created.events.last().unwrap().kind,
        "message.intent-created"
    );
}

/// Source and target agents plus an active lease owned by the source, on top
/// of `with_existing_effect`.
fn with_escrowable_lease() -> OrchestrationSnapshotV1 {
    let mut snapshot = with_existing_effect();
    for (second, id) in [(2, "agent_source"), (3, "agent_target")] {
        snapshot = reduce_orchestration(
            &snapshot,
            OrchestrationMutation::RegisterAgent {
                agent: agent(id, at(second)),
                meta: meta(second, id),
            },
        )
        .unwrap();
    }
    reduce_orchestration(
        &snapshot,
        OrchestrationMutation::AcquireWorktreeLease {
            lease: WorktreeLeaseV1 {
                version: 1,
                lease_id: "lease_escrow".to_owned(),
                canonical_path: "/tmp/repository/.worktrees/escrow".to_owned(),
                canonical_cwd: None,
                repository_root: "/tmp/repository".to_owned(),
                branch: None,
                head: None,
                dirty_state_digest: DIGEST_A.to_owned(),
                owner_agent_id: "agent_source".to_owned(),
                owner_revision: 0,
                state: WorktreeLeaseState::Active,
                escrow: None,
                coordinator: None,
                acquired_at: at(4),
                updated_at: at(4),
                released_at: None,
            },
            meta: meta(4, "acquire"),
        },
    )
    .unwrap()
}

fn prepared_escrow(source_owner_revision: u64, prepared_at: String) -> WorktreeEscrowV1 {
    WorktreeEscrowV1 {
        escrow_id: "escrow_transfer".to_owned(),
        source_agent_id: "agent_source".to_owned(),
        target_agent_id: "agent_target".to_owned(),
        source_owner_revision,
        state: WorktreeEscrowState::Prepared,
        prepared_at,
        settled_at: None,
    }
}

#[test]
fn escrow_intent_admission_keeps_its_error_precedence() {
    let snapshot = with_escrowable_lease();
    let intent = |effect: EffectReservationV1| OrchestrationMutation::BeginWorktreeEscrowIntent {
        lease_id: "lease_escrow".to_owned(),
        escrow: prepared_escrow(0, at(5)),
        effect,
        meta: meta(5, "intent"),
    };
    let valid = reserved_effect(
        "effect_new",
        "key_new",
        OrchestrationEffectKind::TransferWorktree,
        OrchestrationEffectTargetKind::WorktreeLease,
        "lease_escrow",
        at(5),
    );
    let mut everything_wrong = valid.clone();
    everything_wrong.effect_id = "effect_existing".to_owned();
    everything_wrong.idempotency_key = "key_existing".to_owned();
    everything_wrong.kind = OrchestrationEffectKind::ProviderOperation;
    let mut unbound = snapshot.clone();
    unbound.worktree_leases[0].owner_revision = 1;
    assert_eq!(
        refusal(&unbound, intent(everything_wrong.clone())),
        (
            OrchestrationErrorCode::InvalidMutation,
            "escrow must bind the current owner and owner revision".to_owned()
        )
    );
    assert_eq!(
        refusal(&snapshot, intent(everything_wrong.clone())),
        (
            OrchestrationErrorCode::AlreadyExists,
            "effect effect_existing already exists".to_owned()
        )
    );
    everything_wrong.effect_id = "effect_new".to_owned();
    assert_eq!(
        refusal(&snapshot, intent(everything_wrong)),
        (
            OrchestrationErrorCode::IdempotencyConflict,
            "worktree escrow idempotency key already exists".to_owned()
        )
    );
    for mut effect in broken_intent_effects(&valid) {
        effect.reserved_at = at(6);
        assert_eq!(
            refusal(&snapshot, intent(effect)),
            (
                OrchestrationErrorCode::InvalidMutation,
                "worktree escrow requires a matching unattempted transfer effect".to_owned()
            )
        );
    }
    let prepared = reduce_orchestration(&snapshot, intent(valid)).unwrap();
    assert_eq!(
        prepared.events.last().unwrap().kind,
        "worktree.escrow-intent-prepared"
    );
}

#[test]
fn every_owner_revision_bump_refuses_to_overflow() {
    let overflow = (
        OrchestrationErrorCode::LimitExceeded,
        "lease owner revision overflow".to_owned(),
    );
    let mut exhausted = with_escrowable_lease();
    exhausted.worktree_leases[0].owner_revision = u64::MAX;

    assert_eq!(
        refusal(
            &exhausted,
            OrchestrationMutation::ReleaseWorktreeLease {
                lease_id: "lease_escrow".to_owned(),
                meta: meta(5, "release"),
            },
        ),
        overflow
    );
    for state in [
        ReconciledLeaseState::Released,
        ReconciledLeaseState::Unknown,
    ] {
        assert_eq!(
            refusal(
                &exhausted,
                OrchestrationMutation::ReconcileWorktreeLease {
                    lease_id: "lease_escrow".to_owned(),
                    expected_owner_revision: u64::MAX,
                    state,
                    meta: meta(5, "reconcile"),
                },
            ),
            overflow
        );
    }
    let escrowed = reduce_orchestration(
        &exhausted,
        OrchestrationMutation::BeginWorktreeEscrow {
            lease_id: "lease_escrow".to_owned(),
            escrow: prepared_escrow(u64::MAX, at(5)),
            meta: meta(5, "escrow"),
        },
    )
    .unwrap();
    assert_eq!(
        refusal(
            &escrowed,
            OrchestrationMutation::SettleWorktreeEscrow {
                lease_id: "lease_escrow".to_owned(),
                state: SettledEscrowState::Completed,
                meta: meta(6, "settle"),
            },
        ),
        overflow
    );
}

/// The bounded control snapshot keeps 64 agents, so past that the oldest
/// finished workers fall out of it; `worker_session_keys` reads the whole
/// list and still names every worker, and never an orchestrator or the root.
#[test]
fn worker_session_keys_name_every_worker_the_control_snapshot_drops() {
    let mut snapshot = empty();
    let mut root = agent("root", at(0));
    root.role = Some("orchestrator".to_owned());
    snapshot.agents.push(root);
    for index in 1..=69u64 {
        let mut worker = agent(&format!("w{index:02}"), at(index));
        worker.parent_agent_id = Some("root".to_owned());
        worker.depth = 1;
        worker.state = AgentNodeState::Completed;
        snapshot.agents.push(worker);
    }
    let mut lead = agent("lead", at(70));
    lead.parent_agent_id = Some("root".to_owned());
    lead.role = Some("orchestrator".to_owned());
    snapshot.agents.push(lead);
    let mut keyless = agent("keyless", at(71));
    keyless.parent_agent_id = Some("root".to_owned());
    keyless.session_key = None;
    snapshot.agents.push(keyless);

    let control = project_orchestration_snapshot_for_control(&snapshot).unwrap();
    let projected = serde_json::to_value(&control).unwrap();
    let named = projected["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|agent| agent["sessionKey"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(named.len(), 63, "the control snapshot is bounded");
    assert!(!named.contains(&"codex#w01"));

    let keys = worker_session_keys(&snapshot.agents);
    assert_eq!(keys.len(), 69);
    assert_eq!(keys.first().map(String::as_str), Some("codex#w01"));
    assert_eq!(keys.last().map(String::as_str), Some("codex#w69"));
    assert!(is_orchestration_worker(Some("root"), Some("worker")));
    assert!(is_orchestration_worker(Some("root"), None));
    assert!(!is_orchestration_worker(Some("root"), Some("orchestrator")));
    assert!(!is_orchestration_worker(None, Some("worker")));
}
