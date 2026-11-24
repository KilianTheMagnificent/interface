use std::fs;

use interface_collaboration::audit::{
    AuditError, CollaborationAuditSink, aggregate_audit_counters,
};
use interface_collaboration::state::{
    AuditApprovalDecision, CollaborationAuditEventV1, CollaborationEngineId,
    CollaborationErrorCode, CollaborationMode, CollaborationState,
};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn event(
    id: &str,
    revision: u64,
    at: &str,
    from: CollaborationState,
    to: CollaborationState,
) -> CollaborationAuditEventV1 {
    CollaborationAuditEventV1 {
        version: 1,
        event_id: id.to_owned(),
        collaboration_id: "collaboration-1".to_owned(),
        revision,
        at: at.to_owned(),
        mode: CollaborationMode::Consult,
        source_engine: CollaborationEngineId::Claude,
        target_engine: CollaborationEngineId::Codex,
        from,
        to,
        reason_code: None,
        packet_bytes: Some(256),
        result_bytes: None,
        duration_ms: None,
        approval_decision: None,
    }
}

#[test]
#[ignore = "manual performance measurement"]
fn benchmark_audit_snapshot() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("audit");
    let sink = CollaborationAuditSink::open(&root).expect("open");
    let path = sink.log_file();
    drop(sink);
    let mut bytes = Vec::new();
    for index in 0..2_000 {
        let item = event(
            &format!("event-{index}"),
            index + 1,
            "2026-09-04T00:00:00.000Z",
            CollaborationState::Requested,
            CollaborationState::AwaitingApproval,
        );
        serde_json::to_writer(&mut bytes, &item).expect("encode");
        bytes.push(b'\n');
    }
    fs::write(&path, &bytes).expect("fixture");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("mode");
    let sink = CollaborationAuditSink::open(&root).expect("reopen");
    let started = std::time::Instant::now();
    for _ in 0..20 {
        let snapshot = std::hint::black_box(sink.snapshot(&[CollaborationState::Unknown]));
        assert_eq!(snapshot.requests, 2_000);
    }
    eprintln!(
        "audit snapshot: 2,000 events, {} bytes, 20 reads: {:?}",
        bytes.len(),
        started.elapsed()
    );
}

#[test]
fn aggregation_matches_request_approval_queue_and_ambiguity_semantics() {
    let mut events = vec![
        event(
            "event-1",
            1,
            "2026-08-27T12:00:00.000Z",
            CollaborationState::Requested,
            CollaborationState::AwaitingApproval,
        ),
        event(
            "event-2",
            2,
            "2026-08-27T12:00:01.000Z",
            CollaborationState::AwaitingApproval,
            CollaborationState::Approved,
        ),
        event(
            "event-3",
            3,
            "2026-08-27T12:00:02.000Z",
            CollaborationState::ResultReady,
            CollaborationState::AwaitingPeerReview,
        ),
        event(
            "event-4",
            4,
            "2026-08-27T12:00:07.000Z",
            CollaborationState::AwaitingPeerReview,
            CollaborationState::DeliveryPending,
        ),
        event(
            "event-5",
            5,
            "2026-08-27T12:00:08.000Z",
            CollaborationState::DeliveryPending,
            CollaborationState::Unknown,
        ),
    ];
    events[1].approval_decision = Some(AuditApprovalDecision::Approved);
    events[3].result_bytes = Some(900);
    events[4].reason_code = Some(CollaborationErrorCode::DeliveryUnknown);
    events[4].duration_ms = Some(8_000);
    for event in &events {
        event.validate().expect("event");
    }

    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("audit");
    let mut sink = CollaborationAuditSink::open(&root).expect("open");
    for (index, event) in events.iter().enumerate() {
        sink.record(event.clone()).expect("record");
        let records = [
            CollaborationState::Unknown,
            CollaborationState::AwaitingApproval,
        ];
        assert_eq!(
            sink.snapshot(&records),
            aggregate_audit_counters(&events[..=index], &records)
        );
        // Rebuild the same counters and held-at queue across a reopen, including
        // the split between AwaitingPeerReview and DeliveryPending.
        drop(sink);
        sink = CollaborationAuditSink::open(&root).expect("reopen");
        assert_eq!(
            sink.snapshot(&records),
            aggregate_audit_counters(&events[..=index], &records)
        );
    }

    let counters = aggregate_audit_counters(
        &events,
        &[
            CollaborationState::Unknown,
            CollaborationState::AwaitingApproval,
        ],
    );
    assert_eq!(counters.requests, 1);
    assert_eq!(counters.approvals, 1);
    assert_eq!(counters.ambiguous_outcomes, 1);
    assert_eq!(counters.queue_latency_ms.count, 1);
    assert_eq!(counters.queue_latency_ms.sum, 5_000);
    assert_eq!(counters.packet_bytes.count, 5);
    assert_eq!(counters.result_bytes.sum, 900);
    assert_eq!(
        counters.failures_by_code[&CollaborationErrorCode::DeliveryUnknown],
        1
    );
    assert_eq!(counters.active_by_state[&CollaborationState::Unknown], 1);
}

