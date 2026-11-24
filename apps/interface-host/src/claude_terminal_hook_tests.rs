use interface_storage::WriterLock;

use super::*;
use crate::capture_generation::RECEIPT_ROOT_LOCK;

const CAPTURE_TOKEN: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_ID: &str = "22222222-2222-4222-8222-222222222222";

fn prepare_capture_generation(receipts: &Path, capture_token: &str) {
    let generation = receipts.join(capture_token);
    std::fs::create_dir(&generation).expect("receipt generation");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o700))
            .expect("private receipt generation");
    }
}

fn invoke_with_token(capture_token: &str, payload: &[u8]) -> Vec<u8> {
    let receipts = tempfile::tempdir().expect("receipt root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(receipts.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private receipt root");
    }
    if canonical_uuid_v4(capture_token) {
        prepare_capture_generation(receipts.path(), capture_token);
    }
    let mut output = Vec::new();
    run_claude_terminal_session_hook(capture_token, receipts.path(), payload, &mut output)
        .expect("hook helper");
    output
}

fn invoke(payload: &[u8]) -> Vec<u8> {
    invoke_with_token(CAPTURE_TOKEN, payload)
}

#[test]
fn every_session_transition_emits_only_a_correlated_lookup_wake() {
    for source in ["startup", "resume", "clear", "compact", "fork"] {
        let input = format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp/worktree","hook_event_name":"SessionStart","source":"{source}","model":"fixture"}}"#
        );
        let output = invoke(input.as_bytes());
        let decoded: serde_json::Value = serde_json::from_slice(&output).expect("JSON output");
        assert_eq!(
            decoded,
            serde_json::json!({
                "terminalSequence": format!(
                    "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{CAPTURE_TOKEN}:receipt\u{7}"
                )
            }),
            "source {source}"
        );
    }
}

#[test]
fn turn_boundary_hooks_answer_with_an_activity_frame() {
    for (event, state) in [
        ("UserPromptSubmit", "working"),
        ("PreToolUse", "working"),
        ("PermissionRequest", "waiting"),
        ("Stop", "idle"),
    ] {
        let payload = format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/probe/{SESSION_ID}.jsonl","cwd":"/tmp/probe","hook_event_name":"{event}","permission_mode":"default"}}"#
        );
        let output = invoke(payload.as_bytes());
        let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
        assert_eq!(
            response["terminalSequence"],
            format!("\u{1b}]2;interface-claude-activity:{state}\u{7}")
        );
    }
}

#[test]
fn tool_use_reports_work_without_prompt_context() {
    let payload = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash"}}"#
    );
    let output = invoke(payload.as_bytes());
    let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
    assert_eq!(
        response,
        serde_json::json!({"terminalSequence": "\u{1b}]2;interface-claude-activity:working\u{7}"})
    );
}

#[test]
fn a_permission_dialog_marks_the_pane_waiting_without_deciding_it() {
    // Payload shape recorded from Claude Code 2.1.291 for a Bash prompt and
    // an AskUserQuestion prompt. The answer carries only the activity frame:
    // no decision, so Claude still shows its own dialog.
    for tool in ["Bash", "AskUserQuestion"] {
        let payload = format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"PermissionRequest","permission_mode":"default","tool_name":"{tool}","tool_input":{{}},"permission_suggestions":[]}}"#
        );
        let output = invoke(payload.as_bytes());
        let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
        assert_eq!(
            response,
            serde_json::json!({"terminalSequence": "\u{1b}]2;interface-claude-activity:waiting\u{7}"}),
            "tool {tool}"
        );
    }
}

#[test]
fn stop_keeps_the_pane_working_while_background_agents_or_workflows_run() {
    // Shapes recorded from Claude Code 2.1.280 `Stop` payloads.
    let shell = r#"{"id":"b2zscduzu","type":"shell","status":"running","description":"Background sleep","command":"sleep 240"}"#;
    let agent = r#"{"id":"a16f3e20ca83f59b8","type":"subagent","status":"running","description":"Audit","agent_type":"general-purpose"}"#;
    let workflow = r#"{"id":"w72s9dvms","type":"workflow","status":"running","description":"Audit","name":"interface-rust-audit"}"#;
    let finished = r#"{"id":"a2","type":"subagent","status":"completed","description":"Done"}"#;
    for (tasks, state) in [
        (format!("[{agent}]"), "working"),
        (format!("[{shell},{workflow}]"), "working"),
        (format!("[{shell}]"), "idle"),
        (format!("[{finished}]"), "idle"),
        ("[]".to_owned(), "idle"),
        (r#"{"unexpected":true}"#.to_owned(), "idle"),
    ] {
        let payload = format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"Stop","stop_hook_active":false,"background_tasks":{tasks}}}"#
        );
        let output = invoke(payload.as_bytes());
        let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
        assert_eq!(
            response["terminalSequence"],
            format!("\u{1b}]2;interface-claude-activity:{state}\u{7}"),
            "background tasks {tasks}"
        );
    }
}

