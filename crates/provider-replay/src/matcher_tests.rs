//! The matcher and the replay engine over small hand-made transcripts.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    matcher::{Bindings, bind, difference, signature, substitute},
    replay::{Replay, Start, start},
    transcript::{Entry, Probe, Transcript, flag_shape},
};

fn launch(argv: &[&str], cwd: &str) -> Entry {
    Entry::Launch {
        argv: argv.iter().map(|argument| (*argument).to_owned()).collect(),
        cwd: cwd.to_owned(),
    }
}

fn out(frame: Value) -> Entry {
    Entry::Out { frame }
}

fn inbound(frame: Value) -> Entry {
    Entry::In { frame }
}

fn transcript(entries: Vec<Entry>) -> Transcript {
    Transcript {
        version: 1,
        engine: "codex".into(),
        protocol: "app-server".into(),
        cli_version: "codex-cli 0.0.0".into(),
        scenario: "test".into(),
        recorded_at: "2026-11-14".into(),
        probes: BTreeMap::from([(
            "--version".to_owned(),
            Probe {
                stdout: "codex-cli 0.0.0\n".into(),
                code: 0,
            },
        )]),
        entries,
    }
}

fn session(transcript: &Transcript, argv: &[&str], cwd: &str) -> (Replay, Vec<String>) {
    let argv: Vec<String> = argv.iter().map(|argument| (*argument).to_owned()).collect();
    match start(transcript, &argv, cwd).expect("a launch") {
        Start::Session { replay, lines } => (replay, lines),
        Start::Probe { .. } => panic!("answered as a probe"),
    }
}

fn frames(lines: &[String]) -> Vec<Value> {
    lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("JSON line"))
        .collect()
}

#[test]
fn signatures_name_rpc_and_stream_json_frames() {
    assert_eq!(
        signature(&json!({"id": 1, "method": "initialize", "params": {}})),
        "request initialize"
    );
    assert_eq!(
        signature(&json!({"method": "turn/started", "params": {}})),
        "notification turn/started"
    );
    assert_eq!(
        signature(&json!({"id": 1, "result": {}})),
        "response (result)"
    );
    assert_eq!(
        signature(
            &json!({"type": "control_request", "request_id": "x", "request": {"subtype": "interrupt"}})
        ),
        "control_request interrupt"
    );
    assert_eq!(
        signature(&json!({"type": "result", "subtype": "success"})),
        "result success"
    );
    assert_eq!(signature(&json!({"type": "user", "message": {}})), "user");
}

#[test]
fn shapes_must_agree_but_leaf_values_need_not() {
    let recorded = json!({"id": 1, "method": "turn/start", "params": {"threadId": "a", "input": [{"type": "text", "text": "hi"}]}});
    let same_shape = json!({"id": 7, "method": "turn/start", "params": {"threadId": "b", "input": [{"type": "text", "text": "other"}]}});
    assert_eq!(difference(&recorded, &same_shape), None);

    let extra_field = json!({"id": 7, "method": "turn/start", "params": {"threadId": "b", "model": "x", "input": [{"type": "text", "text": "hi"}]}});
    assert_eq!(
        difference(&recorded, &extra_field).as_deref(),
        Some("at $.params: unexpected [\"model\"]")
    );
    let other_method = json!({"id": 7, "method": "turn/steer", "params": {}});
    assert_eq!(
        difference(&recorded, &other_method).as_deref(),
        Some("expected a request turn/start, received a request turn/steer")
    );
    let wrong_type = json!({"id": 7, "method": "turn/start", "params": {"threadId": 3, "input": [{"type": "text", "text": "hi"}]}});
    assert_eq!(
        difference(&recorded, &wrong_type).as_deref(),
        Some("at $.params.threadId: expected a string, received a number")
    );
}

