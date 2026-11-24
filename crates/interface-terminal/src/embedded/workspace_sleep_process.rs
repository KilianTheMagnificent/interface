//! Exact-identity process suspension. Only groups captured from this PTY's tree
//! are resumed; switching workspaces never launches or replays a command.
#[cfg(unix)]
use super::{
    Ordering, ProcessGroupReceipt, Signal, lock, process_snapshot, signal_managed_process_group,
};
use super::{SystemTerminalControl, TerminalBackendError};

#[cfg(unix)]
type ProcessTable = std::collections::HashMap<u32, (u32, u32)>;

/// One `ps` listing of every process: pid → (parent, group).
#[cfg(unix)]
fn process_table() -> std::io::Result<ProcessTable> {
    crate::proc_ancestry::process_table()
        .ok_or_else(|| std::io::Error::other("terminal process tree is unavailable"))
}

/// Whether any process in `table` is a direct child of the PTY's root.
#[cfg(unix)]
fn root_has_child(control: &SystemTerminalControl, table: &ProcessTable) -> bool {
    let root = control.process_group_receipt.process_id;
    table.values().any(|(parent, _)| *parent == root)
}

/// The root's group, then every group led by one of its descendants, in
/// group-id order, from one `ps` listing. Only the leader of a group not yet
/// seen gets its exact identity snapshot, so the cost is one spawn per group
/// rather than per process. A leader that has vanished is an error, which
/// declines the suspension.
#[cfg(unix)]
fn descendants(
    control: &SystemTerminalControl,
    table: &ProcessTable,
) -> std::io::Result<Vec<ProcessGroupReceipt>> {
    let root = control.process_group_receipt.process_id;
    let parents = table
        .iter()
        .map(|(&pid, &(parent, _))| (pid, parent))
        .collect::<std::collections::HashMap<_, _>>();
    let mut groups = std::collections::BTreeMap::new();
    groups.insert(
        control.process_group_receipt.process_group_id,
        control.process_group_receipt.clone(),
    );
    for (&pid, &(_, group)) in table {
        if pid == root
            || groups.contains_key(&group)
            || crate::proc_ancestry::owned_ancestor(pid, &parents, |id| id == root).is_none()
        {
            continue;
        }
        let leader = process_snapshot(group)?
            .ok_or_else(|| std::io::Error::other("terminal child group has no identity"))?;
        if crate::proc_ancestry::owned_ancestor(leader.process_id, &parents, |id| id == root)
            .is_none()
        {
            return Err(std::io::Error::other(
                "terminal child group ownership changed",
            ));
        }
        groups.insert(leader.process_group_id, leader);
    }
    let mut result = vec![control.process_group_receipt.clone()];
    result.extend(
        groups
            .into_values()
            .filter(|group| group.process_id != root),
    );
    Ok(result)
}

