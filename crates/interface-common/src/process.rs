//! Child-process plumbing shared by the bounded subprocess runners.
//!
//! Byte-capped pipe drains, group teardown and poll backoff live here. The runners
//! around them — the registry's git runner and the review runner — differ in
//! what they do with the exit status after a kill and what they report, so each
//! keeps its own loop.

use std::io::{self, Read};
use std::process::Child;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How long a drained pipe must stay empty, once its child is done, before
/// the drain stops waiting for the pipe to close.
///
/// A child's own output is in the pipe by the time it exits. Something else
/// may still hold the pipe open: on macOS a pipe becomes close-on-exec only
/// after it is created, so a child another thread spawns in that moment (a
/// pane's shell, say) inherits it and can hold it for hours. That must not
/// make a finished command look hung.
pub const PIPE_IDLE_AFTER_EXIT: Duration = Duration::from_millis(100);

/// Whether the process group `group` has no process left. The caller made
/// it with `process_group(0)` and has reaped its leader; a group that cannot
/// be checked counts as still running.
#[must_use]
pub fn process_group_is_empty(group: u32) -> bool {
    #[cfg(unix)]
    {
        i32::try_from(group)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .is_some_and(|group| {
                rustix::process::test_kill_process_group(group) == Err(rustix::io::Errno::SRCH)
            })
    }
    #[cfg(not(unix))]
    {
        let _ = group;
        false
    }
}

/// What a captured pipe's reader must be: on Unix it is also polled, so
/// its drain can stop once the child is done and the pipe is empty.
#[cfg(unix)]
pub trait PipeReader: Read + std::os::fd::AsFd + Send + 'static {}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsFd + Send + 'static> PipeReader for T {}
#[cfg(not(unix))]
pub trait PipeReader: Read + Send + 'static {}
#[cfg(not(unix))]
impl<T: Read + Send + 'static> PipeReader for T {}

/// Drains `reader` to end of stream, keeping at most `maximum` bytes and
/// setting `overflow` as soon as a byte is dropped.
///
/// The reader is always drained: the caller decides whether an overflow means
/// kill the child, and stopping early would block the child on a full pipe.
///
/// # Errors
///
/// Returns the first read error from `reader`.
pub fn read_capped(
    mut reader: impl Read,
    maximum: usize,
    overflow: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut kept = Vec::with_capacity(maximum.min(64 * 1024));
    drain_capped(&mut reader, maximum, overflow, |bytes| {
        kept.extend_from_slice(bytes);
    })?;
    Ok(kept)
}

fn drain_capped(
    reader: impl Read,
    maximum: usize,
    overflow: &AtomicBool,
    append: impl FnMut(&[u8]),
) -> io::Result<()> {
    drain_capped_while(reader, maximum, overflow, |_| Ok(true), append)
}

/// [`drain_capped`], asking `more` before each read whether to read on; a
/// `false` answer ends the drain.
fn drain_capped_while<R: Read>(
    mut reader: R,
    maximum: usize,
    overflow: &AtomicBool,
    mut more: impl FnMut(&R) -> io::Result<bool>,
    mut append: impl FnMut(&[u8]),
) -> io::Result<()> {
    let mut kept = 0;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        if !more(&reader)? {
            return Ok(());
        }
        let read = match reader.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            return Ok(());
        }
        let room = maximum.saturating_sub(kept);
        let copy = room.min(read);
        if copy != 0 {
            append(&buffer[..copy]);
            kept += copy;
        }
        if copy < read {
            overflow.store(true, Ordering::Release);
        }
    }
}