#[test]
fn sink_persists_only_sanitized_projection_with_owner_private_mode() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("audit");
    let mut sink = CollaborationAuditSink::open(&root).expect("open");
    sink.record(event(
        "event-1",
        1,
        "2026-08-27T12:00:00.000Z",
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ))
    .expect("record");
    let text = fs::read_to_string(sink.log_file()).expect("log");
    assert!(text.ends_with('\n'));
    assert!(!text.contains("objective"));
    assert!(!text.contains("sessionKey"));
    assert!(!text.contains("providerSession"));
    assert!(!text.contains("capability"));
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(sink.log_file())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(sink);

    let reopened = CollaborationAuditSink::open(&root).expect("reopen");
    assert_eq!(reopened.snapshot(&[]).requests, 1);
}

#[test]
fn external_log_drift_fails_closed_and_poisoned_sink_cannot_retry() {
    let temp = TempDir::new().expect("temp");
    let mut sink = CollaborationAuditSink::open(temp.path().join("audit")).expect("open");
    sink.record(event(
        "event-1",
        1,
        "2026-08-27T12:00:00.000Z",
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ))
    .expect("record");
    let log = sink.log_file();
    let mut bytes = fs::read(&log).expect("read");
    bytes.extend_from_slice(b" \n");
    fs::write(log, bytes).expect("drift");

    let error = sink
        .record(event(
            "event-2",
            2,
            "2026-08-27T12:00:01.000Z",
            CollaborationState::AwaitingApproval,
            CollaborationState::Approved,
        ))
        .expect_err("drift");
    assert!(matches!(error, AuditError::Storage(_)));
    assert_eq!(sink.snapshot(&[]).requests, 1);
    assert_eq!(sink.snapshot(&[]).approvals, 0);
    assert!(matches!(
        sink.record(event(
            "event-3",
            3,
            "2026-08-27T12:00:02.000Z",
            CollaborationState::Approved,
            CollaborationState::Preflight,
        )),
        Err(AuditError::Poisoned)
    ));
}

#[test]
fn duplicate_json_member_is_refused_without_rewriting_forensic_bytes() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("audit");
    let mut sink = CollaborationAuditSink::open(&root).expect("open");
    sink.record(event(
        "event-1",
        1,
        "2026-08-27T12:00:00.000Z",
        CollaborationState::Requested,
        CollaborationState::AwaitingApproval,
    ))
    .expect("record");
    let log = sink.log_file();
    drop(sink);
    let original = fs::read_to_string(&log).expect("read");
    let corrupt = original.replacen('{', "{\"version\":1,", 1);
    fs::write(&log, &corrupt).expect("corrupt fixture");

    assert!(matches!(
        CollaborationAuditSink::open(&root),
        Err(AuditError::Storage(_))
    ));
    assert_eq!(fs::read_to_string(log).expect("preserved"), corrupt);
}

#[test]
fn audit_sink_enforces_one_writer() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("audit");
    let _first = CollaborationAuditSink::open(&root).expect("first");
    assert!(matches!(
        CollaborationAuditSink::open(&root),
        Err(AuditError::Storage(_))
    ));
}