#[test]
fn host_ids_are_bound_and_substituted_into_later_answers() {
    let mut bindings = Bindings::default();
    let recorded =
        json!({"id": 3, "method": "thread/start", "params": {"cwd": "/tmp/recorded/work"}});
    let arrived =
        json!({"id": 9, "method": "thread/start", "params": {"cwd": "/private/tmp/today/work"}});
    bind(&recorded, &arrived, &mut bindings);

    // The response to the host's request carries the host's id; a request
    // the provider makes keeps its own id; the path is replaced inside text.
    assert_eq!(
        substitute(
            &json!({"id": 3, "result": {"cwd": "/tmp/recorded/work/a.txt"}}),
            &bindings
        ),
        json!({"id": 9, "result": {"cwd": "/private/tmp/today/work/a.txt"}})
    );
    assert_eq!(
        substitute(
            &json!({"id": 3, "method": "item/commandExecution/requestApproval", "params": {}}),
            &bindings
        ),
        json!({"id": 3, "method": "item/commandExecution/requestApproval", "params": {}})
    );

    // Claude's control request ids are strings and bind the same way.
    let mut bindings = Bindings::default();
    bind(
        &json!({"type": "control_request", "request_id": "00000000-0000-4000-8000-000000000003", "request": {"subtype": "initialize"}}),
        &json!({"type": "control_request", "request_id": "6f1c0e8e-2f7a-4f8b-9c55-0e1d2c3b4a59", "request": {"subtype": "initialize"}}),
        &mut bindings,
    );
    assert_eq!(
        substitute(
            &json!({"type": "control_response", "response": {"subtype": "success", "request_id": "00000000-0000-4000-8000-000000000003"}}),
            &bindings
        ),
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": "6f1c0e8e-2f7a-4f8b-9c55-0e1d2c3b4a59"}})
    );
}

#[test]
fn a_launch_serves_answers_only_once_their_question_arrives() {
    let recorded = transcript(vec![
        launch(&["app-server", "-c", "features.plugins=false"], "/tmp/rec"),
        out(json!({"id": 0, "method": "initialize", "params": {}})),
        inbound(json!({"id": 0, "result": {"userAgent": "x"}})),
        out(json!({"method": "initialized"})),
        out(json!({"id": 1, "method": "thread/start", "params": {"cwd": "/tmp/rec"}})),
        inbound(json!({"id": 1, "result": {"thread": {"id": "t1", "cwd": "/tmp/rec"}}})),
        inbound(json!({"method": "thread/started", "params": {"thread": {"id": "t1"}}})),
    ]);
    let (mut replay, lines) = session(
        &recorded,
        &["app-server", "-c", "features.plugins=false"],
        "/work/now",
    );
    assert!(lines.is_empty(), "nothing before the host's first frame");
    let answer = replay
        .accept(&json!({"id": 0, "method": "initialize", "params": {}}))
        .unwrap();
    assert_eq!(
        frames(&answer),
        [json!({"id": 0, "result": {"userAgent": "x"}})]
    );
    assert!(
        replay
            .accept(&json!({"method": "initialized"}))
            .unwrap()
            .is_empty()
    );
    let answer = replay
        .accept(&json!({"id": 5, "method": "thread/start", "params": {"cwd": "/work/now"}}))
        .unwrap();
    assert_eq!(
        frames(&answer),
        [
            json!({"id": 5, "result": {"thread": {"id": "t1", "cwd": "/work/now"}}}),
            json!({"method": "thread/started", "params": {"thread": {"id": "t1"}}}),
        ]
    );
    assert_eq!(
        replay.finish(),
        0,
        "a killed recording ends at stdin's close"
    );
}

#[test]
fn an_unexpected_frame_fails_naming_the_expected_and_the_received_frame() {
    let recorded = transcript(vec![
        launch(&["--output-format", "stream-json"], "/tmp/rec"),
        out(
            json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}}),
        ),
        inbound(
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1"}}),
        ),
        out(json!({"type": "user", "message": {"role": "user", "content": "hi"}})),
        Entry::Exit { code: 7 },
    ]);
    let (mut replay, _) = session(&recorded, &["--output-format", "stream-json"], "/w");
    let error = replay
        .accept(&json!({"type": "control_request", "request_id": "m", "request": {"subtype": "set_model", "model": "x"}}))
        .unwrap_err();
    assert!(
        error.starts_with("launch 1, step 1: unexpected frame from Interface"),
        "{error}"
    );
    assert!(
        error.contains("expected: control_request initialize: {")
            && error.contains("received: control_request set_model: {")
            && error.contains(
                "difference: expected a control_request initialize, received a control_request set_model"
            ),
        "{error}"
    );

    // After the recorded exit, any frame is unexpected. A close before a
    // frame the recording still holds leaves with the recorded exit code.
    let (mut replay, _) = session(&recorded, &["--output-format", "stream-json"], "/w");
    replay
        .accept(&json!({"type": "control_request", "request_id": "x", "request": {"subtype": "initialize"}}))
        .unwrap();
    assert_eq!(replay.finish(), 7);
    replay
        .accept(&json!({"type": "user", "message": {"role": "user", "content": "hi"}}))
        .unwrap();
    assert_eq!(replay.finish(), 7);
    let late = replay
        .accept(&json!({"type": "user", "message": {"role": "user", "content": "again"}}))
        .unwrap_err();
    assert!(
        late.contains("after the recorded process exited with code 7"),
        "{late}"
    );
}

