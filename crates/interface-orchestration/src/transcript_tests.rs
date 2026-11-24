use interface_contracts::engine::{
    EngineEvent, EngineEventKind, EngineId, NoticeTone, SessionStatus,
};
use serde_json::Value;

use super::*;

#[test]
#[allow(clippy::too_many_lines)]
fn orchestration_transcript_lines_match_the_frozen_human_renderer() {
    let cases = [
        (
            EngineEventKind::UserMessage {
                text: "hello\0 world".into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
            Some("user: hello world"),
        ),
        (
            EngineEventKind::MessageDelta {
                text: "answer".into(),
                agent_key: None,
            },
            Some("agent: answer"),
        ),
        (
            EngineEventKind::PeerResult {
                operation_id: "operation-one".into(),
                source_engine: EngineId::new("codex").expect("engine"),
                source_session_id: "source-one".into(),
                objective: "review".into(),
                text: "peer result".into(),
                untrusted: true,
            },
            Some("untrusted peer (codex): peer result"),
        ),
        (
            EngineEventKind::ToolCall {
                call_id: "call-one".into(),
                tool: "shell".into(),
                summary: "run tests".into(),
                input: Value::Null,
                agent_key: None,
            },
            Some("tool shell: run tests"),
        ),
        (
            EngineEventKind::ToolResult {
                call_id: "call-one".into(),
                ok: false,
                summary: "failed".into(),
                agent_key: None,
            },
            Some("tool result (failed): failed"),
        ),
        (
            EngineEventKind::ApprovalRequested {
                approval_id: "approval-one".into(),
                tool: "write".into(),
                summary: "edit file".into(),
                input: Value::Null,
            },
            Some("approval requested for write: edit file"),
        ),
        (
            EngineEventKind::DiffProduced {
                call_id: "call-two".into(),
                path: "src/main.rs".into(),
                before: String::new(),
                after: String::new(),
                agent_key: None,
            },
            Some("diff: src/main.rs"),
        ),
        (
            EngineEventKind::StatusChanged {
                status: SessionStatus::Waiting,
            },
            Some("status: waiting"),
        ),
        (
            EngineEventKind::Notice {
                message: "heads up".into(),
                tone: NoticeTone::Warning,
            },
            Some("warning: heads up"),
        ),
        (
            EngineEventKind::Error {
                cause: None,
                message: "try again".into(),
                retryable: true,
            },
            Some("error (retrying): try again"),
        ),
        (
            EngineEventKind::SubagentStarted {
                agent_key: "worker-one".into(),
                subagent_type: "explore".into(),
                description: "audit".into(),
                model: None,
            },
            Some("subagent started: audit"),
        ),
        (
            EngineEventKind::SubagentFinished {
                agent_key: "worker-one".into(),
                ok: true,
                summary: "done".into(),
            },
            Some("subagent finished: done"),
        ),
        (EngineEventKind::SessionClosed, Some("session closed")),
        (
            EngineEventKind::UsageUpdated {
                input_tokens: 1,
                output_tokens: 2,
                rate_limited: None,
            },
            None,
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(
            orchestration_line(&EngineEvent { at: None, event }).as_deref(),
            expected
        );
    }
}

#[test]
fn orchestration_transcript_keeps_only_the_requested_tail() {
    let events = (0..8)
        .map(|index| EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: format!("line-{index}"),
                agent_key: None,
            },
        })
        .collect::<Vec<_>>();
    assert_eq!(
        render_orchestration_transcript(&events, 3),
        ["agent: line-5", "agent: line-6", "agent: line-7"]
    );
    assert_eq!(
        render_orchestration_transcript(&events, 0),
        Vec::<String>::new()
    );
}

#[test]
fn orchestration_text_cleaning_matches_node_byte_truncation() {
    assert_eq!(clean_orchestration_text("a\u{1b}\n\tb"), "a\n\tb");

    let oversized = format!("{}{}", "a".repeat(4_079), "😀".repeat(5));
    let cleaned = clean_orchestration_text(&oversized);
    assert!(cleaned.starts_with(&"a".repeat(4_079)));
    assert!(cleaned.ends_with("�…"));
}
