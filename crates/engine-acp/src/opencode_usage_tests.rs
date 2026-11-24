use super::*;

const FULL: &str = include_str!("../../../electron/src/fixtures/opencode-go-usage-v1.json");
const PARTIAL: &str =
    include_str!("../../../electron/src/fixtures/opencode-go-usage-partial-v1.json");
const PROJECTED: &str =
    include_str!("../../../electron/src/fixtures/opencode-go-usage-projection-v1.json");

fn response(status: u16, body: Value) -> String {
    let mut frame = json!({"version":1,"type":"response","status":status});
    frame["body"] = body;
    format!(
        "{PREFIX}{}\n{PREFIX}{}\n",
        json!({"version":1,"type":"ready"}),
        frame
    )
}

#[test]
fn provider_windows_match_the_renderer_contract_without_inventing_spend() {
    let usage = parse_go_usage(&serde_json::from_str(FULL).unwrap());
    assert_eq!(
        serde_json::to_value(&usage).unwrap(),
        serde_json::from_str::<Value>(PROJECTED).unwrap()
    );
    assert!(usage.cost_usd.is_none());
    assert_eq!(
        parse_response(
            response(200, serde_json::from_str(FULL).unwrap()).as_bytes(),
            true
        ),
        usage
    );
}

#[test]
fn zero_is_reported_but_missing_malformed_and_unknown_windows_are_not_zero() {
    let usage = parse_go_usage(&serde_json::from_str(PARTIAL).unwrap());
    assert!(usage.available);
    assert_eq!(usage.windows.len(), 3);
    assert_eq!(usage.windows[0].utilization, Some(0.0));
    for window in &usage.windows[1..] {
        assert_eq!(window.utilization, None);
        assert_eq!(window.resets_at, None);
    }
    for payload in [
        json!({}),
        json!({"usage":{}}),
        json!({"usage":{"rolling":{"status":"ok","percent":-1}}}),
        json!({"usage":{"rolling":{"status":"ok","percent":"0"}}}),
    ] {
        let usage = parse_go_usage(&payload);
        assert!(!usage.available);
        assert!(usage.windows.is_empty());
    }
}

#[test]
fn invalid_reset_dates_do_not_become_countdowns() {
    for invalid in [
        "tomorrow",
        "2026-02-31T20:00:00.000Z",
        "2026-13-01T20:00:00.000Z",
        "2026-09-07T25:00:00.000Z",
        "2026-09-07T20:99:00.000Z",
    ] {
        let mut payload: Value = serde_json::from_str(FULL).unwrap();
        payload["usage"]["rolling"]["resetsAt"] = invalid.into();
        let usage = parse_go_usage(&payload);
        assert_eq!(usage.windows[0].resets_at, None);
        assert_eq!(usage.windows[0].utilization, Some(12.5));
    }
}

#[test]
fn failures_and_ambiguous_transport_frames_never_publish_zero_usage() {
    let body: Value = serde_json::from_str(FULL).unwrap();
    let valid = response(200, body);
    for raw in [
        String::new(),
        "opencode-go/minimax-m3\n".into(),
        valid.replace("\"version\":1", "\"version\":2"),
        format!("{valid}{valid}"),
        response(401, json!({"private":"must never be forwarded"})),
        response(403, json!({})),
        response(429, json!({})),
        response(500, json!({})),
        format!("{PREFIX}{{bad json}}\n"),
    ] {
        let usage = parse_response(raw.as_bytes(), true);
        assert!(!usage.available, "{raw}");
        assert!(usage.windows.is_empty());
        assert!(!usage.note.unwrap().contains("must never be forwarded"));
    }
    assert!(!parse_response(valid.as_bytes(), false).available);
    assert!(
        parse_response(response(401, Value::Null).as_bytes(), true)
            .note
            .unwrap()
            .contains("/connect")
    );
    assert!(
        parse_response(response(403, Value::Null).as_bytes(), true)
            .note
            .unwrap()
            .contains("subscription")
    );
}

#[test]
fn version_gate_prevents_unsafe_or_unverified_cli_grammars() {
    for version in ["1.18.29\n", "1.19.0", "1.20.7"] {
        assert!(supported_version(version.as_bytes()));
    }
    for version in [
        "1.18.28",
        "2.0.0",
        "1.18.29-beta",
        "1.18.29+local",
        "unknown",
        "1.18",
        "",
    ] {
        assert!(!supported_version(version.as_bytes()));
    }
}