/// Waits until `reader` has bytes (or its end) to read: `true`. Once
/// `exited` names the finished child's group (or `None`, it had no group of
/// its own) and that group is empty, a pipe that stays empty for
/// [`PIPE_IDLE_AFTER_EXIT`] answers `false` instead of waiting to close.
#[cfg(unix)]
fn readable_until_done(
    reader: &impl std::os::fd::AsFd,
    exited: &OnceLock<Option<u32>>,
) -> io::Result<bool> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    const WAIT: Timespec = Timespec {
        tv_sec: 0,
        tv_nsec: 20_000_000,
    };
    let mut idle = Duration::ZERO;
    loop {
        // Read before the wait: a pipe that stayed empty for a whole wait
        // after the child was done can no longer get the child's output.
        let done = exited
            .get()
            .is_some_and(|group| group.is_none_or(process_group_is_empty));
        let mut fds = [PollFd::new(reader, PollFlags::IN)];
        match poll(&mut fds, Some(&WAIT)) {
            Ok(0) if done => {
                idle += Duration::from_millis(20);
                if idle >= PIPE_IDLE_AFTER_EXIT {
                    return Ok(false);
                }
            }
            Ok(0) | Err(rustix::io::Errno::INTR) => idle = Duration::ZERO,
            Ok(_) => return Ok(true),
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(not(unix))]
fn readable_until_done(_reader: &impl Read, _exited: &OnceLock<Option<u32>>) -> io::Result<bool> {
    Ok(true)
}

/// A capped pipe drain whose completion can be polled alongside the child.
/// An inherited pipe must not turn a bounded subprocess into an unbounded join.
pub struct CapturedPipe {
    bytes: Arc<Mutex<Vec<u8>>>,
    exited: Arc<OnceLock<Option<u32>>>,
    reader: JoinHandle<io::Result<()>>,
}

impl CapturedPipe {
    #[must_use]
    pub fn spawn(reader: impl PipeReader, maximum: usize, overflow: Arc<AtomicBool>) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::with_capacity(maximum.min(64 * 1024))));
        let exited = Arc::new(OnceLock::new());
        let output = Arc::clone(&bytes);
        let done = Arc::clone(&exited);
        let reader = thread::spawn(move || {
            drain_capped_while(
                reader,
                maximum,
                &overflow,
                |reader| readable_until_done(reader, &done),
                |chunk| {
                    output
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend_from_slice(chunk);
                },
            )
        });
        Self {
            bytes,
            exited,
            reader,
        }
    }

    /// The child has exited. `group` is the process group it led (started
    /// with `process_group(0)`), whose remaining members may still write;
    /// `None` for a child without one. Once that group is empty too, the
    /// drain ends when the pipe stays empty for [`PIPE_IDLE_AFTER_EXIT`],
    /// even if another process still holds it open.
    pub fn child_exited(&self, group: Option<u32>) {
        let _ = self.exited.set(group);
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.reader.is_finished()
    }

    /// Returns the captured prefix without waiting for an inherited pipe.
    /// Callers must classify an unfinished drain as a timeout/failure first.
    ///
    /// # Errors
    /// Returns a completed reader's I/O error or panic as an I/O error.
    pub fn finish(self) -> io::Result<Vec<u8>> {
        if self.reader.is_finished() {
            self.reader
                .join()
                .map_err(|_| io::Error::other("child output reader failed"))??;
        }
        let mut bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(std::mem::take(&mut *bytes))
    }
}

