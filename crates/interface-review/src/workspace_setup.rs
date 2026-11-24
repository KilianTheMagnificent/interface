//! The only executor for owner-saved worktree preparation. No repository discovery.
use crate::{
    process::{cap_utf8_owned, run_bounded_command},
    thread_git::ship::{filter_credentials, redact_credentials},
};
use interface_contracts::workflows::{ProjectSetup, SetupReport, SetupState, SetupStep};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

/// Run the exact saved commands once, prerequisites first. A failed, interrupted,
/// truncated or timed-out step prevents every later step and provider launch.
#[must_use]
pub fn prepare(cwd: &Path, setup: &ProjectSetup) -> SetupReport {
    let mut report = SetupReport {
        cwd: cwd.to_string_lossy().into_owned(),
        state: SetupState::Ready,
        steps: Vec::new(),
        message: if setup.commands.is_empty() && setup.prerequisites.is_empty() {
            "No setup commands configured. The worktree was created without dependency checks."
        } else {
            "The worktree is ready; all configured commands passed."
        }
        .into(),
    };
    if !(1..=600).contains(&setup.timeout_seconds)
        || setup.prerequisites.len() + setup.commands.len() > 12
    {
        report.state = SetupState::Failed;
        report.message = "The saved setup exceeds its execution bounds.".into();
        return report;
    }
    let snapshot = if setup.commands.is_empty() && setup.prerequisites.is_empty() {
        None
    } else if let Ok(snapshot) = interface_common::fsid::DirectorySnapshot::capture(cwd) {
        Some(snapshot)
    } else {
        report.state = SetupState::Failed;
        report.message =
            "The worktree folder changed or is reached through a symbolic link. No commands ran."
                .into();
        return report;
    };
    let started = Instant::now();
    let deadline = Duration::from_secs(u64::from(setup.timeout_seconds));
    for (prerequisite, commands) in [(true, &setup.prerequisites), (false, &setup.commands)] {
        for source in commands {
            if snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.assert_unchanged().is_err())
            {
                report.state = SetupState::Failed;
                report.message =
                    "The worktree folder changed during setup. No further commands ran.".into();
                return report;
            }
            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() || source.len() > 4096 || source.contains('\0') {
                report.state = SetupState::Failed;
                report.message =
                    "Setup exceeded its deadline or command bound. No session was started.".into();
                return report;
            }
            let mut command = Command::new("/bin/sh");
            command.args(["-c", source]).current_dir(cwd);
            // Explicit owner-authored shell code, with no copied credentials,
            // no stdin, and a process-group lifetime bounded by the total deadline.
            filter_credentials(&mut command);
            for (name, _) in std::env::vars_os() {
                if name.to_str().is_some_and(|name| {
                    name.starts_with("GIT_")
                        || name.starts_with("GH_")
                        || name.starts_with("GITHUB_")
                        || name.ends_with("_TOKEN")
                }) {
                    command.env_remove(name);
                }
            }
            command.env("GIT_TERMINAL_PROMPT", "0").env("CI", "1");
            let output = run_bounded_command(command, None, remaining, 12 * 1024);
            let succeeded = output.exit_code == Some(0)
                && !output.timed_out
                && !output.output_truncated
                && !output.unavailable;
            let (log, log_truncated) = cap_utf8_owned(
                redact_credentials(&format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )),
                24 * 1024,
            );
            report.steps.push(SetupStep {
                command: cap_utf8_owned(redact_credentials(source), 4096).0,
                prerequisite,
                output: log,
                exit_code: output.exit_code,
                timed_out: output.timed_out,
                truncated: output.output_truncated || log_truncated,
            });
            if !succeeded || log_truncated {
                report.state = SetupState::Failed;
                report.message = if prerequisite { "A prerequisite check failed. Setup and session launch were stopped." }
                    else { "Workspace setup failed. No session was started; inspect the retained worktree and logs." }.into();
                return report;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_prerequisite_failure_prevents_commands_and_retains_bounded_log() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        let report = prepare(
            &cwd,
            &ProjectSetup {
                prerequisites: vec!["printf 'missing tool'; exit 7".into()],
                commands: vec!["touch should-not-exist".into()],
                timeout_seconds: 2,
            },
        );
        assert_eq!(report.state, SetupState::Failed);
        assert_eq!(report.steps[0].exit_code, Some(7));
        assert!(report.steps[0].output.contains("missing tool"));
        assert!(!dir.path().join("should-not-exist").exists());
    }

    #[test]
    fn setup_runs_once_in_new_cwd_and_redacts_output() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        let setup = ProjectSetup {
            prerequisites: vec!["command -v sh".into()],
            commands: vec!["printf 'ready' > result; printf 'token=secret-value'".into()],
            timeout_seconds: 2,
        };
        let report = prepare(&cwd, &setup);
        assert_eq!(report.state, SetupState::Ready);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("result")).unwrap(),
            "ready"
        );
        assert!(!report.steps[1].output.contains("secret-value"));
    }

    #[test]
    fn setup_timeout_and_output_overflow_never_report_ready() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        for command in ["sleep 10", "yes output"] {
            let report = prepare(
                &cwd,
                &ProjectSetup {
                    commands: vec![command.into()],
                    prerequisites: vec![],
                    timeout_seconds: 1,
                },
            );
            assert_eq!(report.state, SetupState::Failed);
            assert!(report.steps[0].timed_out || report.steps[0].truncated);
            assert!(report.steps[0].output.len() <= 32 * 1024);
        }
    }
}
