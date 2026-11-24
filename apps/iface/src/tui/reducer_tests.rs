use serde_json::json;

use super::super::agent_text_rows;
use super::super::tests::session;
use super::*;
use crate::agent_protocol::AgentStatus;
use crate::diagnostics::ContextTelemetryPhase;
use interface_contracts::engine::{ContextUsage, PlanEntryPriority, PlanEntryStatus};

#[test]
fn plan_updates_are_sanitized_at_push() {
    let mut row = session("claude#plan", AgentStatus::Idle);
    reduce_event(
        &mut row,
        &EngineEvent {
            at: None,
            event: EngineEventKind::PlanUpdated {
                entries: vec![PlanEntry {
                    content: "do \u{1b}[31msecret".into(),
                    priority: PlanEntryPriority::Medium,
                    status: PlanEntryStatus::Pending,
                }],
            },
        },
    );
    match row.transcript.last() {
        Some(TranscriptCell::Plan { entries }) => {
            assert_eq!(entries[0].content, "do secret");
        }
        other => panic!("expected plan cell, got {other:?}"),
    }
}

#[test]
fn context_telemetry_records_first_complete_snapshot_and_changed_turn_boundary() {
    let mut row = session("claude#context", AgentStatus::Working);
    row.setting_sources = Some(vec!["project".into()]);
    let startup = EngineEvent {
        at: None,
        event: EngineEventKind::ContextUsage {
            usage: ContextUsage {
                used: Some(100),
                size: Some(1_000),
                percent: Some(10.0),
                categories: None,
                auto_compact: None,
                auto_compact_threshold: None,
                compacting: None,
            },
        },
    };
    let before = row.state.status;
    reduce_event(&mut row, &startup);
    let sample = context_telemetry_sample("claude#context", &mut row, &startup, before)
        .expect("startup sample");
    assert_eq!(sample.phase, ContextTelemetryPhase::Startup);
    assert_eq!(sample.scope.as_deref(), Some("project"));

    let changed = EngineEvent {
        at: None,
        event: EngineEventKind::ContextUsage {
            usage: ContextUsage {
                used: Some(250),
                size: Some(1_000),
                percent: Some(25.0),
                categories: None,
                auto_compact: None,
                auto_compact_threshold: None,
                compacting: None,
            },
        },
    };
    let before = row.state.status;
    reduce_event(&mut row, &changed);
    assert!(context_telemetry_sample("claude#context", &mut row, &changed, before).is_none());
    let idle = EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    };
    let before = row.state.status;
    reduce_event(&mut row, &idle);
    let sample =
        context_telemetry_sample("claude#context", &mut row, &idle, before).expect("turn sample");
    assert_eq!(sample.phase, ContextTelemetryPhase::Turn);
    assert_eq!(sample.used, 250);
}

#[test]
fn provider_history_replaces_local_audit_projection_before_resume() {
    let mut row = session("claude#history-one", AgentStatus::NotLive);
    row.transcript.push(TranscriptCell::Notice {
        message: "local audit entry".into(),
        tone: CellTone::Info,
    });
    replace_provider_history(
        &mut row,
        interface_contracts::ipc::HistoryPage {
            availability: Some(HistoryAvailability::Available),
            reason: None,
            events: vec![
                EngineEvent {
                    at: None,
                    event: EngineEventKind::UserMessage {
                        text: "persisted question".into(),
                        agent_key: None,
                        images: None,
                        image_references: None,
                    },
                },
                EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: "persisted answer".into(),
                        agent_key: None,
                    },
                },
            ],
            messages: Some(2),
            older_messages: 3,
        },
    );
    let text = row
        .transcript
        .iter()
        .flat_map(agent_text_rows)
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        [
            "user: persisted question",
            "assistant: persisted answer",
            "notice: 3 older stored messages are not shown."
        ]
    );
    assert!(!text.iter().any(|line| line.contains("local audit")));
    assert_eq!(row.state.status, AgentStatus::NotLive);
    assert!(!row.state.live);
}

#[test]
fn unknown_tool_input_preview_marks_truncation_only_past_the_limit() {
    // Pretty-printing a string adds its two quotes.
    let exact = display_unknown(&json!("a".repeat(4_094)));
    assert_eq!(exact.chars().count(), 4_096);
    assert!(!exact.ends_with('…'));
    let over = display_unknown(&json!("a".repeat(4_095)));
    assert_eq!(over.chars().count(), 4_097);
    assert!(over.ends_with('…'));
}

#[test]
fn capped_tool_input_preview_equals_the_whole_pretty_print_it_replaced() {
    fn whole(value: &serde_json::Value) -> String {
        let source = serde_json::to_string_pretty(value).unwrap_or_else(|_| "[unavailable]".into());
        let mut output = source.chars().take(4_096).collect::<String>();
        if source.char_indices().nth(4_096).is_some() {
            output.push('…');
        }
        for_display(&output)
    }
    let file = "line é 漢字 🦀\n".repeat(40_000);
    let mut inputs = vec![
        json!({"file_path": "/repo/a.rs", "content": file}),
        json!({"old_string": "x".repeat(20_000), "new_string": "🦀".repeat(5_000)}),
        json!(["é".repeat(4_094)]),
        json!("🦀".repeat(4_094)),
        json!("🦀".repeat(4_095)),
        json!({"command": "ls -la", "nested": {"deep": [1, 2, 3]}}),
        json!(null),
    ];
    // Put every byte width of the last kept character on the cap boundary.
    for width in 0..8 {
        inputs.push(json!(format!(
            "{}{}",
            "a".repeat(4_090 + width),
            "🦀".repeat(4_000)
        )));
    }
    for input in &inputs {
        assert_eq!(display_unknown(input), whole(input));
    }
}

/// T3 section 8: the terminal never draws a visual's page; it names it, its
/// title sanitized like any provider text.
#[test]
fn a_visual_reply_is_named_never_drawn() {
    let mut row = session("claude#visual", AgentStatus::Idle);
    reduce_event(
        &mut row,
        &EngineEvent {
            at: None,
            event: EngineEventKind::VisualShown {
                id: "v1731630000500-0a1b2c3d".into(),
                title: "Signups \u{1b}[31mby week".into(),
                bytes: 1_834,
            },
        },
    );
    match row.transcript.last() {
        Some(TranscriptCell::Notice { message, .. }) => assert_eq!(
            message,
            "Showed a visual: Signups by week (open the thread in Interface to see it)"
        ),
        other => panic!("expected a notice, got {other:?}"),
    }
}