/// Stops a command launched with `CommandExt::process_group(0)`, including
/// descendants holding its output pipes. Reaping is off the caller's deadline.
/// Only use this for a process group created by the caller.
pub fn terminate_process_group(mut child: Child) {
    // `process_group(0)` made the child its group's leader, so its pid names
    // the group. An already-gone group (ESRCH) is fine: the kill is best effort.
    #[cfg(unix)]
    if let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Sleep interval for a bounded wait on a child process, tightest at the start.
///
/// Both process runners poll a child they have just spawned: the first
/// milliseconds are when a short command exits, so the wait is fine-grained
/// there and settles to `cap` afterwards.
#[must_use]
pub fn poll_backoff(elapsed: Duration, cap: Duration) -> Duration {
    let interval = if elapsed < Duration::from_millis(2) {
        Duration::from_micros(100)
    } else if elapsed < Duration::from_millis(20) {
        Duration::from_millis(1)
    } else {
        cap
    };
    interval.min(cap)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::read_capped;

    #[test]
    fn read_capped_keeps_the_prefix_and_flags_the_overflow() {
        let overflow = AtomicBool::new(false);
        let kept = read_capped(&b"0123456789"[..], 4, &overflow).unwrap();
        assert_eq!(kept, b"0123");
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn read_capped_leaves_the_flag_clear_when_everything_fits() {
        let overflow = AtomicBool::new(false);
        let kept = read_capped(&b"0123"[..], 4, &overflow).unwrap();
        assert_eq!(kept, b"0123");
        assert!(!overflow.load(Ordering::Acquire));
    }

    #[test]
    fn read_capped_with_a_zero_maximum_keeps_nothing() {
        let overflow = AtomicBool::new(false);
        let kept = read_capped(&b"0"[..], 0, &overflow).unwrap();
        assert!(kept.is_empty());
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn interrupted_reads_retry_without_losing_the_prefix_or_overflow() {
        struct InterruptedReader<'a> {
            bytes: &'a [u8],
            interrupt: bool,
        }
        impl Read for InterruptedReader<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.interrupt = !self.interrupt;
                if self.interrupt {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let count = output.len().min(2);
                self.bytes.read(&mut output[..count])
            }
        }
        let overflow = AtomicBool::new(false);
        let reader = InterruptedReader {
            bytes: b"abcdef",
            interrupt: false,
        };
        assert_eq!(read_capped(reader, 5, &overflow).unwrap(), b"abcde");
        assert!(overflow.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    fn teardown_closes_pipes_held_by_descendants() {
        use super::{CapturedPipe, terminate_process_group};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let mut child = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 10 & printf ready; wait"])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pipe = CapturedPipe::spawn(
            child.stdout.take().unwrap(),
            128,
            Arc::new(AtomicBool::new(false)),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipe.bytes.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        terminate_process_group(child);
        while !pipe.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            pipe.is_finished(),
            "descendant must release its inherited pipe after teardown"
        );
        assert_eq!(pipe.finish().unwrap(), b"ready");
    }

    /// Runs `script` in its own process group with a captured stdout, and
    /// returns the child, the pipe and the first line it printed.
    #[cfg(unix)]
    fn captured(script: &str) -> (std::process::Child, super::CapturedPipe, String) {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pipe = super::CapturedPipe::spawn(
            child.stdout.take().unwrap(),
            128,
            Arc::new(AtomicBool::new(false)),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let line = loop {
            let bytes = pipe.bytes.lock().unwrap().clone();
            if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                break String::from_utf8(bytes[..end].to_vec()).unwrap();
            }
            assert!(Instant::now() < deadline, "the script never printed");
            std::thread::sleep(Duration::from_millis(1));
        };
        (child, pipe, line)
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_held_open_outside_the_childs_group_ends_with_the_child() {
        use std::time::{Duration, Instant};

        // `set -m` puts the lingering job in a group of its own: it holds
        // the pipe as an unrelated process that inherited it would.
        let (mut child, pipe, lingering) = captured("set -m; /bin/sleep 30 & echo $!; echo done");
        child.wait().unwrap();
        pipe.child_exited(Some(child.id()));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pipe.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::process::Command::new("/bin/kill")
            .arg(&lingering)
            .status();
        assert!(pipe.is_finished(), "a finished child's pipe kept the drain");
        assert_eq!(
            pipe.finish().unwrap(),
            format!("{lingering}\ndone\n").as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_held_by_the_childs_own_group_is_still_awaited() {
        use std::time::Duration;

        let (mut child, pipe, _) = captured("/bin/sleep 30 & echo started");
        let group = child.id();
        child.wait().unwrap();
        pipe.child_exited(Some(group));
        std::thread::sleep(super::PIPE_IDLE_AFTER_EXIT * 3);
        assert!(!pipe.is_finished(), "a descendant may still write");
        super::terminate_process_group(child);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !pipe.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pipe.is_finished());
        assert_eq!(pipe.finish().unwrap(), b"started\n");
    }
}
