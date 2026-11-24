use super::*;
use crate::audit::{CollaborationAuditSink, aggregate_audit_counters};
use crate::state::{CollaborationAuditEventV1, CollaborationState};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn event(
    revision: u64,
    from: CollaborationState,
    to: CollaborationState,
) -> CollaborationAuditEventV1 {
    serde_json::from_value(
        serde_json::json!({"version":1,"eventId":format!("event-{revision}"),
        "collaborationId":"fixture","revision":revision,"at":"2026-09-14T00:00:10.000Z",
        "mode":"consult","sourceEngine":"claude","targetEngine":"codex","from":from,"to":to}),
    )
    .unwrap()
}
fn write_private(path: &std::path::Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn line(event: &CollaborationAuditEventV1) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(event).unwrap();
    bytes.push(b'\n');
    bytes
}
#[test]
fn lossless_rotations_preserve_legacy_details_and_lifetime_projection() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::open_or_create(temp.path().join("audit")).unwrap();
    let first = event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    );
    let legacy = line(&first);
    write_private(&root.path().join(AUDIT_LOG), &legacy);
    let mut events = vec![first];
    let mut sealed = Vec::new();
    for index in 0..4 {
        let mut sink = CollaborationAuditSink::open(root.path()).unwrap();
        let item = event(
            index * 2 + 2,
            CollaborationState::Requested,
            CollaborationState::AwaitingApproval,
        );
        sink.record(item.clone()).unwrap();
        events.push(item);
        let active = sink.log_file();
        drop(sink);
        let mut bytes = fs::read(&active).unwrap();
        let mut held = event(
            index * 2 + 3,
            CollaborationState::ResultReady,
            CollaborationState::AwaitingPeerReview,
        );
        held.at = "2026-09-14T00:00:00.000Z".into();
        bytes.extend(line(&held));
        events.push(held);
        bytes.resize(SEGMENT_BYTES, b'\n');
        write_private(&active, &bytes);
        sealed.push((active, bytes));
        let mut sink = CollaborationAuditSink::open(root.path()).unwrap();
        let delivered = event(
            index * 2 + 4,
            CollaborationState::AwaitingPeerReview,
            CollaborationState::DeliveryPending,
        );
        sink.record(delivered.clone()).unwrap();
        events.push(delivered);
        assert_eq!(sink.snapshot(&[]), aggregate_audit_counters(&events, &[]));
        drop(sink);
        assert_eq!(
            CollaborationAuditSink::open(root.path())
                .unwrap()
                .snapshot(&[]),
            aggregate_audit_counters(&events, &[])
        );
    }
    assert_eq!(fs::read(root.path().join(AUDIT_LOG)).unwrap(), legacy);
    for (path, bytes) in sealed {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| e.to == CollaborationState::DeliveryPending)
            .count(),
        4
    );
}
#[test]
fn publication_boundary_recovers_prepared_and_selected_empty_segments_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).unwrap();
    let item = event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    );
    sink.record(item.clone()).unwrap();
    let next = segment(1);
    Storage::prepare_empty(&sink.root, &sink.lock, &next).unwrap();
    let path = sink.root.path().to_owned();
    drop(sink);
    let mut sink = CollaborationAuditSink::open(&path).unwrap();
    assert_eq!(sink.snapshot(&[]).requests, 1);
    assert_eq!(sink.storage.name, segment(0));
    sink.storage
        .rotate(&sink.root, &sink.lock, &sink.projection)
        .unwrap();
    drop(sink);
    let mut sink = CollaborationAuditSink::open(&path).unwrap();
    assert_eq!(sink.storage.name, next);
    assert_eq!(sink.snapshot(&[]).requests, 1);
    sink.record(item).unwrap();
    drop(sink);
    assert_eq!(
        CollaborationAuditSink::open(&path)
            .unwrap()
            .snapshot(&[])
            .requests,
        2
    );
}
#[test]
fn checkpoint_schema_and_drift_refuse_without_overwriting_evidence() {
    for mutation in [
        "version",
        "unknownField",
        "missingActive",
        "sealed",
        "legacy",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).unwrap();
        sink.record(event(
            1,
            CollaborationState::Requested,
            CollaborationState::AwaitingApproval,
        ))
        .unwrap();
        sink.storage
            .rotate(&sink.root, &sink.lock, &sink.projection)
            .unwrap();
        let root = sink.root.path().to_owned();
        let active = sink.log_file();
        drop(sink);
        let checkpoint = root.join(CHECKPOINT);
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&checkpoint).unwrap()).unwrap();
        match mutation {
            "version" => {
                state["version"] = 99.into();
                write_private(&checkpoint, &serde_json::to_vec(&state).unwrap());
            }
            "unknownField" => {
                state["extra"] = true.into();
                write_private(&checkpoint, &serde_json::to_vec(&state).unwrap());
            }
            "missingActive" => {
                fs::remove_file(active).unwrap();
            }
            "sealed" => write_private(&root.join(segment(0)), b" \n"),
            "legacy" => write_private(&root.join(AUDIT_LOG), b" \n"),
            _ => unreachable!(),
        }
        let forensic = fs::read(&checkpoint).unwrap();
        assert!(CollaborationAuditSink::open(&root).is_err(), "{mutation}");
        assert_eq!(fs::read(checkpoint).unwrap(), forensic);
    }
}
#[test]
fn live_checkpoint_drift_and_oversize_events_poison_before_projection_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).unwrap();
    let item = event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    );
    sink.record(item.clone()).unwrap();
    let active = fs::read(sink.log_file()).unwrap();
    assert!(
        sink.storage
            .append(
                &sink.root,
                &sink.lock,
                &sink.projection,
                &vec![b' '; SEGMENT_BYTES + 1]
            )
            .is_err()
    );
    let checkpoint = sink.root.path().join(CHECKPOINT);
    let mut bytes = fs::read(&checkpoint).unwrap();
    bytes.push(b' ');
    write_private(&checkpoint, &bytes);
    assert!(sink.record(item.clone()).is_err());
    assert!(matches!(sink.record(item), Err(AuditError::Poisoned)));
    assert_eq!(sink.snapshot(&[]).requests, 1);
    assert_eq!(fs::read(sink.log_file()).unwrap(), active);
    assert_eq!(fs::read(checkpoint).unwrap(), bytes);
}
#[test]
fn a_nonempty_unreferenced_segment_is_never_replayed_or_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).unwrap();
    let path = sink.root.path().join(segment(0));
    let bytes = line(&event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ));
    write_private(&path, &bytes);
    assert!(
        sink.record(event(
            2,
            CollaborationState::Requested,
            CollaborationState::AwaitingApproval
        ))
        .is_err()
    );
    assert_eq!(sink.snapshot(&[]).requests, 0);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn near_capacity_legacy_import_preserves_details_and_uses_small_segments() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::open_or_create(temp.path().join("audit")).unwrap();
    let item = event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    );
    let mut legacy = line(&item);
    legacy.resize(MAX_AUDIT_BYTES - 1, b' ');
    let path = root.path().join(AUDIT_LOG);
    write_private(&path, &legacy);
    let mut sink = CollaborationAuditSink::open(root.path()).unwrap();
    assert!(!root.path().join(CHECKPOINT).exists());
    sink.record(item).unwrap();
    assert_eq!(sink.snapshot(&[]).requests, 2);
    assert!(fs::metadata(sink.log_file()).unwrap().len() < 1024);
    assert_eq!(fs::read(path).unwrap(), legacy);
    drop(sink);
    assert_eq!(
        CollaborationAuditSink::open(root.path())
            .unwrap()
            .snapshot(&[])
            .requests,
        2
    );
}

