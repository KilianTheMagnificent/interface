use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

fn report() -> Value {
    json!({"version":1,"outcome":"changes","summary":"Changed a.rs","missingFiles":[],"files":[{"path":"a.rs","operation":"write","content":"new\n"}]})
}
fn started() -> CodexEvents {
    let mut events = CodexEvents::default();
    events
        .observe(
            &json!({"type":"thread.started","thread_id":"fixture"}),
            None,
        )
        .unwrap();
    events
        .observe(&json!({"type":"turn.started"}), None)
        .unwrap();
    events
}
#[test]
fn requires_complete_success_and_rejects_tool_activity() {
    for kind in [
        "command_execution",
        "file_change",
        "mcp_tool_call",
        "web_search",
        "error",
    ] {
        assert!(
            started()
                .observe(&json!({"type":"item.started","item":{"type":kind}}), None)
                .is_err()
        );
    }
    assert!(started().finish().is_err());
    let mut e = started();
    e.observe(&json!({"type":"item.completed","item":{"type":"agent_message","text":report().to_string()}}), None).unwrap();
    e.observe(
        &json!({"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}),
        None,
    )
    .unwrap();
    assert!(e.observe(&json!({"type":"turn.started"}), None).is_err());
    let r = e.finish().unwrap();
    assert_eq!(r.files[0].after.as_deref(), Some("new\n"));
    assert_eq!(r.output.usage.input_tokens, Some(7));
    assert_eq!(r.output.usage.cost_usd, None);
    assert_eq!(r.output.actual_model, None);
}
#[tokio::test]
async fn cli_is_isolated_and_returns_content_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let packet = temp.path().join("packet");
    fs::create_dir(&packet).unwrap();
    fs::write(packet.join("a.rs"), "old\n").unwrap();
    let exe = temp.path().join("worker.py");
    fs::write(&exe, format!(r"#!/usr/bin/python3
import json,os,pathlib,sys
args=sys.argv
if args[1:]==['debug','models','--bundled']:
 print(json.dumps({{'models':[{{'slug':'fixture','tool_mode':'code_mode_only','multi_agent_version':'v2','apply_patch_tool_type':'freeform'}}]}}));sys.exit(0)
assert '--ignore-user-config' in args and '--strict-config' in args
assert args[args.index('--sandbox')+1] == 'read-only'
assert args[args.index('--model')+1] == 'fixture'
for feature in ['shell_tool','unified_exec','apps','plugins','memories','multi_agent','browser_use','computer_use','view_image']:
 assert any(args[i:i+2] == ['--disable',feature] for i in range(len(args)))
assert not pathlib.Path('a.rs').exists()
assert 'INTERFACE_PANE_MCP_SOCKET' not in os.environ
assert 'task input' in sys.stdin.read()
for event in [{{'type':'thread.started','thread_id':'fixture'}},{{'type':'turn.started'}},{{'type':'item.completed','item':{{'type':'agent_message','text':json.dumps({})}}}},{{'type':'turn.completed','usage':{{'input_tokens':4,'output_tokens':6}}}}]:
 print(json.dumps(event),flush=True)
", report())).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let (_tx, rx) = watch::channel(false);
    let result = run_with_effort(&exe, &packet, "codex/fixture", "task input", rx, None, None)
        .await
        .unwrap();
    assert_eq!(result.files[0].after.as_deref(), Some("new\n"));
    assert_eq!(fs::read_to_string(packet.join("a.rs")).unwrap(), "old\n");
}
#[tokio::test]
async fn cancellation_kills_a_running_worker_and_preserves_source() {
    let temp = tempfile::tempdir().unwrap();
    let packet = temp.path().join("packet");
    fs::create_dir(&packet).unwrap();
    let exe = temp.path().join("wait.py");
    fs::write(&exe, "#!/usr/bin/python3\nimport time\ntime.sleep(120)\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        run_with_effort(&exe, &packet, "codex/fixture", "task", rx, None, None).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tx.send(true).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .contains("cancelled")
    );
}

#[test]
fn catalog_preserves_native_model_identity_but_removes_forced_tool_modes() {
    let native = json!({"models":[{"slug":"chosen","base_instructions":"native","context_window":1000,"tool_mode":"code_mode_only","apply_patch_tool_type":"freeform","multi_agent_version":"v2"},{"slug":"other"}]});
    let limited = restricted_catalog(&native, "chosen").unwrap();
    let entry = &limited["models"][0];
    assert_eq!(entry["slug"], "chosen");
    assert_eq!(entry["base_instructions"], "native");
    assert_eq!(entry["context_window"], 1000);
    assert!(entry["tool_mode"].is_null());
    assert!(entry["apply_patch_tool_type"].is_null());
    assert_eq!(entry["multi_agent_version"], "v1");
    assert_eq!(limited["models"].as_array().unwrap().len(), 1);
    assert!(restricted_catalog(&native, "missing").is_err());
}

#[tokio::test]
async fn the_bundled_catalog_is_asked_once_per_executable_file() {
    let temp = tempfile::tempdir().unwrap();
    let packet = temp.path().join("packet");
    fs::create_dir(&packet).unwrap();
    let asked = temp.path().join("asked");
    let exe = temp.path().join("catalog.py");
    fs::write(&exe, format!(r"#!/usr/bin/python3
import json,sys
args=sys.argv
if args[1:]==['debug','models','--bundled']:
 open({asked:?},'a').write('asked\n')
 print(json.dumps({{'models':[{{'slug':'fixture'}}]}}));sys.exit(0)
sys.stdin.read()
for event in [{{'type':'thread.started','thread_id':'fixture'}},{{'type':'turn.started'}},{{'type':'item.completed','item':{{'type':'agent_message','text':json.dumps({report})}}}},{{'type':'turn.completed','usage':{{'input_tokens':4,'output_tokens':6}}}}]:
 print(json.dumps(event),flush=True)
", asked = asked.to_str().unwrap(), report = report())).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    for _ in 0..2 {
        let (_tx, rx) = watch::channel(false);
        run_with_effort(&exe, &packet, "codex/fixture", "task", rx, None, None)
            .await
            .unwrap();
    }
    assert_eq!(fs::read_to_string(&asked).unwrap(), "asked\n");
    // An unknown model is still refused from the kept catalog.
    let (_tx, rx) = watch::channel(false);
    let refused = run_with_effort(&exe, &packet, "codex/unknown", "task", rx, None, None)
        .await
        .unwrap_err();
    assert!(
        refused.contains("not in the installed CLI catalog"),
        "{refused}"
    );
    assert_eq!(fs::read_to_string(&asked).unwrap(), "asked\n");
}
