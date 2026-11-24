use super::*;

#[test]
fn command_options_require_exact_controls_and_never_accept_a_resume() {
    let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    assert!(
        options(&args(
            "start --directory /tmp/new --config /tmp/config.json"
        ))
        .is_ok()
    );
    assert!(
        options(&args(
            "feedback --directory /tmp/run --id review-1 --message fix"
        ))
        .is_ok()
    );
    for input in [
        "resume --directory /tmp/run",
        "start --directory relative --config /tmp/c",
        "stop --directory /tmp/r --duration 50",
        "status --directory /tmp/a --directory /tmp/b",
    ] {
        assert!(options(&args(input)).is_err());
    }
}

#[test]
fn option_refusals_keep_their_cycle_wording() {
    let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    let unknown = "Unknown or duplicate cycle option";
    let incomplete = "Cycle requires every named option and an absolute run directory";
    // Only the run directory has to be absolute.
    assert!(options(&args("start --config relative --directory /tmp/new")).is_ok());
    for (input, message) in [
        ("stop --directory /tmp/r --duration 50", unknown),
        ("status --directory /tmp/a --directory /tmp/b", unknown),
        ("start --directory relative --config /tmp/c", incomplete),
        ("start --directory /tmp/new", incomplete),
        ("status --directory", incomplete),
    ] {
        assert_eq!(options(&args(input)).unwrap_err(), message, "{input}");
    }
}

#[tokio::test]
async fn real_check_failure_retains_output_for_refinement() {
    let dir = tempfile::tempdir().unwrap();
    let result = checks::run(
        &[
            "/usr/bin/python3".into(),
            "-c".into(),
            "import sys; print('counterexample: empty input'); sys.exit(1)".into(),
        ],
        dir.path(),
    )
    .await
    .unwrap();
    assert_eq!(result["passed"], false);
    assert_eq!(result["exitCode"], 1);
    assert!(
        result["stdout"]
            .as_str()
            .unwrap()
            .contains("counterexample")
    );
}

#[tokio::test]
async fn dropping_a_running_check_terminates_its_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let script = "import subprocess, pathlib, time\np=subprocess.Popen(['/usr/bin/python3','-c',\"import time,pathlib; time.sleep(.4); pathlib.Path('escaped').write_text('alive')\"])\npathlib.Path('child.pid').write_text(str(p.pid))\ntime.sleep(30)";
    let argv = vec!["/usr/bin/python3".into(), "-c".into(), script.into()];
    let mut work = Box::pin(checks::run(&argv, dir.path()));
    tokio::select! {result=&mut work=>panic!("unexpected completion {result:?}"),()=async {while !dir.path().join("child.pid").exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}=>{},()=tokio::time::sleep(std::time::Duration::from_secs(2))=>panic!("check did not start")}
    drop(work);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        !dir.path().join("escaped").exists(),
        "descendant survived cancellation"
    );
}
