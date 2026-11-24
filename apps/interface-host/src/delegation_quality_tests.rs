//! The deterministic gate for the quality grader the Away CLI embeds
//! (`scripts/delegation-quality-grade.py`). The live pane-delegation quality
//! runs that also used it were retired with pane delegation on 2026-10-14.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::process::Command;

fn grader() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/delegation-quality-grade.py")
}

#[tokio::test]
async fn independent_quality_grader_rejects_known_wrong_and_unsafe_outputs() {
    let output = Command::new("/usr/bin/python3")
        .args(["-I", "-S"])
        .arg(grader())
        .arg("--self-test")
        .env_clear()
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report, json!({"goodBaselines":10,"rejectedMutants":14}));
}