#[test]
fn checkpoint_capacity_failure_leaves_committed_log_readable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("audit");
    let mut sink = CollaborationAuditSink::open(&path).unwrap();
    sink.record(event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ))
    .unwrap();
    let checkpoint = fs::read(path.join(CHECKPOINT)).unwrap();
    let mut excessive = sink.projection.clone();
    excessive.held_at.insert("x".repeat(CHECKPOINT_BYTES), 0);
    assert!(
        sink.storage
            .rotate(&sink.root, &sink.lock, &excessive)
            .is_err()
    );
    assert_eq!(fs::read(path.join(CHECKPOINT)).unwrap(), checkpoint);
    drop(sink);
    assert_eq!(
        CollaborationAuditSink::open(&path)
            .unwrap()
            .snapshot(&[])
            .requests,
        1
    );
}

#[cfg(unix)]
#[test]
fn selected_segment_symlinks_are_refused_without_following_them() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("audit");
    let mut sink = CollaborationAuditSink::open(&path).unwrap();
    sink.record(event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ))
    .unwrap();
    let active = sink.log_file();
    drop(sink);
    let preserved = temp.path().join("preserved.jsonl");
    fs::rename(&active, &preserved).unwrap();
    let bytes = fs::read(&preserved).unwrap();
    std::os::unix::fs::symlink(&preserved, active).unwrap();
    assert!(CollaborationAuditSink::open(&path).is_err());
    assert_eq!(fs::read(preserved).unwrap(), bytes);
}

