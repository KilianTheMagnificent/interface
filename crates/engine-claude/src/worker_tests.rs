use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
fn report() -> Value {
    json!({"version":1,"outcome":"changes","summary":"Changed a.rs","missingFiles":[],"files":[{"path":"a.rs","operation":"write","content":"new\n"}]})
}
fn init() -> Value {
    json!({"type":"system","subtype":"init","session_id":"fixture","tools":[],"mcp_servers":[],"plugins":[],"skills":[],"model":"claude-fixture"})
}
#[test]
fn refuses_inherited_tools_incomplete_results_and_tool_calls() {
    let mut init = init();
    init["tools"] = json!(["Read"]);
    assert!(ClaudeEvents::default().observe(&init, None).is_err());
    let mut e = ClaudeEvents::default();
    e.observe(&self::init(), None).unwrap();
    assert!(e.observe(&json!({"type":"assistant","session_id":"fixture","message":{"content":[{"type":"tool_use","name":"Read"}]}}), None).is_err());
    assert!(e.finish().is_err());
}
#[test]
fn preserves_reported_usage_and_checks_session_identity() {
    let mut e = ClaudeEvents::default();
    e.observe(&init(), None).unwrap();
    let mut result = json!({"type":"result","subtype":"success","is_error":false,"session_id":"fixture","structured_output":report(),"usage":{"input_tokens":3,"output_tokens":5},"total_cost_usd":0.1});
    result["session_id"] = json!("other");
    assert!(e.observe(&result, None).is_err());
    result["session_id"] = json!("fixture");
    e.observe(&result, None).unwrap();
    let r = e.finish().unwrap();
    assert_eq!(r.files[0].after.as_deref(), Some("new\n"));
    assert_eq!(r.output.actual_model.as_deref(), Some("claude-fixture"));
    assert_eq!(r.output.usage.cost_usd, Some(0.1));
}
#[tokio::test]
async fn cli_is_isolated_without_disabling_subscription_auth() {
    let temp = tempfile::tempdir().unwrap();
    let packet = temp.path().join("packet");
    fs::create_dir(&packet).unwrap();
    fs::write(packet.join("a.rs"), "old\n").unwrap();
    let exe = temp.path().join("worker.py");
    let result = json!({"type":"result","subtype":"success","is_error":false,"session_id":"fixture","structured_output":report()});
    fs::write(
        &exe,
        format!(
            r"#!/usr/bin/python3
import json,os,pathlib,sys
args=sys.argv
assert '--safe-mode' in args and '--restricted' in args and '--bare' not in args
assert json.loads(args[args.index('--settings')+1]) == {{'enabledPlugins': {{'agents-md@builtin': False, 'telemetry@builtin': False}}}}
assert args[args.index('--tools')+1] == ''
assert '--strict-mcp-config' in args
assert json.loads(args[args.index('--mcp-config')+1]) == {{'mcpServers':{{}}}}
assert '--disable-slash-commands' in args
assert '--max-budget-usd' not in args and '--max-turns' not in args
assert not pathlib.Path('a.rs').exists()
assert 'INTERFACE_PANE_MCP_SOCKET' not in os.environ
assert 'task input' in sys.stdin.read()
print({:?},flush=True)
print({:?},flush=True)
",
            init().to_string(),
            result.to_string()
        ),
    )
    .unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let (_tx, rx) = watch::channel(false);
    let r = run_with_effort(
        &exe,
        &packet,
        "claude/fixture",
        "task input",
        rx,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(r.files[0].after.as_deref(), Some("new\n"));
    assert_eq!(fs::read_to_string(packet.join("a.rs")).unwrap(), "old\n");
}

#[test]
fn synthetic_output_requires_its_exact_acknowledgement_and_final_report() {
    let mut e = ClaudeEvents::default();
    let mut initial = init();
    initial["tools"] = json!(["StructuredOutput"]);
    e.observe(&initial, None).unwrap();
    e.observe(
        &json!({"type":"rate_limit_event","session_id":"fixture"}),
        None,
    )
    .unwrap();
    e.observe(
        &json!({"type":"system","subtype":"thinking_tokens","session_id":"fixture"}),
        None,
    )
    .unwrap();
    e.observe(&json!({"type":"assistant","session_id":"fixture","message":{"content":[{"type":"tool_use","id":"report-tool","name":"StructuredOutput","input":report()}]}}),None).unwrap();
    let mut ack = json!({"type":"user","session_id":"fixture","message":{"content":[{"type":"tool_result","tool_use_id":"wrong"}]}});
    assert!(e.observe(&ack, None).is_err());
    ack["message"]["content"][0]["tool_use_id"] = json!("report-tool");
    e.observe(&ack, None).unwrap();
    let mut result = json!({"type":"result","subtype":"success","session_id":"fixture","is_error":false,"structured_output":report()});
    result["structured_output"]["summary"] = json!("changed");
    assert!(e.observe(&result, None).is_err());
    result["structured_output"] = report();
    e.observe(&result, None).unwrap();
    assert!(e.finish().is_ok());
}

#[test]
fn native_schema_rejection_can_be_corrected_without_replaying_a_worker() {
    let mut e = ClaudeEvents::default();
    e.observe(&init(), None).unwrap();
    let attempt = json!({"type":"assistant","session_id":"fixture","message":{"content":[{"type":"tool_use","id":"draft","name":"StructuredOutput","input":{"version":1}}]}});
    e.observe(&attempt, None).unwrap();
    e.observe(&json!({"type":"user","session_id":"fixture","message":{"content":[{"type":"tool_result","tool_use_id":"draft","is_error":true,"content":"Schema validation failed"}]}}),None).unwrap();
    let result = json!({"type":"result","subtype":"success","session_id":"fixture","is_error":false,"structured_output":report()});
    assert!(e.observe(&result, None).is_err());
    let mut corrected = attempt;
    corrected["message"]["content"][0]["input"] = report();
    corrected["message"]["content"][0]["id"] = json!("corrected");
    e.observe(&corrected, None).unwrap();
    e.observe(&json!({"type":"user","session_id":"fixture","message":{"content":[{"type":"tool_result","tool_use_id":"corrected"}]}}),None).unwrap();
    e.observe(&result, None).unwrap();
    assert_eq!(e.finish().unwrap().output.tool_error_count, 1);
}
