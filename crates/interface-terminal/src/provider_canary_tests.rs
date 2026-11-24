use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::*;

fn script(directory: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("fake CLI");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
    path
}

const FAKE_PROBES: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &["--resume", "--session-id"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "default"],
    },
    // A script carries no strings of its own: the probe does not apply.
    CanaryProbe::Executable {
        names: &["SessionStart"],
    },
];

const FAKE: CanaryExpectation = CanaryExpectation {
    profile: TerminalLaunchProfileId::Claude,
    command: "claude",
    override_env: None,
    verified: "2.1.300",
    probes: FAKE_PROBES,
    note: None,
};

/// A fake `claude` whose help lists `help` and that refuses any permission
/// mode but `accepted`.
fn fake_claude(directory: &TempDir, version: &str, help: &str, accepted: &str) -> PathBuf {
    script(
        directory,
        "claude",
        &format!(
            r#"case "$*" in
  "--version") echo "{version} (Claude Code)" ;;
  "--help") echo "Options: {help}" ;;
  "--permission-mode {accepted} --version") echo "{version} (Claude Code)" ;;
  *) echo "error: option '--permission-mode <mode>' argument is invalid" >&2; exit 1 ;;
esac"#
        ),
    )
}

#[test]
fn provider_canary_reports_a_matching_cli_and_names_each_drifted_flag_and_choice() {
    let directory = TempDir::new().expect("fake CLIs");
    let matching = fake_claude(&directory, "2.1.300", "--resume --session-id", "default");
    let report = run_canary(&FAKE, &matching);
    assert_eq!(report.version.as_deref(), Some("2.1.300 (Claude Code)"));
    assert_eq!(report.version_number.as_deref(), Some("2.1.300"));
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    // The version, the help and the accepted mode; the script has no strings.
    assert_eq!(report.checks, 3);
    assert_eq!(
        report.summary(),
        "Claude Code 2.1.300: what Interface depends on still matches (3 checks)"
    );

    let other = TempDir::new().expect("fake CLIs");
    let drifted = fake_claude(&other, "2.1.301", "--resume", "manual");
    let report = run_canary(&FAKE, &drifted);
    assert_eq!(
        report.findings,
        [
            "`claude --help` no longer lists --session-id.",
            "`claude --permission-mode default` is no longer accepted: error: option '--permission-mode <mode>' argument is invalid.",
        ]
    );
    assert!(report.summary().starts_with(
        "Claude Code 2.1.301 (verified at 2.1.300): 2 things Interface depends on no longer match: "
    ));
}

#[test]
fn provider_canary_stops_a_cli_that_does_not_answer_and_never_reads_stdin() {
    let directory = TempDir::new().expect("fake CLIs");
    let hung = script(&directory, "hung", "sleep 30");
    let started = Instant::now();
    let error = run_bounded(&hung, &["--help"], Duration::from_millis(300))
        .err()
        .expect("a hung CLI is stopped");
    assert_eq!(error, "did not answer within 0 s");
    assert!(started.elapsed() < Duration::from_secs(5));
    // stdin is closed: a CLI that waits on input reads end of file at once.
    let reader = script(&directory, "reader", "read line; echo \"read:[$line]\"");
    let answer = run_bounded(&reader, &[], Duration::from_secs(5)).expect("answers");
    assert_eq!(answer.text.trim(), "read:[]");
}

#[test]
fn provider_canary_finds_hook_names_in_an_executable_across_chunk_edges() {
    let directory = TempDir::new().expect("fake binary");
    let binary = directory.path().join("binary");
    let mut bytes = vec![0x7f_u8, b'E', b'L', b'F'];
    // Put one name across the scan's 4 MiB chunk boundary.
    bytes.resize(4 * 1024 * 1024 - 3, 0);
    bytes.extend_from_slice(b"SessionStart");
    bytes.extend_from_slice(b"\0PermissionRequest\0");
    fs::write(&binary, &bytes).expect("binary");
    assert_eq!(
        executable_strings(
            &binary,
            &["SessionStart", "PermissionRequest", "Stop_hook_gone"]
        )
        .expect("readable"),
        Some(vec!["Stop_hook_gone".to_owned()])
    );
    let shell = script(&directory, "shell", "exit 0");
    assert_eq!(
        executable_strings(&shell, &["SessionStart"]).expect("readable"),
        None
    );
}

