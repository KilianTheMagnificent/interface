use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use interface_contracts::engine::EngineId;
use interface_contracts::ipc::{CHANNELS, EngineProbe};
use serde_json::{Value, json};

use super::*;

#[derive(Default)]
struct FakeProbes;

#[async_trait]
impl ProviderProbes for FakeProbes {
    async fn probes(&self) -> Vec<EngineProbe> {
        vec![EngineProbe {
            engine: EngineId::new("codex").expect("engine"),
            ok: false,
            version: None,
            info: None,
            error: Some("initialize: Codex app-server closed stdout".into()),
        }]
    }
}

struct FixedAppVersion;

#[async_trait]
impl AppVersion for FixedAppVersion {
    async fn app_version(&self) -> Option<String> {
        Some("0.3.158".into())
    }
}

#[derive(Default)]
struct RecordingRevealer {
    revealed: Mutex<Vec<(PathBuf, bool)>>,
}

impl Revealer for RecordingRevealer {
    fn reveal(&self, path: &Path, select: bool) -> Result<(), String> {
        self.revealed
            .lock()
            .expect("revealed")
            .push((path.to_path_buf(), select));
        Ok(())
    }
}

struct Fixture {
    temporary: tempfile::TempDir,
    data_root: PathBuf,
    host: DiagnosticsHost,
    notices: Arc<StoreNotices>,
    revealer: Arc<RecordingRevealer>,
}

fn fixture() -> Fixture {
    let temporary = tempfile::tempdir().expect("temporary");
    let home = temporary.path().join("home");
    let data_root = home.join("Library/Application Support/Interface");
    std::fs::create_dir_all(&data_root).expect("data root");
    let notices = Arc::new(StoreNotices::default());
    let revealer = Arc::new(RecordingRevealer::default());
    let host = DiagnosticsHost::new(DiagnosticsHostConfig {
        data_root: data_root.clone(),
        home,
        product_version: "0.3.10".into(),
        app_version: Some(Arc::new(FixedAppVersion)),
        probes: Arc::new(FakeProbes),
        notices: notices.clone(),
        revealer: revealer.clone(),
    });
    Fixture {
        temporary,
        data_root,
        host,
        notices,
        revealer,
    }
}

async fn ask(host: &DiagnosticsHost, payload: Value) -> Result<Value, DispatchError> {
    host.dispatch(CHANNELS.diagnostics, payload).await
}

#[tokio::test]
async fn a_set_aside_store_is_noticed_once_until_dismissed_and_reveals_its_file() {
    let fixture = fixture();
    assert_eq!(
        ask(&fixture.host, json!({"kind": "notices"}))
            .await
            .expect("notices"),
        json!({"notices": []})
    );
    let artifact = fixture
        .data_root
        .join(".quarantine/q-1-corrupt.quarantined");
    fixture.notices.record(SetAsideStore::Threads, &artifact);
    let listed = ask(&fixture.host, json!({"kind": "notices"}))
        .await
        .expect("notices");
    assert_eq!(
        listed,
        json!({"notices": [{"id": "threads-1", "what": "your Threads list"}]})
    );
    // Asking again (a reloaded window) still shows it: only a dismissal ends it.
    assert_eq!(
        ask(&fixture.host, json!({"kind": "notices"}))
            .await
            .expect("notices"),
        listed
    );
    ask(
        &fixture.host,
        json!({"kind": "reveal", "target": "notice", "id": "threads-1"}),
    )
    .await
    .expect("reveal");
    assert_eq!(
        *fixture.revealer.revealed.lock().expect("revealed"),
        vec![(artifact, true)]
    );
    assert_eq!(
        ask(
            &fixture.host,
            json!({"kind": "dismissNotice", "id": "threads-1"})
        )
        .await
        .expect("dismiss"),
        json!({"notices": []})
    );
    let error = ask(
        &fixture.host,
        json!({"kind": "reveal", "target": "notice", "id": "threads-1"}),
    )
    .await
    .expect_err("a dismissed notice reveals nothing");
    assert_eq!(error.code, "diagnostics-unknown-notice");
    assert!(error.replay_safe);
}

