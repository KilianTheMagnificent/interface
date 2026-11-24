//! Live cwd of a spawned terminal child, for pass-off panes that must start
//! where the source agent actually works (e.g. a CLI-created worktree).
//!
//! The workspace forbids `unsafe`, so the kernel's cwd record is read through
//! `lsof` rather than `proc_pidinfo` — the same shell-out shape the embedded
//! backend already uses (`/bin/ps`) for live process facts.
//!
//! The wait on that child is bounded: `lsof` stats every mount point while
//! building its view, so one wedged network share can hang it indefinitely.
//! Past the deadline the caller abandons the child and answers `None`, so its
//! worst case is the deadline plus one poll interval regardless of what the
//! filesystem is doing.

use std::path::PathBuf;

/// Best-effort: `None` on any failure, non-macOS, or an empty path.
#[cfg(target_os = "macos")]
pub(crate) fn live_process_cwd(pid: u32) -> Option<PathBuf> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Long enough for a healthy `lsof` many times over, short enough that a
    /// wedged mount cannot strand a pass-off.
    const DEADLINE: Duration = Duration::from_secs(2);
    const POLL: Duration = Duration::from_millis(25);

    let mut child = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        // stdout stays unread until exit, which is safe because `-d cwd -Fn`
        // answers in a few hundred bytes, far inside the pipe buffer. stderr
        // is discarded rather than piped: an undrained pipe on a host with
        // many broken mounts could stall a healthy call into a deadline miss.
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) | Err(_) => {
                // A process parked in an uninterruptible filesystem wait does
                // not die on SIGKILL — the signal stays pending until the
                // syscall returns — so reaping it here would re-open exactly
                // the block this deadline exists to prevent. Hand the reap to
                // a detached thread: no zombie, and no wait on the caller.
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }

    let mut stdout = String::new();
    child.stdout.as_mut()?.read_to_string(&mut stdout).ok()?;
    // `-Fn` prints one field per line behind its field letter; the cwd path is
    // the first `n` line of the single `cwd` file set.
    let path = stdout.lines().find_map(|line| line.strip_prefix('n'))?;
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// Best-effort: `None` on any failure, non-macOS, or an empty path.
#[cfg(not(target_os = "macos"))]
pub(crate) fn live_process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::live_process_cwd;

    #[test]
    fn reads_the_live_cwd_of_a_child_process() {
        let dir = std::env::temp_dir();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .current_dir(&dir)
            .spawn()
            .expect("spawn sleep");
        let cwd = live_process_cwd(child.id()).expect("live cwd");
        let canonical = std::fs::canonicalize(&dir).expect("canonical tmp");
        assert_eq!(
            std::fs::canonicalize(cwd).expect("canonical live"),
            canonical
        );
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn returns_none_for_a_dead_pid() {
        assert_eq!(live_process_cwd(u32::MAX - 7), None);
    }
}
