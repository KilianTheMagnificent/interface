use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use interface_collaboration::broker::{
    CollaborationModelAuthority, collaboration_source_lineage_id,
};
use interface_collaboration::coordinator::{
    ApprovalAction, CollaborationCoordinator, CoordinatorError, EffectCommand, EffectOutcome,
    EffectSuccess, LockedSessionRole, PacketBuildResult, PeerAction, PendingPeer, PendingPeerState,
    PreparedTarget, SessionParticipants, SourceSnapshot,
};
use interface_collaboration::journal::CollaborationJournal;
use interface_collaboration::state::{
    CollaborationEngineId, CollaborationErrorCode, CollaborationIntentInput,
    CollaborationLiveStatus, CollaborationMode, CollaborationOrigin, CollaborationPolicyOverrides,
    CollaborationState, CollaborationWorktreeOwnerV1, CollaborationWorktreeV1, ManagedWorktreeV1,
    UnmanagedWorktreeV1,
};
use tempfile::TempDir;

const AT: &str = "2026-08-27T12:00:00.000Z";

fn source() -> SourceSnapshot {
    SourceSnapshot {
        engine: CollaborationEngineId::Claude,
        session_key: "claude#source-1".to_owned(),
        provider_session_id: "source-1".to_owned(),
        title: Some("Source".to_owned()),
        registry_revision: 7,
        runtime_epoch: "runtime-epoch-1".to_owned(),
        live_handle_generation: 3,
        live_status: CollaborationLiveStatus::Idle,
        canonical_cwd: "/tmp/project".to_owned(),
        canonical_project_root: "/tmp/project".to_owned(),
        pending_approval_set_hash:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        pending_approval_count: 0,
        queue_revision: 4,
        queue_depth: 0,
        delivery_revision: 5,
        delivery_unknown: false,
        worktree: Some(CollaborationWorktreeV1::Unmanaged(UnmanagedWorktreeV1 {
            version: 1,
            managed: false,
        })),
    }
}

fn packet() -> PacketBuildResult {
    PacketBuildResult {
        body: "packet context".to_owned(),
        context_bytes: 14,
        omissions: 0,
    }
}

fn input(mode: CollaborationMode) -> CollaborationIntentInput {
    CollaborationIntentInput {
        version: Some(1),
        mode,
        source_session_key: "claude#source-1".to_owned(),
        target_engine: CollaborationEngineId::Codex,
        objective: "Review the implementation".to_owned(),
        client_request_id: "request-1".to_owned(),
        origin: CollaborationOrigin::Human,
    }
}

fn coordinator(root: &std::path::Path, at: &'static str) -> CollaborationCoordinator {
    let journal = CollaborationJournal::open(root).expect("journal");
    let ids = Arc::new(Mutex::new(0_u64));
    let id_state = Arc::clone(&ids);
    let mut coordinator = CollaborationCoordinator::with_generators(
        journal,
        None,
        CollaborationPolicyOverrides::default(),
        move || at.to_owned(),
        move |kind| {
            let mut current = id_state.lock().expect("id lock");
            *current += 1;
            format!("{kind}-{current}")
        },
    );
    coordinator.recover().expect("recover");
    coordinator
}

fn submit_and_approve(
    coordinator: &mut CollaborationCoordinator,
) -> (String, SourceSnapshot, PacketBuildResult) {
    let source = source();
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &source, &packet)
        .expect("submit");
    assert_eq!(submitted.state, CollaborationState::AwaitingApproval);
    let preview = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("pending");
    let approved = coordinator
        .approve_human(
            &ApprovalAction {
                collaboration_id: preview.collaboration_id.clone(),
                approval_id: preview.approval_id,
                expected_revision: preview.expected_revision,
            },
            &source,
            &packet,
        )
        .expect("approve");
    assert_eq!(approved.state, CollaborationState::Approved);
    (submitted.collaboration_id, source, packet)
}