#[test]
fn receipt_preserves_the_exact_transcript_path_before_waking() {
    const DURABLE_ID: &str = "33333333-3333-4333-8333-333333333333";

    let receipts = tempfile::tempdir().expect("receipt root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(receipts.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private receipt root");
    }
    prepare_capture_generation(receipts.path(), CAPTURE_TOKEN);
    let transcript_path = format!("/tmp/worktree/{DURABLE_ID}.jsonl");
    let input = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"{transcript_path}","cwd":"/tmp/worktree","hook_event_name":"SessionStart","source":"resume"}}"#
    );
    let mut output = Vec::new();
    run_claude_terminal_session_hook(
        CAPTURE_TOKEN,
        receipts.path(),
        input.as_bytes(),
        &mut output,
    )
    .expect("hook helper");

    let generation = receipts.path().join(CAPTURE_TOKEN);
    let entries = std::fs::read_dir(&generation)
        .expect("receipt generation")
        .collect::<Result<Vec<_>, _>>()
        .expect("receipt entries");
    let receipts = entries
        .iter()
        .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let bytes = std::fs::read(receipts[0].path()).expect("receipt bytes");
    let receipt: ClaudeTerminalSessionReceipt =
        serde_json::from_slice(&bytes).expect("receipt JSON");
    assert_eq!(receipt.reported_session_id, SESSION_ID);
    assert_eq!(receipt.transcript_path, PathBuf::from(transcript_path));
    assert_eq!(receipt.source, "resume");
    assert!(receipt.captured_at_unix_nanos > 0);
    let decoded: serde_json::Value = serde_json::from_slice(&output).expect("hook output");
    assert_eq!(
        decoded["terminalSequence"],
        format!("\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{CAPTURE_TOKEN}:receipt\u{7}")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(receipts[0].path())
                .expect("receipt metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn writer_compacts_a_full_generation_without_losing_transition_authority() {
    const DURABLE_ID: &str = "33333333-3333-4333-8333-333333333333";

    let receipts = tempfile::tempdir().expect("receipt root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(receipts.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private receipt root");
    }
    let generation = receipts.path().join(CAPTURE_TOKEN);
    std::fs::create_dir(&generation).expect("receipt generation");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o700))
            .expect("private receipt generation");
    }
    let future_timestamp: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos()
        .try_into()
        .expect("timestamp fits u64");
    let future_timestamp = future_timestamp + 10_000_000_000;
    for index in 0..MAX_RECEIPTS_PER_GENERATION {
        let receipt = ClaudeTerminalSessionReceipt {
            version: RECEIPT_VERSION,
            capture_token: CAPTURE_TOKEN.to_owned(),
            parent_process_id: parent_process_id().expect("test parent process"),
            reported_session_id: SESSION_ID.to_owned(),
            transcript_path: PathBuf::from(format!("/tmp/{DURABLE_ID}.jsonl")),
            cwd: PathBuf::from("/tmp/worktree"),
            source: if index == 17 { "resume" } else { "startup" }.to_owned(),
            captured_at_unix_nanos: future_timestamp
                + u64::try_from(index).expect("fixture index fits u64"),
        };
        let path = generation.join(format!("{}.json", Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&receipt).expect("receipt JSON"))
            .expect("receipt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .expect("private receipt mode");
        }
    }

    let input = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{DURABLE_ID}.jsonl","cwd":"/tmp/worktree","hook_event_name":"SessionStart","source":"startup"}}"#
    );
    let mut output = Vec::new();
    run_claude_terminal_session_hook(
        CAPTURE_TOKEN,
        receipts.path(),
        input.as_bytes(),
        &mut output,
    )
    .expect("hook helper");

    let entries = std::fs::read_dir(&generation)
        .expect("receipt generation")
        .collect::<Result<Vec<_>, _>>()
        .expect("receipt entries");
    let installed = entries
        .iter()
        .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
        .map(|entry| {
            let bytes = std::fs::read(entry.path()).expect("receipt bytes");
            serde_json::from_slice::<ClaudeTerminalSessionReceipt>(&bytes).expect("receipt JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        installed.len(),
        1,
        "one effective receipt remains for the parent lineage"
    );
    assert!(installed.iter().any(|receipt| receipt.source == "resume"));
    assert_eq!(
        installed
            .iter()
            .map(|receipt| receipt.captured_at_unix_nanos)
            .max(),
        Some(future_timestamp + 17),
        "a later provisional startup cannot displace the retained resume authority"
    );
    assert!(
        output.is_empty(),
        "a provisional startup cannot downgrade an existing transition"
    );
}

#[test]
fn restored_generations_publish_concurrently_without_cross_token_lock_contention() {
    const RESTORED_TERMINALS: usize = 32;

    let receipts = tempfile::tempdir().expect("receipt root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(receipts.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private receipt root");
    }
    let private_root = PrivateDirectory::open(receipts.path()).expect("private receipt root");
    WriterLock::acquire(&private_root, RECEIPT_ROOT_LOCK)
        .expect("initialize root lock")
        .release(&private_root)
        .expect("release initialized root lock");
    let root = receipts.path().to_path_buf();
    let identities = (0..RESTORED_TERMINALS)
        .map(|_| {
            let token = Uuid::new_v4().hyphenated().to_string();
            let session = Uuid::new_v4().hyphenated().to_string();
            prepare_capture_generation(&root, &token);
            (token, session)
        })
        .collect::<Vec<_>>();
    let writers = identities
        .into_iter()
        .map(|(token, session)| {
            let root = root.clone();
            std::thread::spawn(move || {
                let input = format!(
                    r#"{{"session_id":"{session}","transcript_path":"/tmp/{session}.jsonl","cwd":"/tmp/worktree","hook_event_name":"SessionStart","source":"startup"}}"#
                );
                let mut output = Vec::new();
                run_claude_terminal_session_hook(
                    &token,
                    &root,
                    input.as_bytes(),
                    &mut output,
                )
                .expect("concurrent hook helper");
                assert!(!output.is_empty());
                token
            })
        })
        .collect::<Vec<_>>();
    let tokens = writers
        .into_iter()
        .map(|writer| writer.join().expect("concurrent hook thread"))
        .collect::<Vec<_>>();

    assert_eq!(tokens.len(), RESTORED_TERMINALS);
    for token in tokens {
        let receipt_count = std::fs::read_dir(root.join(token))
            .expect("receipt generation")
            .map(|entry| entry.expect("receipt entry").file_name())
            .filter(|name| receipt_file_id(&name.to_string_lossy()).is_some())
            .count();
        assert_eq!(receipt_count, 1);
    }
}