#[tokio::test]
async fn reveal_logs_selects_host_log_and_reveal_data_folder_opens_the_folder() {
    let fixture = fixture();
    ask(&fixture.host, json!({"kind": "reveal", "target": "logs"}))
        .await
        .expect("reveal logs");
    std::fs::write(fixture.data_root.join("host.log"), "WARN x\n").expect("log");
    ask(&fixture.host, json!({"kind": "reveal", "target": "logs"}))
        .await
        .expect("reveal logs");
    ask(
        &fixture.host,
        json!({"kind": "reveal", "target": "dataFolder"}),
    )
    .await
    .expect("reveal data");
    assert_eq!(
        *fixture.revealer.revealed.lock().expect("revealed"),
        vec![
            // No warning yet: the folder the log will be in.
            (fixture.data_root.clone(), false),
            (fixture.data_root.join("host.log"), true),
            (fixture.data_root.clone(), false),
        ]
    );
}

#[tokio::test]
async fn copy_diagnostics_report_quotes_probes_and_the_log_tail_filtered() {
    let fixture = fixture();
    std::fs::write(
        fixture.data_root.join("host.log"),
        format!(
            "2026-10-14T12:00:00Z  WARN interface_host: retry token=ghp_abcdefghijklmnopqrstuvwxyz0123 in {}\n",
            fixture.data_root.display()
        ),
    )
    .expect("log");
    let answer = ask(&fixture.host, json!({"kind": "report"}))
        .await
        .expect("report");
    let report = answer["report"].as_str().expect("report text");
    assert!(
        report.starts_with("Interface diagnostics\nVersion: 0.3.158 (host 0.3.10)\n"),
        "{report}"
    );
    assert!(
        report.contains("Data folder: ~/Library/Application Support/Interface"),
        "{report}"
    );
    assert!(
        report.contains("- Codex: Unavailable · initialize: Codex app-server closed stdout"),
        "{report}"
    );
    assert!(
        report.contains("retry token=[redacted] in ~/Library"),
        "{report}"
    );
    assert!(!report.contains("ghp_"), "{report}");
    assert!(
        !report.contains(&fixture.temporary.path().display().to_string()),
        "{report}"
    );
}

#[tokio::test]
async fn malformed_diagnostics_requests_are_refused_unsent() {
    let fixture = fixture();
    for payload in [
        json!(null),
        json!([]),
        json!({"kind": "reveal", "target": "notice"}),
        json!({"kind": "reveal", "target": "logs", "id": "threads-1"}),
        json!({"kind": "dismissNotice", "id": "../x"}),
        json!({"kind": "shred"}),
    ] {
        let error = ask(&fixture.host, payload.clone())
            .await
            .expect_err("refused");
        assert_eq!(error.code, "diagnostics-invalid", "{payload}");
        assert!(error.replay_safe, "{payload}");
    }
    assert!(
        fixture
            .revealer
            .revealed
            .lock()
            .expect("revealed")
            .is_empty()
    );
}

#[test]
fn the_macos_version_comes_from_the_system_version_file() {
    let temporary = tempfile::tempdir().expect("temporary");
    let plist = temporary.path().join("SystemVersion.plist");
    std::fs::write(
        &plist,
        "<plist><dict><key>ProductBuildVersion</key>\n\t<string>25B78</string>\n\
         <key>ProductName</key><string>macOS</string>\
         <key>ProductVersion</key><string>26.1</string></dict></plist>",
    )
    .expect("plist");
    assert_eq!(macos_version(&plist).as_deref(), Some("macOS 26.1 (25B78)"));
    assert_eq!(macos_version(&temporary.path().join("missing")), None);
}