#[test]
fn durable_human_preview_resumes_after_a_crash_before_external_reads() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("collaboration");
    let requested = {
        let mut first = coordinator(&root, AT);
        let requested = first
            .begin_human(input(CollaborationMode::Consult))
            .expect("durably begin request");
        assert_eq!(requested.state, CollaborationState::Requested);
        assert_eq!(
            first
                .human_preview_material(&requested.collaboration_id)
                .expect("preview material")
                .objective,
            "Review the implementation"
        );
        requested
    };

    let mut recovered = coordinator(&root, "2026-08-27T12:00:01.000Z");
    assert_eq!(
        recovered
            .human_status(&requested.collaboration_id)
            .expect("recovered request")
            .state,
        CollaborationState::Requested
    );
    let replay = recovered
        .begin_human(input(CollaborationMode::Consult))
        .expect("idempotent begin replay");
    assert_eq!(replay.collaboration_id, requested.collaboration_id);
    assert_eq!(replay.revision, requested.revision);
    let completed = recovered
        .complete_human_preview(&requested.collaboration_id, &source(), &packet())
        .expect("complete preview after recovery");
    assert_eq!(completed.state, CollaborationState::AwaitingApproval);
    assert!(
        recovered
            .approval_preview(&requested.collaboration_id)
            .expect("approval preview")
            .is_some()
    );
}

/// The single-record peer lookup returns exactly the row the full listing
/// holds for that collaboration, and nothing for an unknown id.
fn peer_lookup_matches_listing(
    coordinator: &CollaborationCoordinator,
    id: &str,
) -> Option<PendingPeer> {
    let listed = coordinator.pending_peers().expect("peers");
    assert!(listed.len() <= 1);
    let found = coordinator.pending_peer(id).expect("peer lookup");
    assert_eq!(found, listed.into_iter().next());
    assert_eq!(
        coordinator.pending_peer("unknown").expect("no record"),
        None
    );
    found
}

#[test]
fn only_a_record_awaiting_approval_wakes_the_expiry_sweep() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    assert!(!coordinator.has_awaiting_approval());
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &source(), &packet())
        .expect("submit");
    assert_eq!(submitted.state, CollaborationState::AwaitingApproval);
    assert!(coordinator.has_awaiting_approval());
    let preview = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("pending");
    coordinator
        .approve_human(
            &ApprovalAction {
                collaboration_id: preview.collaboration_id,
                approval_id: preview.approval_id,
                expected_revision: preview.expected_revision,
            },
            &source(),
            &packet(),
        )
        .expect("approve");
    assert!(!coordinator.has_awaiting_approval());
}

#[test]
fn full_consult_flow_commits_every_attempt_before_command_and_prunes_terminal_bodies() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("collaboration");
    let mut coordinator = coordinator(&root, AT);
    let (id, source, packet) = submit_and_approve(&mut coordinator);

    let preparing = coordinator
        .reserve_prepare(&id, &source, &packet)
        .expect("reserve prepare");
    assert_eq!(preparing.state, CollaborationState::PreparingTarget);
    assert_eq!(preparing.revision, 4);

    let prepare = coordinator
        .start_reserved_effect(&id, Some(&source), Some(&packet))
        .expect("start prepare")
        .expect("command");
    assert!(matches!(prepare, EffectCommand::PrepareTarget { .. }));
    assert_eq!(
        coordinator.human_status(&id).expect("status").revision,
        5,
        "started attempt must be durable before the command is returned"
    );
    coordinator
        .settle_effect(
            &id,
            prepare.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::PreparedTarget(PreparedTarget {
                    session_key: "codex#target-1".to_owned(),
                    provider_session_id: "target-1".to_owned(),
                    reconciliation_marker: None,
                }),
                provider_effect_id: Some("provider-prepare-1".to_owned()),
            },
        )
        .expect("settle prepare");

    let run = coordinator
        .start_reserved_effect(&id, None, None)
        .expect("start run")
        .expect("run command");
    assert!(matches!(run, EffectCommand::RunTarget { .. }));
    coordinator
        .settle_effect(
            &id,
            run.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::ConsultResult("peer result".to_owned()),
                provider_effect_id: Some("provider-run-1".to_owned()),
            },
        )
        .expect("result ready");
    coordinator.hold_peer_result(&id).expect("hold");

    let peer = peer_lookup_matches_listing(&coordinator, &id).expect("peer");
    coordinator
        .accept_peer_human(
            &PeerAction {
                collaboration_id: id.clone(),
                result_id: peer.result_id,
                result_digest: peer.result_digest,
                expected_revision: peer.expected_revision,
            },
            &source,
        )
        .expect("accept");
    assert!(peer_lookup_matches_listing(&coordinator, &id).is_some());
    let deliver = coordinator
        .start_reserved_effect(&id, Some(&source), None)
        .expect("start delivery")
        .expect("delivery command");
    assert_eq!(peer_lookup_matches_listing(&coordinator, &id), None);
    assert!(matches!(deliver, EffectCommand::DeliverResult { .. }));
    let completed = coordinator
        .settle_effect(
            &id,
            deliver.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::Unit,
                provider_effect_id: Some("provider-delivery-1".to_owned()),
            },
        )
        .expect("complete");
    assert_eq!(completed.state, CollaborationState::Completed);
    assert_eq!(completed.revision, 12);
    drop(coordinator);

    let reopened = CollaborationJournal::open(&root).expect("reopen");
    let record = reopened.get(&id).expect("get").expect("record");
    assert_eq!(record.state, CollaborationState::Completed);
    assert!(record.intent.objective.is_none());
    assert!(record.packet.as_ref().expect("packet").body.is_none());
    assert!(record.peer_result.as_ref().expect("peer").body.is_none());
}

