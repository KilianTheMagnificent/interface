use interface_contracts::engine::{
    EngineEvent, EngineEventKind, EngineId, SessionStatus, SessionSummary,
};

use super::*;

#[test]
fn byte_budgets_release_closed_transcripts_but_keep_approval_and_uncertainty() {
    let mut store = EngineHistoryStore::open(
        "/unused",
        HistoryLimits {
            bytes_per_session: 64 * 1024,
            bytes_total: 96 * 1024,
            ..HistoryLimits::default()
        },
    )
    .unwrap();
    for index in 0..4 {
        let key = format!("codex#{index}");
        store
            .register_session(key.clone(), summary(&index.to_string()), "at".into())
            .unwrap();
        for _ in 0..16 {
            store
                .append_event(
                    &key,
                    EngineEvent {
                        at: None,
                        event: EngineEventKind::MessageDelta {
                            text: "界".repeat(4096),
                            agent_key: None,
                        },
                    },
                    "at".into(),
                )
                .unwrap();
            assert!(store.total_bytes <= 96 * 1024);
            assert!(store.retained_bytes[&key] <= 64 * 1024);
        }
        store
            .append_event(
                &key,
                EngineEvent {
                    at: None,
                    event: EngineEventKind::ApprovalRequested {
                        approval_id: format!("approval-{index}"),
                        tool: "fixture".into(),
                        summary: "Review exact input".into(),
                        input: serde_json::json!({"text":"x".repeat(80 * 1024)}),
                    },
                },
                "at".into(),
            )
            .unwrap();
        store
            .append_effect(
                &key,
                "send".into(),
                RuntimeEffectKind::Send,
                "at".into(),
                &DeliveryOutcome::OutcomeUnknown(EngineError::outcome_unknown(
                    interface_engine::EngineErrorKind::Transport,
                    "unknown",
                    false,
                )),
            )
            .unwrap();
        store
            .mark_stream_terminal(&key, RuntimeStreamState::Closed, None, "at".into())
            .unwrap();
        let closed = store.session(&key).unwrap();
        assert_eq!(store.retained_bytes[&key], 0);
        assert!(
            closed
                .events
                .iter()
                .all(|e| !matches!(e.event.event, EngineEventKind::MessageDelta { .. }))
        );
        assert!(closed.events.iter().any(|e| matches!(&e.event.event,
            EngineEventKind::ApprovalRequested { input, .. } if input["text"].as_str().unwrap().len() == 80 * 1024)));
        assert_eq!(closed.delivery_state, RuntimeDeliveryState::OutcomeUnknown);
        assert_eq!(
            closed.effects.back().unwrap().disposition,
            EffectAuditDisposition::OutcomeUnknown
        );
        assert!(closed.omitted_events_through.is_some());
    }
    assert_eq!(store.total_bytes, 0);
    assert_eq!(store.snapshot.sessions.len(), 4);
}

#[test]
fn global_budget_compacts_old_live_content_and_preserves_latest_control_facts() {
    let mut store = EngineHistoryStore::open(
        "/unused",
        HistoryLimits {
            bytes_per_session: 32 * 1024,
            bytes_total: 24 * 1024,
            ..HistoryLimits::default()
        },
    )
    .unwrap();
    for index in 0..8 {
        let key = format!("codex#{index}");
        store
            .register_session(key.clone(), summary(&index.to_string()), format!("{index}"))
            .unwrap();
        for model in ["old", "current"] {
            store
                .append_event(
                    &key,
                    EngineEvent {
                        at: None,
                        event: EngineEventKind::ConfigChanged {
                            model: Some(model.into()),
                            effort: None,
                        },
                    },
                    format!("{index}"),
                )
                .unwrap();
        }
        store
            .append_event(
                &key,
                EngineEvent {
                    at: None,
                    event: EngineEventKind::ToolCall {
                        call_id: "call".into(),
                        tool: "fixture".into(),
                        summary: "content".into(),
                        input: serde_json::json!({"data":"x".repeat(12 * 1024)}),
                        agent_key: None,
                    },
                },
                format!("{index}"),
            )
            .unwrap();
        assert!(store.total_bytes <= 24 * 1024);
    }
    assert!(
        store
            .session("codex#0")
            .unwrap()
            .omitted_events_through
            .is_some()
    );
    for session in &store.snapshot.sessions {
        assert_eq!(session.summary.model.as_deref(), Some("current"));
        assert!(session.events.iter().any(|e| matches!(&e.event.event,
            EngineEventKind::ConfigChanged { model: Some(model), .. } if model == "current")));
    }
    assert_eq!(
        store.total_bytes,
        store.retained_bytes.values().sum::<usize>()
    );
}