#[test]
fn malformed_or_unsupported_input_is_silent() {
    let fixtures = [
        Vec::from(&b"not JSON"[..]),
        format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/x","cwd":"/tmp","hook_event_name":"SessionEnd","source":"startup"}}"#
        )
        .into_bytes(),
        format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/x","cwd":"/tmp","hook_event_name":"SessionStart","source":"unknown"}}"#
        )
        .into_bytes(),
        br#"{"session_id":"not-a-uuid","transcript_path":"/tmp/x","cwd":"/tmp","hook_event_name":"SessionStart","source":"startup"}"#.to_vec(),
        format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"relative","cwd":"/tmp","hook_event_name":"SessionStart","source":"startup"}}"#
        )
        .into_bytes(),
    ];
    for fixture in fixtures {
        assert!(invoke(&fixture).is_empty());
    }
}

#[test]
fn invalid_capture_token_is_silent() {
    let input = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/x","cwd":"/tmp","hook_event_name":"SessionStart","source":"startup"}}"#
    );
    for token in [
        "not-a-token",
        "11111111-1111-3111-8111-111111111111",
        "11111111-1111-4111-7111-111111111111",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
    ] {
        assert!(
            invoke_with_token(token, input.as_bytes()).is_empty(),
            "token {token}"
        );
    }
}

#[test]
fn oversized_input_is_silent() {
    let maximum = usize::try_from(MAX_HOOK_INPUT_BYTES).expect("hook limit fits usize");
    assert!(invoke(&vec![b'x'; maximum + 1]).is_empty());
}

#[test]
fn long_turn_boundary_payloads_still_report_activity() {
    // Claude Code sends the whole prompt, tool input and final answer with
    // these events; a long answer must still end the turn.
    let long = "a".repeat(200 * 1_024);
    for (event, field, state) in [
        ("UserPromptSubmit", "prompt", "working"),
        ("PreToolUse", "tool_input", "working"),
        ("Stop", "last_assistant_message", "idle"),
    ] {
        let payload = format!(
            r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"{event}","{field}":"{long}"}}"#
        );
        let output = invoke(payload.as_bytes());
        let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
        assert_eq!(
            response["terminalSequence"],
            format!("\u{1b}]2;interface-claude-activity:{state}\u{7}"),
            "event {event}"
        );
    }
    // A receipt never comes from an oversized payload.
    let session_start = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"SessionStart","source":"startup","model":"{long}"}}"#
    );
    assert!(invoke(session_start.as_bytes()).is_empty());
}

#[test]
fn a_multi_megabyte_stop_payload_still_ends_the_turn() {
    // A final answer quoting a large file or a screenshot runs to megabytes.
    let long = "b".repeat(4 * 1_024 * 1_024);
    let payload = format!(
        r#"{{"session_id":"{SESSION_ID}","transcript_path":"/tmp/{SESSION_ID}.jsonl","cwd":"/tmp","hook_event_name":"Stop","last_assistant_message":"{long}"}}"#
    );
    let output = invoke(payload.as_bytes());
    let response: serde_json::Value = serde_json::from_slice(&output).expect("hook JSON");
    assert_eq!(
        response["terminalSequence"],
        "\u{1b}]2;interface-claude-activity:idle\u{7}"
    );
}

#[test]
fn the_helpers_own_identity_needs_no_liveness_probe_and_matches_ps() {
    assert_eq!(
        parent_process_id().expect("parent from getppid"),
        parent_process_id_of(std::process::id()).expect("parent from ps")
    );
    let own = current_hook_process_identity().expect("own identity");
    let probed = hook_process_identity(std::process::id())
        .expect("probed identity")
        .expect("this process is alive");
    assert_eq!(own, probed);
}
