use std::io::Write as _;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use interface_common::git_hardening;
use interface_common::process::{CapturedPipe, poll_backoff, terminate_process_group};

use crate::checks::ReviewCheckRunnerResult as ProcessOutput;

/// What a bounded child left behind, as raw bytes.
#[derive(Debug, Default)]
pub(crate) struct BoundedOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub unavailable: bool,
}

// Deliberately not shared with `interface-registry`'s git runner: this one keeps
// the exit code observed after a kill, distinguishes timeout from overflow from
// spawn failure on `ProcessOutput`, and backs off from 100µs to a 5ms cap. Only
// Pipe capture and process-group teardown are shared; result policy stays here.
pub(crate) fn run_bounded_process(
    executable: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    max_output_bytes: usize,
) -> ProcessOutput {
    let mut command = scrubbed_command(executable, cwd);
    command.args(args);
    process_output(&run_bounded_command(
        command,
        None,
        timeout,
        max_output_bytes,
    ))
}

/// `git args` in `cwd` as [`run_bounded_process`] runs it, except that no
/// program the repository's own settings name can run: the settings are
/// listed first ([`git_hardening::PROBE_ARGS`]) and the call runs with
/// [`git_hardening::Overrides`] and [`git_hardening::STATIC_ARGS`]. When the
/// listing fails, git is not run at all and the listing's failure is the
/// answer. Both runs share `timeout`.
pub(crate) fn run_hardened_git(
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    max_output_bytes: usize,
) -> ProcessOutput {
    let started = Instant::now();
    let mut probe = scrubbed_command("git", cwd);
    probe.args(git_hardening::PROBE_ARGS);
    let listed = run_bounded_command(probe, None, timeout, git_hardening::PROBE_MAX_BYTES);
    if listed.unavailable || listed.timed_out {
        return process_output(&listed);
    }
    let overrides = (git_hardening::probe_answered(listed.exit_code) && !listed.output_truncated)
        .then(|| git_hardening::Overrides::from_listing(&listed.stdout))
        .flatten();
    let Some(overrides) = overrides else {
        let stderr = String::from_utf8_lossy(&listed.stderr);
        return ProcessOutput {
            exit_code: Some(listed.exit_code.filter(|code| *code > 1).unwrap_or(128)),
            stderr: format!("{} {}", git_hardening::PROBE_REFUSED, stderr.trim())
                .trim_end()
                .to_owned(),
            ..ProcessOutput::default()
        };
    };
    let mut command = scrubbed_command("git", cwd);
    overrides.apply(&mut command);
    command.args(git_hardening::STATIC_ARGS).args(args);
    process_output(&run_bounded_command(
        command,
        None,
        timeout.saturating_sub(started.elapsed()),
        max_output_bytes,
    ))
}

/// `executable` in `cwd` without the inherited `GIT_*` variables, GitHub's
/// tokens or Interface's and the providers' secrets, never prompting.
fn scrubbed_command(executable: &str, cwd: &Path) -> Command {
    let mut command = Command::new(executable);
    for (name, _) in std::env::vars_os() {
        let remove = name.as_encoded_bytes().starts_with(b"GIT_")
            || name.to_str().is_some_and(|key| {
                let upper = key.to_ascii_uppercase();
                upper.starts_with("GH_")
                    || upper.starts_with("GITHUB_")
                    || crate::thread_git::ship::credential_variable(key)
            });
        if remove {
            command.env_remove(name);
        }
    }
    command
        .env_remove("INTERFACE_PANE_MCP_SOCKET")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(cwd);
    command
}

fn process_output(output: &BoundedOutput) -> ProcessOutput {
    ProcessOutput {
        exit_code: output.exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        timed_out: output.timed_out,
        output_truncated: output.output_truncated,
        unavailable: output.unavailable,
    }
}

/// Runs a prepared `command` (program, arguments, folder and environment
/// already set) in its own process group under the same deadline and output
/// cap as [`run_bounded_process`]. `stdin`, when given, is written from a
/// helper thread and then closed; otherwise the child reads `/dev/null`.
pub(crate) fn run_bounded_command(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    max_output_bytes: usize,
) -> BoundedOutput {
    let maximum = max_output_bytes.max(1);
    command.process_group(0);
    let child = command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return BoundedOutput {
                stderr: error.to_string().into_bytes(),
                unavailable: true,
                ..BoundedOutput::default()
            };
        }
    };
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Detached: a child that stops reading gets EPIPE once it exits or
        // is torn down, which ends the write.
        thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_overflow = Arc::clone(&overflow);
    let stderr_overflow = Arc::clone(&overflow);
    let stdout_reader = CapturedPipe::spawn(stdout, maximum, stdout_overflow);
    let stderr_reader = CapturedPipe::spawn(stderr, maximum, stderr_overflow);

    let started = Instant::now();
    let mut timed_out = false;
    let mut overflow_kill = false;
    let mut process_failed = false;
    let mut status = None;
    loop {
        if overflow.load(Ordering::Acquire) {
            overflow_kill = true;
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            break;
        }
        if status.is_none() {
            if let Ok(observed) = child.try_wait() {
                status = observed;
                if status.is_some() {
                    stdout_reader.child_exited(Some(child.id()));
                    stderr_reader.child_exited(Some(child.id()));
                }
            } else {
                process_failed = true;
                break;
            }
        }
        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break;
        }
        let elapsed = started.elapsed();
        let remaining = timeout.saturating_sub(elapsed);
        // git answers within milliseconds; a check run can take minutes, and
        // waking every 5 ms for those buys nothing.
        let cap = if elapsed < Duration::from_secs(1) {
            Duration::from_millis(5)
        } else {
            Duration::from_millis(50)
        };
        thread::sleep(poll_backoff(elapsed, cap).min(remaining));
    }
    let exit_code = status.and_then(|status| status.code());
    if timed_out || overflow_kill || process_failed {
        terminate_process_group(child);
    }

    let stdout = stdout_reader.finish();
    let stderr = stderr_reader.finish();
    process_failed |= stdout.is_err() || stderr.is_err();
    BoundedOutput {
        exit_code,
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
        timed_out,
        output_truncated: overflow_kill || overflow.load(Ordering::Acquire),
        unavailable: process_failed,
    }
}

pub(crate) fn cap_utf8_owned(mut value: String, maximum: usize) -> (String, bool) {
    if value.len() <= maximum {
        return (value, false);
    }
    let mut end = maximum;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::run_bounded_process;

    #[test]
    fn direct_process_runner_kills_on_output_and_time_limits() {
        let output = run_bounded_process(
            "/usr/bin/yes",
            &[],
            Path::new("/"),
            Duration::from_secs(2),
            128,
        );
        assert!(output.output_truncated);
        assert!(output.stdout.len() <= 128);

        let timeout = run_bounded_process(
            "/bin/sleep",
            &["1".to_owned()],
            Path::new("/"),
            Duration::from_millis(20),
            128,
        );
        assert!(timeout.timed_out);
    }

    #[test]
    fn deadline_covers_descendants_holding_stdout_and_stderr_after_leader_exit() {
        for pipe in ["printf ready", "printf ready >&2"] {
            let started = std::time::Instant::now();
            let output = run_bounded_process(
                "/bin/sh",
                &["-c".into(), format!("{pipe}; /bin/sleep 2 & exit 0")],
                Path::new("/"),
                Duration::from_millis(100),
                128,
            );
            assert!(
                output.timed_out,
                "an inherited pipe is part of the command deadline"
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(format!("{}{}", output.stdout, output.stderr), "ready");
        }
    }
}
