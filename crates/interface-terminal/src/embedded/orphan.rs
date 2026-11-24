//! Settle a stopped captured PTY group after its supervisor disappears.
//! No CONT or provider input is sent; active, leaderless and recycled groups decline.
use super::{TerminalBackendError, TerminalProcessIdentity};

/// Settle an exact, owner-private capture's stopped orphan process group.
/// Call only while exclusively owning the capture store and after validating
/// its saved owner. `false` means no signal was sent. Unsupported platforms decline.
/// # Errors
/// Returns an error on failed observation or unproven teardown.
pub fn settle_stopped_orphan_process_group(
    process: &TerminalProcessIdentity,
) -> Result<bool, TerminalBackendError> {
    #[cfg(target_os = "macos")]
    {
        settle(process)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = process;
        Ok(false)
    }
}

#[cfg(target_os = "macos")]
use super::{
    Duration, Instant, Signal, captured_process_group_is_live, raw_signal_process_group, thread,
};

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    pid: u32,
    parent: u32,
    group: u32,
    terminal: String,
    state: String,
    uid: u32,
    start: String,
}

#[cfg(target_os = "macos")]
fn observations() -> Result<Vec<Observation>, TerminalBackendError> {
    let output = crate::PsProbe {
        columns: &["pid", "ppid", "pgid", "tdev", "stat", "uid", "lstart"],
        select: &["-ax"],
        utc: true,
        stderr: false,
    }
    .output()
    .map_err(|_| failed())?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err(failed());
    }
    std::str::from_utf8(&output.stdout)
        .map_err(|_| failed())?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_observation)
        .collect()
}

#[cfg(target_os = "macos")]
fn parse_observation(line: &str) -> Result<Observation, TerminalBackendError> {
    let mut fields = line.split_whitespace();
    let pid = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(failed)?;
    let parent = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(failed)?;
    let group = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(failed)?;
    let terminal = fields.next().ok_or_else(failed)?.to_owned();
    let state = fields.next().ok_or_else(failed)?.to_owned();
    // `ps` prints a uid as a signed number: `nobody` is -2, and dhcp6d runs
    // as nobody while a virtual machine or Internet Sharing is on. One such
    // row must not fail the whole listing.
    let uid = fields
        .next()
        .and_then(|s| {
            s.parse::<u32>()
                .ok()
                .or_else(|| s.parse::<i32>().ok().map(i32::cast_unsigned))
        })
        .ok_or_else(failed)?;
    let start = fields.collect::<Vec<_>>().join(" ");
    if start.is_empty() {
        return Err(failed());
    }
    Ok(Observation {
        pid,
        parent,
        group,
        terminal,
        state,
        uid,
        start,
    })
}

#[cfg(target_os = "macos")]
fn stopped_orphan(expected: &TerminalProcessIdentity, observed: &[Observation], uid: u32) -> bool {
    let Some(leader) = observed.iter().find(|row| row.pid == expected.process_id) else {
        return false;
    };
    leader.parent == 1
        && leader.group == expected.process_group_id
        && leader.uid == uid
        && leader.start == expected.process_start
        && Some(leader.terminal.as_str()) == expected.terminal_device.as_deref()
        && leader.state.starts_with('T')
        && observed
            .iter()
            .filter(|row| row.group == expected.process_group_id)
            .all(|row| {
                row.uid == uid
                    && row.terminal == leader.terminal
                    && (row.state.starts_with('T') || row.state.starts_with('Z'))
            })
}

#[cfg(target_os = "macos")]
fn settle(process: &TerminalProcessIdentity) -> Result<bool, TerminalBackendError> {
    if !captured_process_group_is_live(process)? {
        return Ok(true);
    }
    let uid = interface_common::unix::effective_uid().map_err(|_| failed())?;
    if !stopped_orphan(process, &observations()?, uid) {
        return Ok(false);
    }
    // Recheck just before the only signal. A stopped leader cannot replace
    // itself; a concurrent external resume, reparent or reuse declines.
    if !stopped_orphan(process, &observations()?, uid) {
        return Ok(false);
    }
    if !raw_signal_process_group(process.process_group_id, Signal::KILL) {
        return if captured_process_group_is_live(process)? {
            Err(failed())
        } else {
            Ok(true)
        };
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !captured_process_group_is_live(process)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(failed());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn failed() -> TerminalBackendError {
    TerminalBackendError::new("stopped orphan terminal ownership or teardown could not be verified")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    fn identity() -> TerminalProcessIdentity {
        TerminalProcessIdentity {
            process_id: 42,
            process_group_id: 42,
            terminal_device: Some("16/4".into()),
            process_start: "Tue Sep 29 03:41:19 2026".into(),
        }
    }
    #[test]
    fn a_row_owned_by_nobody_does_not_fail_the_listing() {
        let nobody = parse_observation("46253 1 46253 ?? Ss -2 Tue Nov 24 15:18:26 2026").unwrap();
        assert_eq!(nobody.uid, u32::MAX - 1);
        let leader = parse_observation("42 1 42 16/4 Ts+ 501 Tue Sep 29 03:41:19 2026").unwrap();
        assert!(stopped_orphan(&identity(), &[nobody, leader], 501));
        assert!(parse_observation("46253 1 46253 ?? Ss nobody Tue Nov 24 15:18:26 2026").is_err());
    }

    #[test]
    fn orphan_recovery_requires_exact_stopped_owned_group_and_absent_parent() {
        let leader = parse_observation("42 1 42 16/4 Ts+ 501 Tue Sep 29 03:41:19 2026").unwrap();
        assert!(stopped_orphan(
            &identity(),
            std::slice::from_ref(&leader),
            501
        ));
        for field in ["parent", "group", "uid", "start", "terminal", "state"] {
            let mut changed = leader.clone();
            match field {
                "parent" => changed.parent = 99,
                "group" => changed.group = 99,
                "uid" => changed.uid = 502,
                "start" => changed.start.push('0'),
                "terminal" => changed.terminal = "16/5".into(),
                "state" => changed.state = "S+".into(),
                _ => unreachable!(),
            }
            assert!(!stopped_orphan(&identity(), &[changed], 501), "{field}");
        }
        let mut child = leader.clone();
        child.pid = 43;
        child.parent = 42;
        child.state = "S".into();
        assert!(!stopped_orphan(
            &identity(),
            &[leader.clone(), child.clone()],
            501
        ));
        child.state = "T".into();
        assert!(stopped_orphan(&identity(), &[leader, child], 501));
        assert!(!stopped_orphan(&identity(), &[], 501));
        assert!(parse_observation("42 1 42 16/4 Ts+ 501").is_err());
    }
}