#[test]
fn crashed_started_effect_recovers_to_unknown_and_is_never_redispatched() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("collaboration");
    let mut first = coordinator(&root, AT);
    let (id, source, packet) = submit_and_approve(&mut first);
    first
        .reserve_prepare(&id, &source, &packet)
        .expect("reserve");
    let command = first
        .start_reserved_effect(&id, Some(&source), Some(&packet))
        .expect("start")
        .expect("command");
    let effect_id = command.effect_id().to_owned();
    drop(first);

    let mut recovered = coordinator(&root, "2026-08-27T12:00:01.000Z");
    let status = recovered.human_status(&id).expect("status");
    assert_eq!(status.state, CollaborationState::Unknown);
    assert_eq!(
        status.error.as_ref().map(|error| error.code),
        Some(CollaborationErrorCode::ProviderOutcomeUnknown)
    );
    assert!(matches!(
        recovered.start_reserved_effect(&id, Some(&source), Some(&packet)),
        Err(CoordinatorError::Ingress {
            code: CollaborationErrorCode::RecoveryRequired,
            collaboration_id: Some(ref collaboration_id),
            retryable: false,
        }) if collaboration_id == &id
    ));
    drop(recovered);
    let reopened = CollaborationJournal::open(&root).expect("reopen");
    let record = reopened.get(&id).expect("get").expect("record");
    let effect = record.effects.prepare_target.expect("effect");
    assert_eq!(effect.effect_id, effect_id);
    assert_eq!(effect.attempts, 1);
    assert_eq!(
        effect.status,
        interface_collaboration::state::CollaborationEffectStatus::Unknown
    );
}

#[test]
fn approval_refresh_changes_single_use_id_and_old_action_stays_stale() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let source = source();
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &source, &packet)
        .expect("submit");
    let original = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("pending");
    let old_action = ApprovalAction {
        collaboration_id: original.collaboration_id.clone(),
        approval_id: original.approval_id.clone(),
        expected_revision: original.expected_revision,
    };
    let mut changed = source.clone();
    changed.registry_revision += 1;
    let refreshed = coordinator
        .approve_human(&old_action, &changed, &packet)
        .expect("refresh");
    assert_eq!(refreshed.state, CollaborationState::AwaitingApproval);
    let replacement = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("replacement");
    assert_ne!(replacement.approval_id, original.approval_id);
    assert!(matches!(
        coordinator.approve_human(&old_action, &changed, &packet),
        Err(CoordinatorError::Ingress {
            code: CollaborationErrorCode::ApprovalStale,
            ..
        })
    ));
}

#[test]
fn failed_source_during_approval_is_committed_as_a_durable_known_failure() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("collaboration");
    let mut coordinator = coordinator(&root, AT);
    let source = source();
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &source, &packet)
        .expect("submit");
    let preview = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("approval");
    let mut failed_source = source;
    failed_source.live_status = CollaborationLiveStatus::Failed;
    let failed = coordinator
        .approve_human(
            &ApprovalAction {
                collaboration_id: preview.collaboration_id,
                approval_id: preview.approval_id,
                expected_revision: preview.expected_revision,
            },
            &failed_source,
            &packet,
        )
        .expect("durable source failure");
    assert_eq!(failed.state, CollaborationState::Failed);
    assert_eq!(
        failed.error.as_ref().map(|error| error.code),
        Some(CollaborationErrorCode::SourceNotLive)
    );
    drop(coordinator);

    let reopened = CollaborationJournal::open(&root).expect("reopen");
    assert_eq!(
        reopened
            .get(&submitted.collaboration_id)
            .expect("get")
            .expect("record")
            .state,
        CollaborationState::Failed
    );
}