#[test]
fn a_frame_sent_a_little_early_is_matched_ahead() {
    let recorded = transcript(vec![
        launch(&["--x"], "/r"),
        out(json!({"type": "user", "message": "a"})),
        inbound(json!({"type": "assistant", "message": "one"})),
        out(
            json!({"type": "control_request", "request_id": "q", "request": {"subtype": "get_context_usage"}}),
        ),
        inbound(
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "q"}}),
        ),
        out(
            json!({"type": "control_request", "request_id": "i", "request": {"subtype": "interrupt"}}),
        ),
        inbound(
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "i"}}),
        ),
    ]);
    let (mut replay, _) = session(&recorded, &["--x"], "/r");
    replay
        .accept(&json!({"type": "user", "message": "a"}))
        .unwrap();
    // The interrupt arrives before the usage request it followed when recorded.
    let held = replay
        .accept(&json!({"type": "control_request", "request_id": "I2", "request": {"subtype": "interrupt"}}))
        .unwrap();
    assert!(held.is_empty(), "its answer waits for its place");
    let answer = replay
        .accept(&json!({"type": "control_request", "request_id": "Q2", "request": {"subtype": "get_context_usage"}}))
        .unwrap();
    assert_eq!(
        frames(&answer),
        [
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "Q2"}}),
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "I2"}}),
        ]
    );
}

#[test]
fn probes_answer_by_argv_and_launches_are_chosen_by_flags_then_frames() {
    let recorded = transcript(vec![
        launch(&["--session-id=00000000-0000-4000-8000-000000000001"], "/r"),
        out(json!({"type": "user", "message": "first"})),
        inbound(json!({"type": "result", "session_id": "00000000-0000-4000-8000-000000000001"})),
        Entry::Exit { code: 0 },
        launch(&["--resume=00000000-0000-4000-8000-000000000001"], "/r"),
        out(json!({"type": "user", "message": "second"})),
        inbound(json!({"type": "result", "session_id": "00000000-0000-4000-8000-000000000001"})),
        Entry::Exit { code: 0 },
        launch(&["app-server"], "/r"),
        out(json!({"id": 1, "method": "thread/start"})),
        inbound(json!({"id": 1, "result": "started"})),
        launch(&["app-server"], "/r"),
        out(json!({"id": 1, "method": "thread/resume"})),
        inbound(json!({"id": 1, "result": "resumed"})),
    ]);
    match start(&recorded, &["--version".to_owned()], "/w").unwrap() {
        Start::Probe { stdout, code } => {
            assert_eq!((stdout.as_str(), code), ("codex-cli 0.0.0\n", 0));
        }
        Start::Session { .. } => panic!("--version is a probe"),
    }
    let unknown = start(&recorded, &["auth".to_owned(), "--json".to_owned()], "/w").unwrap_err();
    assert!(
        unknown.contains("no recorded launch or probe matches argv"),
        "{unknown}"
    );

    // The resume is told apart by its flag, and its id bound from argv.
    let fresh = "4b9d8a52-0d0e-4c1f-9e6a-8d7c6b5a4f3e";
    let (mut resumed, _) = session(&recorded, &[&format!("--resume={fresh}")], "/w");
    let answer = resumed
        .accept(&json!({"type": "user", "message": "second"}))
        .unwrap();
    assert_eq!(
        frames(&answer),
        [json!({"type": "result", "session_id": fresh})]
    );

    // Two launches with one argv are told apart by the first frame that differs.
    let (mut codex, _) = session(&recorded, &["app-server"], "/w");
    let answer = codex
        .accept(&json!({"id": 1, "method": "thread/resume"}))
        .unwrap();
    assert_eq!(frames(&answer), [json!({"id": 1, "result": "resumed"})]);
    assert_eq!(
        flag_shape(&[
            "app-server".into(),
            "-c".into(),
            "a=b".into(),
            "--x=1".into()
        ]),
        ["--x", "-c"]
    );
}

#[test]
fn transcripts_round_trip_through_their_file_text() {
    let recorded = transcript(vec![
        launch(&["--x"], "/r"),
        out(json!({"type": "user"})),
        inbound(Value::String("not json".into())),
        Entry::Exit { code: 0 },
    ]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("t.json");
    std::fs::write(&path, recorded.to_file_text()).unwrap();
    assert_eq!(Transcript::read(&path).unwrap(), recorded);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"dir\": \"in\"") && text.contains("\"recordedAt\""));
}