pub(super) fn suspend(
    control: &SystemTerminalControl,
    agent_idle: bool,
) -> Result<bool, TerminalBackendError> {
    #[cfg(unix)]
    {
        let terminated = lock(&control.terminated);
        if *terminated || control.exited.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut stopped = lock(&control.sleeping_groups);
        if !stopped.is_empty() {
            return Ok(true);
        }
        let result = (|| -> std::io::Result<bool> {
            // Nothing is signalled before the first pass, so the precheck and
            // that pass share one listing; every later pass reads afresh to
            // catch a child spawned while groups were being stopped.
            let mut table = process_table()?;
            if !agent_idle && root_has_child(control, &table) {
                // Silence is not completion: foreground AND background jobs keep
                // a plain shell awake, including sleep, redirected builds and servers.
                return Ok(false);
            }
            let foreground = if agent_idle {
                lock(&control.master)
                    .as_ref()
                    .and_then(|master| master.process_group_leader())
                    .and_then(|group| u32::try_from(group).ok())
            } else {
                None
            };
            let foreign = |group: &ProcessGroupReceipt| {
                group.process_group_id != control.process_group_receipt.process_group_id
                    && Some(group.process_group_id) != foreground
            };
            let mut groups = descendants(control, &table)?;
            if agent_idle && groups.iter().any(foreign) {
                // A separately grouped background job/server can outlive
                // the agent's turn. Leave the whole terminal awake.
                return Ok(false);
            }
            // Stop parents before descendants, then rescan to catch a child
            // spawned during the first pass. Bound churn and roll back on doubt.
            for pass in 0..3 {
                if pass > 0 {
                    table = process_table()?;
                    groups = descendants(control, &table)?;
                }
                let mut added = false;
                for group in std::mem::take(&mut groups) {
                    if agent_idle && foreign(&group) {
                        return Ok(false);
                    }
                    if stopped.contains(&group) {
                        continue;
                    }
                    if !signal_managed_process_group(&group, Signal::STOP)? {
                        return Ok(false);
                    }
                    stopped.push(group);
                    added = true;
                }
                if !added {
                    return Ok(agent_idle || !root_has_child(control, &table));
                }
            }
            Ok(false)
        })();
        if !matches!(result, Ok(true)) {
            resume_groups(&mut stopped)
                .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        }
        result.map_err(|error| TerminalBackendError::new(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = (control, agent_idle);
        Ok(false)
    }
}

#[cfg(unix)]
fn resume_groups(stopped: &mut Vec<ProcessGroupReceipt>) -> std::io::Result<()> {
    // Child jobs become runnable before their waiting shell. Keep every failed
    // receipt so a later wake or teardown can recover, without signalling a reused PID.
    let mut failure = None;
    for index in (0..stopped.len()).rev() {
        match signal_managed_process_group(&stopped[index], Signal::CONT) {
            Ok(_) => {
                stopped.remove(index);
            }
            Err(error) => {
                failure = Some(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

pub(super) fn resume(control: &SystemTerminalControl) -> Result<(), TerminalBackendError> {
    #[cfg(unix)]
    {
        resume_groups(&mut lock(&control.sleeping_groups))
            .map_err(|error| TerminalBackendError::new(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = control;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use crate::{
        ResolvedTerminalLaunch, SpawnedTerminal, SystemTerminalPtyBackend, TerminalPtyBackend,
        TerminalSpawnSpec,
    };
    use interface_contracts::terminal::TerminalSize;
    use std::{
        io::Read,
        path::Path,
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    // The PTY master is drained for the terminal's whole life: macOS holds an
    // exiting session leader in its tty close until the output it wrote is
    // read, so an undrained master can leave `wait()` in Drop blocked forever.
    struct TestPty(SpawnedTerminal);
    impl Drop for TestPty {
        fn drop(&mut self) {
            let _ = self.0.control.resume();
            let _ = self.0.control.terminate_tree();
            let _ = self.0.waiter.wait();
        }
    }
    fn spawn(command: &str) -> TestPty {
        let mut terminal = SystemTerminalPtyBackend::with_shell(Path::new("/bin/sh"))
            .unwrap()
            .spawn(&TerminalSpawnSpec {
                cwd: std::env::temp_dir(),
                size: TerminalSize {
                    columns: 80,
                    rows: 24,
                },
                launch: ResolvedTerminalLaunch::Program {
                    executable: "/bin/sh".into(),
                    arguments: vec!["-c".into(), command.into()],
                    environment: vec![],
                },
            })
            .unwrap();
        let mut reader = std::mem::replace(&mut terminal.reader, Box::new(std::io::empty()));
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while matches!(reader.read(&mut buffer), Ok(count) if count > 0) {}
        });
        TestPty(terminal)
    }
    fn stopped(pid: u32) -> bool {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .starts_with('T')
    }
    fn until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "test process did not reach expected state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn workspace_sleep_real_process_stops_and_resumes_the_same_identity() {
        let terminal = spawn("while :; do :; done");
        let identity = terminal.0.control.process_identity().unwrap();
        assert!(terminal.0.control.suspend_idle(true).unwrap());
        until(|| stopped(identity.process_id));
        assert!(
            terminal
                .0
                .control
                .process_identity_is_live(&identity)
                .unwrap()
        );
        terminal.0.control.resume().unwrap();
        until(|| !stopped(identity.process_id));
        assert_eq!(terminal.0.control.process_identity().unwrap(), identity);
        assert!(terminal.0.control.suspend_idle(true).unwrap());
        // Drop also exercises closing a sleeping generation without stranding it.
    }

    #[test]
    fn workspace_sleep_real_silent_foreground_and_background_jobs_are_not_stopped() {
        for script in ["sleep 1; read line", "sleep 1 & read line"] {
            let terminal = spawn(script);
            let pid = terminal.0.control.process_id().unwrap();
            until(|| {
                crate::proc_ancestry::process_parents()
                    .unwrap()
                    .values()
                    .any(|parent| *parent == pid)
            });
            assert!(!terminal.0.control.suspend_idle(false).unwrap());
            assert!(!stopped(pid));
            until(|| {
                !crate::proc_ancestry::process_parents()
                    .unwrap()
                    .values()
                    .any(|parent| *parent == pid)
            });
            assert!(terminal.0.control.suspend_idle(false).unwrap());
            until(|| stopped(pid));
            terminal.0.control.resume().unwrap();
        }
    }
    #[test]
    fn workspace_sleep_idle_agent_with_a_separately_grouped_job_declines() {
        let terminal = spawn("set -m; sleep 5 & read line");
        let root = terminal.0.control.process_id().unwrap();
        until(|| {
            crate::proc_ancestry::process_parents()
                .unwrap()
                .values()
                .any(|parent| *parent == root)
        });
        assert!(!terminal.0.control.suspend_idle(true).unwrap());
        assert!(!stopped(root));
    }

    #[test]
    fn workspace_sleep_nested_job_resumes_in_the_foreground() {
        // The job takes the terminal's foreground only after the newline. A
        // job that wins the race with spawn's identity check is accepted too;
        // `spawn_identity_accepts_a_job_that_already_took_the_foreground`
        // holds that order.
        let terminal = spawn("set -m; read line; /bin/sh -c 'while :; do :; done'");
        terminal.0.input.write(b"\n").unwrap();
        let root = terminal.0.control.process_id().unwrap();
        until(|| {
            terminal
                .0
                .control
                .foreground_process_identity()
                .unwrap()
                .is_some_and(|process| process.process_id != root)
        });
        let foreground = terminal
            .0
            .control
            .foreground_process_identity()
            .unwrap()
            .unwrap();
        assert!(terminal.0.control.suspend_idle(true).unwrap());
        until(|| stopped(root) && stopped(foreground.process_id));
        terminal.0.control.resume().unwrap();
        until(|| !stopped(root) && !stopped(foreground.process_id));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            terminal.0.control.foreground_process_identity().unwrap(),
            Some(foreground)
        );
    }
}