#[test]
fn failed_source_before_reserved_dispatch_fails_without_consuming_the_effect() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let (id, source, packet) = submit_and_approve(&mut coordinator);
    coordinator
        .reserve_prepare(&id, &source, &packet)
        .expect("reserve");
    let mut failed_source = source;
    failed_source.live_status = CollaborationLiveStatus::Failed;
    assert!(
        coordinator
            .start_reserved_effect(&id, Some(&failed_source), Some(&packet))
            .expect("known failure")
            .is_none()
    );
    let status = coordinator.human_status(&id).expect("status");
    assert_eq!(status.state, CollaborationState::Failed);
    assert_eq!(
        status.error.as_ref().map(|error| error.code),
        Some(CollaborationErrorCode::SourceNotLive)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn full_unmanaged_handoff_parks_transfers_and_activates_in_durable_order() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("collaboration");
    let mut coordinator = coordinator(&root, AT);
    let source = source();
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Handoff), &source, &packet)
        .expect("submit handoff");
    let preview = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("approval");
    coordinator
        .approve_human(
            &ApprovalAction {
                collaboration_id: preview.collaboration_id.clone(),
                approval_id: preview.approval_id,
                expected_revision: preview.expected_revision,
            },
            &source,
            &packet,
        )
        .expect("approve handoff");
    coordinator
        .reserve_prepare(&submitted.collaboration_id, &source, &packet)
        .expect("reserve prepare");

    let prepare = coordinator
        .start_reserved_effect(&submitted.collaboration_id, Some(&source), Some(&packet))
        .expect("start prepare")
        .expect("prepare command");
    assert!(matches!(prepare, EffectCommand::PrepareTarget { .. }));
    let parked = coordinator
        .settle_effect(
            &submitted.collaboration_id,
            prepare.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::PreparedTarget(PreparedTarget {
                    session_key: "codex#handoff-target-1".to_owned(),
                    provider_session_id: "handoff-target-1".to_owned(),
                    reconciliation_marker: None,
                }),
                provider_effect_id: Some("provider-prepare-handoff".to_owned()),
            },
        )
        .expect("settle prepare");
    assert_eq!(parked.state, CollaborationState::ParkingSource);

    let park = coordinator
        .start_reserved_effect(&submitted.collaboration_id, Some(&source), Some(&packet))
        .expect("start park")
        .expect("park command");
    assert!(matches!(park, EffectCommand::ParkSource { .. }));
    let transferring = coordinator
        .settle_effect(
            &submitted.collaboration_id,
            park.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::Unit,
                provider_effect_id: Some("provider-park-handoff".to_owned()),
            },
        )
        .expect("settle park");
    assert_eq!(transferring.state, CollaborationState::OwnershipTransfer);

    let transfer = coordinator
        .start_reserved_effect(&submitted.collaboration_id, None, None)
        .expect("start transfer")
        .expect("transfer command");
    assert!(matches!(transfer, EffectCommand::TransferOwnership { .. }));
    let activating = coordinator
        .settle_effect(
            &submitted.collaboration_id,
            transfer.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::Unit,
                provider_effect_id: Some("provider-transfer-handoff".to_owned()),
            },
        )
        .expect("settle transfer");
    assert_eq!(activating.state, CollaborationState::ActivatingTarget);

    let activate = coordinator
        .start_reserved_effect(&submitted.collaboration_id, None, None)
        .expect("start activation")
        .expect("activation command");
    assert!(matches!(activate, EffectCommand::ActivateTarget { .. }));
    let completed = coordinator
        .settle_effect(
            &submitted.collaboration_id,
            activate.effect_id(),
            EffectOutcome::Succeeded {
                value: EffectSuccess::Unit,
                provider_effect_id: Some("provider-activate-handoff".to_owned()),
            },
        )
        .expect("settle activation");
    assert_eq!(completed.state, CollaborationState::Completed);
    assert_eq!(completed.revision, 12);
    drop(coordinator);

    let reopened = CollaborationJournal::open(&root).expect("reopen");
    let record = reopened
        .get(&submitted.collaboration_id)
        .expect("get")
        .expect("record");
    assert_eq!(record.state, CollaborationState::Completed);
    assert!(record.intent.objective.is_none());
    assert!(record.packet.as_ref().expect("packet").body.is_none());
    assert_eq!(
        record
            .effects
            .park_source
            .as_ref()
            .expect("park effect")
            .attempts,
        1
    );
    assert_eq!(
        record
            .effects
            .transfer_ownership
            .as_ref()
            .expect("transfer effect")
            .attempts,
        1
    );
    assert_eq!(
        record
            .effects
            .activate_target
            .as_ref()
            .expect("activate effect")
            .attempts,
        1
    );
}