#[test]
fn provider_canary_reads_codex_methods_from_its_generated_schema() {
    let directory = TempDir::new().expect("fake CLIs");
    let codex = script(
        &directory,
        "codex",
        r#"[ "$1 $2 $3" = "app-server generate-json-schema --out" ] || exit 2
printf '{"oneOf":[{"properties":{"method":{"enum":["thread/start"]}}}]}' > "$4/ClientRequest.json""#,
    );
    assert_eq!(codex_methods(&codex, &["thread/start"]), None);
    assert_eq!(
        codex_methods(&codex, &["thread/start", "turn/steer"]).as_deref(),
        Some("Codex's app-server schema no longer declares turn/steer.")
    );
}

#[test]
fn provider_canary_runs_only_version_help_listing_and_schema_commands() {
    // The canary must never start a model turn or spend quota: every command
    // it runs is a help, version, listing or schema command.
    for expectation in EXPECTATIONS {
        for probe in expectation.probes {
            match probe {
                CanaryProbe::Help { args, .. } => assert!(
                    args.last() == Some(&"--help") || *args == ["features", "list"],
                    "{} {args:?} is not a help or listing command",
                    expectation.command
                ),
                // `--version` is appended: the CLI parses its argv and prints its version.
                CanaryProbe::Accepts { args } => assert!(
                    !args
                        .iter()
                        .any(|arg| matches!(*arg, "-p" | "--print" | "exec" | "run")),
                    "{} {args:?} could run a turn",
                    expectation.command
                ),
                CanaryProbe::Executable { .. } => {}
                CanaryProbe::CodexMethods { .. } => assert_eq!(expectation.command, "codex"),
            }
        }
    }
    // Every launcher CLI has expectations, with a version they were verified at.
    for profile in crate::launch::PROFILE_ORDER {
        if profile == TerminalLaunchProfileId::Shell {
            continue;
        }
        let expectation = expectation(profile).expect("expectations for each launcher CLI");
        assert!(version_number(expectation.verified).is_some());
    }
    assert_eq!(expectations_fingerprint().len(), 16);
}

#[test]
fn provider_canary_reads_each_cli_version_format() {
    for (line, number) in [
        ("2.1.300 (Claude Code)", "2.1.300"),
        ("codex-cli 0.162.1", "0.162.1"),
        ("grok 1.0.30 (04b7ffed98c6) [stable]", "1.0.30"),
        ("omp/18.1.16", "18.1.16"),
        ("Muse Code 1.4.6 (1.4.6-R6000.1)", "1.4.6"),
        ("0.22.3", "0.22.3"),
    ] {
        assert_eq!(version_number(line).as_deref(), Some(number), "{line}");
    }
    assert_eq!(version_number("no version here"), None);
}

#[test]
fn provider_canary_finds_a_cli_by_override_path_or_usual_folder() {
    let directory = TempDir::new().expect("fake CLIs");
    let home = directory.path().join("home");
    fs::create_dir_all(home.join(".local/bin")).expect("home");
    let usual = home.join(".local/bin/claude");
    fs::write(&usual, "#!/bin/sh\n").expect("cli");
    fs::set_permissions(&usual, fs::Permissions::from_mode(0o700)).expect("permissions");
    let expectation = expectation(TerminalLaunchProfileId::Claude).expect("claude");
    let empty = directory.path().join("empty");
    fs::create_dir(&empty).expect("empty PATH entry");
    assert_eq!(
        locate(expectation, &home, Some(empty.as_os_str()), None),
        Some(usual.clone())
    );
    let elsewhere = script(&directory, "claude", "exit 0");
    assert_eq!(
        locate(
            expectation,
            Path::new("/nonexistent"),
            Some(directory.path().as_os_str()),
            None
        ),
        Some(elsewhere.clone())
    );
    assert_eq!(
        locate(expectation, &home, None, Some(elsewhere.as_os_str())),
        Some(elsewhere)
    );
}
