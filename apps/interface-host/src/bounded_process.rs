//! The host's one bounded child-process runner, shared by the pane-process
//! `ps` probes and the pane MCP config commands.
//!
//! The child runs in its own process group. It is bounded by a deadline that
//! covers both its exit and the close of its captured pipe, so a descendant
//! that inherited the pipe cannot hold the caller past the deadline; on a
//! timeout the whole group is stopped and reaped off the caller's thread.
//! The crates' runners keep their own loops (see `interface_common::process`).

use std::{
    io,
    os::unix::process::CommandExt,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use interface_common::process::{CapturedPipe, poll_backoff, terminate_process_group};

/// The one output pipe a run captures. The caller configures the others.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CapturedOutput {
    Stdout,
    Stderr,
}

/// How much of the captured pipe is kept, and whether more than that fails
/// the run (stopping the group) or is only dropped.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Capture {
    pub(crate) output: CapturedOutput,
    pub(crate) max_bytes: usize,
    pub(crate) overflow_fails: bool,
}

#[derive(Debug)]
pub(crate) enum BoundedRunError {
    /// The command did not start.
    Spawn(io::Error),
    /// The deadline passed before the child exited and its pipe closed.
    TimedOut,
    /// The child could not be polled.
    Wait,
    /// The captured output passed its bound while that fails the run.
    Overflow,
}

/// Runs `command` in a new process group until it exits and its captured
/// pipe closes, or until `deadline`. Returns the exit status and the captured
/// bytes, or the error reading them.
pub(crate) fn run_bounded(
    command: &mut Command,
    deadline: Duration,
    poll_cap: Duration,
    capture: Capture,
) -> Result<(ExitStatus, io::Result<Vec<u8>>), BoundedRunError> {
    match capture.output {
        CapturedOutput::Stdout => command.stdout(Stdio::piped()),
        CapturedOutput::Stderr => command.stderr(Stdio::piped()),
    };
    let mut child = command
        .process_group(0)
        .spawn()
        .map_err(BoundedRunError::Spawn)?;
    let overflow = Arc::new(AtomicBool::new(false));
    let pipe =
        match capture.output {
            CapturedOutput::Stdout => child.stdout.take().map(|output| {
                CapturedPipe::spawn(output, capture.max_bytes, Arc::clone(&overflow))
            }),
            CapturedOutput::Stderr => child.stderr.take().map(|output| {
                CapturedPipe::spawn(output, capture.max_bytes, Arc::clone(&overflow))
            }),
        };
    let Some(pipe) = pipe else {
        terminate_process_group(child);
        return Err(BoundedRunError::Wait);
    };
    let started = Instant::now();
    let mut status = None;
    loop {
        if started.elapsed() >= deadline {
            terminate_process_group(child);
            return Err(BoundedRunError::TimedOut);
        }
        if capture.overflow_fails && overflow.load(Ordering::Acquire) {
            terminate_process_group(child);
            return Err(BoundedRunError::Overflow);
        }
        if status.is_none() {
            if let Ok(observed) = child.try_wait() {
                status = observed;
                if status.is_some() {
                    pipe.child_exited(Some(child.id()));
                }
            } else {
                terminate_process_group(child);
                return Err(BoundedRunError::Wait);
            }
        }
        if let Some(status) = status.filter(|_| pipe.is_finished()) {
            return Ok((status, pipe.finish()));
        }
        std::thread::sleep(poll_backoff(started.elapsed(), poll_cap));
    }
}