#[test]
fn checkpoint_v2_shape_is_frozen_and_rejects_duplicate_members() {
    let checkpoint = Checkpoint {
        version: 2,
        active_segment: 0,
        legacy_sha256: None,
        preceding_sha256: None,
        projection: AuditProjection::default(),
    };
    let expected = r#"{"version":2,"activeSegment":0,"legacySha256":null,"precedingSha256":null,"projection":{"counters":{"requests":0,"approvals":0,"denials":0,"expiredApprovals":0,"activeByState":{},"failuresByCode":{},"ambiguousOutcomes":0,"recoveryActions":0,"queueLatencyMs":{"count":0,"sum":0},"providerDurationMs":{"count":0,"sum":0},"packetBytes":{"count":0,"sum":0},"resultBytes":{"count":0,"sum":0}},"heldAt":{}}}"#;
    assert_eq!(serde_json::to_string(&checkpoint).unwrap(), expected);
    let duplicate = expected.replacen("\"version\":2", "\"version\":2,\"version\":2", 1);
    assert!(
        interface_storage::decode_json_strict::<Checkpoint>(
            duplicate.as_bytes(),
            limits(),
            CHECKPOINT
        )
        .is_err()
    );
}

#[test]
fn losing_writer_lease_preserves_events_and_poisons_the_sink() {
    let temp = tempfile::tempdir().unwrap();
    let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).unwrap();
    let item = event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    );
    sink.record(item.clone()).unwrap();
    let before = fs::read(sink.log_file()).unwrap();
    let lock = sink.root.path().join(crate::audit::AUDIT_LOCK);
    fs::rename(&lock, sink.root.path().join("preserved.lock")).unwrap();
    write_private(&lock, b"");
    assert!(sink.record(item.clone()).is_err());
    assert!(matches!(sink.record(item), Err(AuditError::Poisoned)));
    assert_eq!(sink.snapshot(&[]).requests, 1);
    assert_eq!(fs::read(sink.log_file()).unwrap(), before);
}

#[test]
fn every_peer_review_exit_ends_the_hold() {
    let mut projection = AuditProjection::default();
    projection.record(&event(
        1,
        CollaborationState::Requested,
        CollaborationState::AwaitingPeerReview,
    ));
    assert_eq!(projection.held_at.len(), 1);
    // A discarded, cancelled or failed consultation leaves no checkpoint entry.
    projection.record(&event(
        2,
        CollaborationState::AwaitingPeerReview,
        CollaborationState::Cancelled,
    ));
    assert!(projection.held_at.is_empty());
    // Delivery still measures queue latency exactly as before.
    projection.record(&event(
        3,
        CollaborationState::Requested,
        CollaborationState::AwaitingPeerReview,
    ));
    projection.record(&event(
        4,
        CollaborationState::AwaitingPeerReview,
        CollaborationState::DeliveryPending,
    ));
    assert!(projection.held_at.is_empty());
    let counters = serde_json::to_value(&projection.counters).unwrap();
    assert_eq!(counters["queueLatencyMs"]["count"], 1);
}