#[test]
fn model_status_requires_exact_source_lineage_not_merely_same_engine() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let source = source();
    let lineage = collaboration_source_lineage_id(&source.session_key);
    let authority = CollaborationModelAuthority {
        engine: CollaborationEngineId::Claude,
        source_session_keys: vec![source.session_key.clone()],
        source_lineage_ids: BTreeMap::from([(source.session_key.clone(), lineage)]),
        depth: 0,
    };
    let submitted = coordinator
        .submit_model(
            input(CollaborationMode::Consult),
            &authority,
            &source,
            &packet(),
        )
        .expect("submit model");
    assert!(
        coordinator
            .model_status(&submitted.collaboration_id, &authority)
            .is_ok()
    );
    let wrong = CollaborationModelAuthority {
        engine: CollaborationEngineId::Claude,
        source_session_keys: vec![source.session_key.clone()],
        source_lineage_ids: BTreeMap::from([(
            source.session_key.clone(),
            "different-stable-lineage".to_owned(),
        )]),
        depth: 0,
    };
    assert!(matches!(
        coordinator.model_status(&submitted.collaboration_id, &wrong),
        Err(CoordinatorError::Ingress {
            code: CollaborationErrorCode::SourceNotFound,
            ..
        })
    ));
}

#[test]
fn ambiguous_outcome_is_non_retryable_and_cancellation_is_not_safe() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let (id, source, packet) = submit_and_approve(&mut coordinator);
    coordinator
        .reserve_prepare(&id, &source, &packet)
        .expect("reserve");
    let command = coordinator
        .start_reserved_effect(&id, Some(&source), Some(&packet))
        .expect("start")
        .expect("command");
    let receipt = coordinator
        .settle_effect(
            &id,
            command.effect_id(),
            EffectOutcome::Ambiguous {
                code: Some(CollaborationErrorCode::BrokerUnavailable),
                provider_effect_id: Some("provider-maybe-1".to_owned()),
            },
        )
        .expect("unknown");
    assert_eq!(receipt.state, CollaborationState::Unknown);
    assert!(!receipt.cancellation_safe);
    assert_eq!(
        receipt.error.as_ref().map(|error| error.retryable),
        Some(false)
    );
    let unchanged = coordinator.cancel_human(&id).expect("cancel no-op");
    assert_eq!(unchanged.state, CollaborationState::Unknown);
}

#[test]
fn renderer_safe_projection_wire_names_are_exact_and_strict() {
    let action = ApprovalAction {
        collaboration_id: "collaboration-1".to_owned(),
        approval_id: "approval-1".to_owned(),
        expected_revision: 7,
    };
    assert_eq!(
        serde_json::to_string(&action).expect("serialize"),
        r#"{"collaborationId":"collaboration-1","approvalId":"approval-1","expectedRevision":7}"#
    );
    assert!(
        serde_json::from_str::<ApprovalAction>(
            r#"{"collaborationId":"collaboration-1","approvalId":"approval-1","expectedRevision":7,"extra":true}"#,
        )
        .is_err()
    );
    assert_eq!(
        serde_json::to_string(&PendingPeerState::AwaitingReview).expect("peer state"),
        r#""awaiting-review""#
    );
    assert_eq!(
        serde_json::to_string(&LockedSessionRole::Target).expect("role"),
        r#""target""#
    );
    assert_eq!(
        serde_json::to_string(&SessionParticipants {
            source_session_key: "claude#source-1".to_owned(),
            target_session_key: None,
        })
        .expect("participants"),
        r#"{"sourceSessionKey":"claude#source-1"}"#
    );
}