fn summary(session_id: &str) -> SessionSummary {
    SessionSummary {
        engine: EngineId::new("codex").expect("engine"),
        session_id: session_id.into(),
        title: session_id.into(),
        cwd: "/work".into(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: "2026-08-27T00:00:00.000Z".into(),
        first_seen: "2026-08-27T00:00:00.000Z".into(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: None,
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

#[test]
fn live_projection_never_creates_or_reloads_an_authoritative_history_file() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let absent_root = temporary.path().join("unwritable-or-absent/history");
    let mut first = EngineHistoryStore::open(&absent_root, HistoryLimits::default())
        .expect("process-local projection");
    first
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("live registration");
    assert!(first.session("codex#one").is_some());
    assert!(!absent_root.exists());

    let reopened = EngineHistoryStore::open(&absent_root, HistoryLimits::default())
        .expect("fresh process-local projection");
    assert!(reopened.session("codex#one").is_none());
    assert!(!absent_root.exists());
}

#[test]
fn history_session_json_keeps_its_frozen_camel_case_arrays() {
    let session = HistorySession {
        key: "codex#one".into(),
        summary: summary("one"),
        attachment_count: 1,
        last_attached_at: "2026-08-27T00:00:00.000Z".into(),
        next_event_sequence: 2,
        next_effect_sequence: 2,
        omitted_events_through: None,
        omitted_effects_through: None,
        stream_state: RuntimeStreamState::Active,
        delivery_state: RuntimeDeliveryState::Open,
        terminal_message: None,
        events: VecDeque::from([HistoryEvent {
            sequence: 1,
            received_at: "2026-08-27T00:00:00.000Z".into(),
            event: EngineEvent {
                at: None,
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            },
        }]),
        effects: VecDeque::from([HistoryEffect {
            sequence: 1,
            effect_id: "effect-one".into(),
            kind: RuntimeEffectKind::Send,
            completed_at: "2026-08-27T00:00:00.000Z".into(),
            disposition: EffectAuditDisposition::Confirmed,
            error: None,
        }]),
    };
    let encoded = serde_json::to_value(&session).expect("history session json");
    assert_eq!(
        encoded["events"],
        serde_json::json!([{
            "sequence": 1,
            "receivedAt": "2026-08-27T00:00:00.000Z",
            "event": {"kind": "status.changed", "status": "idle"}
        }])
    );
    assert_eq!(
        encoded["effects"],
        serde_json::json!([{
            "sequence": 1,
            "effectId": "effect-one",
            "kind": "send",
            "completedAt": "2026-08-27T00:00:00.000Z",
            "disposition": "confirmed"
        }])
    );
    assert_eq!(
        serde_json::from_value::<HistorySession>(encoded).expect("round trip"),
        session
    );
}

#[test]
fn register_session_rejects_an_empty_key_without_mutating() {
    let mut store = EngineHistoryStore::open("/unused", HistoryLimits::default())
        .expect("process-local projection");
    let error = store
        .register_session(
            String::new(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect_err("empty keys are rejected");
    assert_eq!(
        error.message,
        "engine history contains an empty or duplicate session key"
    );
    let snapshot = store.snapshot();
    assert_eq!(snapshot.revision, 0);
    assert!(snapshot.sessions.is_empty());
}

#[test]
fn register_session_bumps_attachment_count_before_merging_summary() {
    let mut store = EngineHistoryStore::open("/unused", HistoryLimits::default())
        .expect("process-local projection");
    store
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("first attachment");
    let mut incoming = summary("one");
    incoming.model = Some("updated".into());
    incoming.title = String::new();
    store
        .register_session(
            "codex#one".into(),
            incoming,
            "2026-08-27T00:00:01.000Z".into(),
        )
        .expect("reattach");
    let session = store.session("codex#one").expect("registered");
    assert_eq!(session.attachment_count, 2);
    assert_eq!(session.summary.model.as_deref(), Some("updated"));
    assert_eq!(session.summary.title, "one");
    assert_eq!(store.snapshot().revision, 2);
}

#[test]
fn append_event_mutates_the_live_snapshot_in_place() {
    let mut store = EngineHistoryStore::open("/unused", HistoryLimits::default())
        .expect("process-local projection");
    store
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("register");
    let stored = store
        .append_event(
            "codex#one",
            EngineEvent {
                at: None,
                event: EngineEventKind::MessageDelta {
                    text: "delta".into(),
                    agent_key: None,
                },
            },
            "2026-08-27T00:00:01.000Z".into(),
        )
        .expect("append");
    assert_eq!(stored.sequence, 1);
    let session = store.session("codex#one").expect("registered");
    assert_eq!(session.events.len(), 1);
    assert_eq!(session.next_event_sequence, 2);
    assert_eq!(store.snapshot().revision, 2);
}

#[test]
fn session_tail_check_accepts_every_mutation_and_rejects_a_broken_tail() {
    let mut store = EngineHistoryStore::open("/unused", HistoryLimits::default())
        .expect("process-local projection");
    for at in ["2026-08-27T00:00:00.000Z", "2026-08-27T00:00:01.000Z"] {
        store
            .register_session("codex#one".into(), summary("one"), at.into())
            .expect("register and reattach");
        assert!(validate_session_tail(&store.snapshot, "codex#one").is_ok());
    }
    for index in 0..2 {
        store
            .append_event(
                "codex#one",
                EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: format!("delta {index}"),
                        agent_key: None,
                    },
                },
                "2026-08-27T00:00:02.000Z".into(),
            )
            .expect("append");
        assert!(validate_session_tail(&store.snapshot, "codex#one").is_ok());
    }
    assert_eq!(
        validate_session_tail(&store.snapshot, "codex#missing")
            .expect_err("unknown session")
            .message,
        "engine history lost the mutated session"
    );
    let session = store
        .snapshot
        .sessions
        .iter_mut()
        .find(|session| session.key == "codex#one")
        .expect("registered");
    session.events.back_mut().expect("appended").sequence = 1;
    assert_eq!(
        validate_session_tail(&store.snapshot, "codex#one")
            .expect_err("repeated sequence")
            .message,
        "engine history event tail is not strictly increasing"
    );
}

#[test]
fn state_and_tail_reads_match_the_whole_session_read() {
    let mut store = EngineHistoryStore::open("/unused", HistoryLimits::default())
        .expect("process-local projection");
    assert!(store.session_state("codex#one").is_none());
    assert!(store.session_events_after("codex#one", 0).is_none());
    store
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("register");
    for index in 0..5 {
        store
            .append_event(
                "codex#one",
                EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: format!("delta {index}"),
                        agent_key: None,
                    },
                },
                "2026-08-27T00:00:01.000Z".into(),
            )
            .expect("append");
    }
    let whole = store.session("codex#one").expect("registered");
    let state = store.session_state("codex#one").expect("state");
    assert_eq!(state, HistorySessionState::from(&whole));
    assert_eq!(state.attachment_count, 1);
    assert_eq!(state.next_event_sequence, 6);
    for after in [0, 2, 5, 9] {
        let (tail_state, events) = store
            .session_events_after("codex#one", after)
            .expect("tail");
        assert_eq!(tail_state, state);
        assert_eq!(
            events,
            whole
                .events
                .iter()
                .filter(|event| event.sequence > after)
                .cloned()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn ring_retention_preserves_public_vectors_sequences_and_json_shape() {
    let mut store = EngineHistoryStore::open(
        "/unused",
        HistoryLimits {
            events_per_session: 2,
            effects_per_session: 2,
            ..HistoryLimits::default()
        },
    )
    .expect("process-local projection");
    store
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("register");

    for sequence in 1..=3 {
        store
            .append_event(
                "codex#one",
                EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: format!("delta-{sequence}"),
                        agent_key: None,
                    },
                },
                format!("2026-08-27T00:00:0{sequence}.000Z"),
            )
            .expect("append event");
        store
            .append_effect(
                "codex#one",
                format!("effect-{sequence}"),
                RuntimeEffectKind::Send,
                format!("2026-08-27T00:00:0{sequence}.000Z"),
                &DeliveryOutcome::Confirmed(()),
            )
            .expect("append effect");
    }

    let session = store.session("codex#one").expect("registered");
    assert_eq!(
        session
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(
        session
            .effects
            .iter()
            .map(|effect| effect.sequence)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(session.omitted_events_through, Some(1));
    assert_eq!(session.omitted_effects_through, Some(1));
    let wire = serde_json::to_value(&session).expect("history session serializes");
    assert!(wire["events"].is_array());
    assert!(wire["effects"].is_array());
    assert_eq!(wire["events"][0]["sequence"], 2);
    assert_eq!(wire["effects"][0]["sequence"], 2);
    let decoded: HistorySession =
        serde_json::from_value(wire).expect("public history JSON round-trips");
    assert_eq!(decoded, session);
}

#[test]
fn ring_retention_repeatedly_wraps_at_a_single_item_cap() {
    let mut store = EngineHistoryStore::open(
        "/unused",
        HistoryLimits {
            events_per_session: 1,
            effects_per_session: 1,
            ..HistoryLimits::default()
        },
    )
    .expect("process-local projection");
    store
        .register_session(
            "codex#one".into(),
            summary("one"),
            "2026-08-27T00:00:00.000Z".into(),
        )
        .expect("register");

    for sequence in 1..=5 {
        store
            .append_event(
                "codex#one",
                EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: format!("delta-{sequence}"),
                        agent_key: None,
                    },
                },
                format!("2026-08-27T00:00:0{sequence}.000Z"),
            )
            .expect("append event");
        store
            .append_effect(
                "codex#one",
                format!("effect-{sequence}"),
                RuntimeEffectKind::Send,
                format!("2026-08-27T00:00:0{sequence}.000Z"),
                &DeliveryOutcome::Confirmed(()),
            )
            .expect("append effect");
    }

    let session = store.session("codex#one").expect("registered");
    assert_eq!(
        session
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [5]
    );
    assert_eq!(
        session
            .effects
            .iter()
            .map(|effect| effect.sequence)
            .collect::<Vec<_>>(),
        [5]
    );
    assert_eq!(session.omitted_events_through, Some(4));
    assert_eq!(session.omitted_effects_through, Some(4));
    assert_eq!(session.next_event_sequence, 6);
    assert_eq!(session.next_effect_sequence, 6);
}