#[cfg(unix)]
fn fake_cli(root: &std::path::Path, pause: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let executable = root.join("fake-opencode");
    let script = format!(
        r"#!/usr/bin/python3
import json, os, pathlib, stat, sys, time
if sys.argv[1:] == ['--version']:
    print('1.18.29')
    sys.exit(0)
assert sys.argv[1:] == ['models', 'opencode-go']
cfg = json.loads(os.environ['OPENCODE_CONFIG_CONTENT'])
assert cfg['enabled_providers'] == ['opencode-go']
assert cfg['mcp'] == {{}} and cfg['instructions'] == []
assert len(cfg['plugin']) == 1 and pathlib.Path(cfg['plugin'][0]).is_file()
assert os.environ['OPENCODE_DISABLE_PROJECT_CONFIG'] == 'true'
assert os.environ['OPENCODE_DISABLE_DEFAULT_PLUGINS'] == 'true'
assert os.environ['OPENCODE_DISABLE_MODELS_FETCH'] == 'true'
assert 'INTERFACE_PANE_MCP_SOCKET' not in os.environ
assert stat.S_IMODE(pathlib.Path(os.environ['XDG_CONFIG_HOME'], 'opencode').stat().st_mode) == 0o500
if {pause}:
    time.sleep(60)
print({output})
",
        pause = if pause { "True" } else { "False" },
        output =
            serde_json::to_string(&response(200, serde_json::from_str(FULL).unwrap())).unwrap()
    );
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    executable
}

#[cfg(unix)]
#[tokio::test]
async fn reader_runs_only_a_read_only_provider_listing_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let reader =
        OpenCodeGoUsageReader::new(fake_cli(root.path(), false), root.path().to_path_buf());
    let usage = reader.read_usage().await;
    assert!(usage.available, "{:?}", usage.note);
    assert_eq!(usage.windows[1].utilization, Some(34.0));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn version_probe_is_cached_across_usage_reads_until_executable_changes() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("fake-opencode-counted");
    let count_file = root.path().join("version-calls.txt");
    let write_cli = |version: &str| {
        let script = format!(
            r"#!/usr/bin/python3
import pathlib, sys
if sys.argv[1:] == ['--version']:
    log = pathlib.Path({count_file})
    log.write_text((log.read_text() if log.exists() else '') + 'v\n')
    print({version:?})
    sys.exit(0)
assert sys.argv[1:] == ['models', 'opencode-go']
print({output})
",
            count_file = serde_json::to_string(count_file.to_str().unwrap()).unwrap(),
            output =
                serde_json::to_string(&response(200, serde_json::from_str(FULL).unwrap())).unwrap(),
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    };

    write_cli("1.18.29");
    let reader = OpenCodeGoUsageReader::new(executable.clone(), root.path().to_path_buf());
    assert!(reader.read_usage().await.available);
    assert!(reader.read_usage().await.available);
    assert_eq!(
        std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count(),
        1
    );

    write_cli("1.18.28");
    assert!(!reader.read_usage().await.available);
    assert_eq!(
        std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_hung_cli_is_bounded_and_never_reports_zero() {
    let root = tempfile::tempdir().unwrap();
    let reader = OpenCodeGoUsageReader::new(fake_cli(root.path(), true), root.path().to_path_buf());
    let usage = reader.read_with_timeout(Duration::from_millis(400)).await;
    assert!(!usage.available);
    assert!(usage.windows.is_empty());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
#[ignore = "requires INTERFACE_TEST_OPENCODE and a connected Go account; read-only, no model call"]
async fn live_opencode_go_allowance_uses_the_provider_auth_boundary() {
    let executable = std::env::var_os("INTERFACE_TEST_OPENCODE").expect("explicit live CLI path");
    let root = tempfile::tempdir().unwrap();
    let usage = OpenCodeGoUsageReader::new(executable.into(), root.path().to_path_buf())
        .read_usage()
        .await;
    assert!(usage.available, "{:?}", usage.note);
    assert_eq!(usage.plan.as_deref(), Some("Go"));
    assert_eq!(usage.windows.len(), 3);
    assert!(
        usage
            .windows
            .iter()
            .all(|window| window.utilization.is_some() && window.resets_at.is_some())
    );
    assert!(usage.cost_usd.is_none());
    println!("{}", serde_json::to_string(&usage).unwrap());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