#[test]
fn idempotent_replay_precedes_capacity_locks_and_external_preview_resolution() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let source = source();
    let first_input = input(CollaborationMode::Consult);
    let first = coordinator
        .submit_human(first_input.clone(), &source, &packet())
        .expect("first");

    let mut second_input = input(CollaborationMode::Consult);
    second_input.client_request_id = "request-2".to_owned();
    coordinator
        .submit_human(second_input, &source, &packet())
        .expect("fill source consultation capacity");

    let mut unusable_source = source;
    unusable_source.session_key = "claude#wrong-source".to_owned();
    unusable_source.provider_session_id = "wrong-source".to_owned();
    unusable_source.live_status = CollaborationLiveStatus::Failed;
    let unusable_packet = PacketBuildResult {
        body: "x".repeat(40 * 1_024),
        context_bytes: 0,
        omissions: 0,
    };
    let replay = coordinator
        .submit_human(first_input, &unusable_source, &unusable_packet)
        .expect("replay must not re-resolve external preview inputs");
    assert_eq!(replay.collaboration_id, first.collaboration_id);
    assert_eq!(replay.revision, first.revision);
    assert_eq!(replay.state, first.state);
}

#[test]
fn absent_snapshot_worktree_matches_the_normalized_record_and_still_approves() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let mut source = source();
    source.worktree = None;
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &source, &packet)
        .expect("submit");
    let preview = coordinator
        .approval_preview(&submitted.collaboration_id)
        .expect("preview")
        .expect("pending");
    let approved = coordinator
        .approve_human(
            &ApprovalAction {
                collaboration_id: preview.collaboration_id.clone(),
                approval_id: preview.approval_id,
                expected_revision: preview.expected_revision,
            },
            &source,
            &packet,
        )
        .expect("approve");
    assert_eq!(approved.state, CollaborationState::Approved);
}

/// The host always sends a worktree, but the coordinator API treats an
/// absent one as unmanaged everywhere: a refresh against a record bound to a
/// managed worktree records the normalized value, so the next approval binds
/// instead of refreshing (or failing validation) forever.
#[test]
fn approval_refresh_records_the_normalized_absent_worktree() {
    let temp = TempDir::new().expect("temp");
    let mut coordinator = coordinator(&temp.path().join("collaboration"), AT);
    let managed = SourceSnapshot {
        worktree: Some(CollaborationWorktreeV1::Managed(ManagedWorktreeV1 {
            version: 1,
            managed: true,
            canonical_path: "/tmp/project-worktree".to_owned(),
            branch: "feature/review".to_owned(),
            owner_revision: 2,
            transfer_id: None,
            release_in_progress: false,
            owner: CollaborationWorktreeOwnerV1::Source {
                session_key: "claude#source-1".to_owned(),
            },
        })),
        ..source()
    };
    let packet = packet();
    let submitted = coordinator
        .submit_human(input(CollaborationMode::Consult), &managed, &packet)
        .expect("submit");
    let id = submitted.collaboration_id;
    let absent = SourceSnapshot {
        worktree: None,
        ..source()
    };
    let action = |coordinator: &mut CollaborationCoordinator| {
        let preview = coordinator
            .approval_preview(&id)
            .expect("preview")
            .expect("pending");
        ApprovalAction {
            collaboration_id: preview.collaboration_id,
            approval_id: preview.approval_id,
            expected_revision: preview.expected_revision,
        }
    };
    let first = action(&mut coordinator);
    let refreshed = coordinator
        .approve_human(&first, &absent, &packet)
        .expect("refresh");
    assert_eq!(refreshed.state, CollaborationState::AwaitingApproval);
    assert_eq!(
        coordinator.runtime_record(&id).expect("record").worktree,
        Some(CollaborationWorktreeV1::Unmanaged(UnmanagedWorktreeV1 {
            version: 1,
            managed: false,
        }))
    );
    let second = action(&mut coordinator);
    let approved = coordinator
        .approve_human(&second, &absent, &packet)
        .expect("approve");
    assert_eq!(approved.state, CollaborationState::Approved);
}
