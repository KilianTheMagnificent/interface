//! Read-only server discovery scoped to a pinned terminal process tree.
use super::{TerminalError, TerminalService, current_entry_mut, lock};
use crate::proc_ancestry::{owned_ancestor, process_parents};
use interface_contracts::{
    terminal::TerminalGenerationRequest, workspace_tools::TerminalServerLink,
};
use std::{
    collections::{BTreeSet, HashMap},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

impl TerminalService {
    /// # Errors
    /// Refuses stale generations, unverified process identity and unavailable OS observations.
    pub fn server_links(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<Vec<TerminalServerLink>, TerminalError> {
        let control = {
            let mut state = lock(&self.shared.state);
            let entry = current_entry_mut(&mut state, request.id, request.generation)?;
            Arc::clone(&entry.resources.control)
        };
        let process = control
            .process_identity()
            .ok_or_else(|| unavailable("Terminal process identity is unavailable."))?;
        if control.process_identity_is_live(&process).ok() != Some(true) {
            return Err(unavailable("Terminal process is no longer live."));
        }
        let parents =
            process_parents().ok_or_else(|| unavailable("Could not inspect the process tree."))?;
        let mut pids: Vec<u32> = parents
            .keys()
            .copied()
            .filter(|pid| {
                owned_ancestor(*pid, &parents, |parent| parent == process.process_id).is_some()
            })
            .collect();
        pids.sort_unstable();
        if pids.len() > 256 {
            return Err(unavailable("Process tree exceeds the inspection limit."));
        }
        let output = read_listeners(&pids)
            .ok_or_else(|| unavailable("Server discovery timed out or was unavailable."))?;
        let current =
            process_parents().ok_or_else(|| unavailable("Could not recheck process ownership."))?;
        if self.generation_has_settled(request)
            || control.process_identity_is_live(&process).ok() != Some(true)
        {
            return Err(unavailable("Terminal changed during server discovery."));
        }
        Ok(parse_listeners(&output, &current, process.process_id))
    }
}
fn unavailable(message: &str) -> TerminalError {
    TerminalError::Backend(message.into())
}
fn read_listeners(pids: &[u32]) -> Option<String> {
    if pids.is_empty() {
        return Some(String::new());
    }
    let mut child = Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fpn",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let overflow = Arc::new(AtomicBool::new(false));
    let pipe = interface_common::process::CapturedPipe::spawn(
        child.stdout.take()?,
        128 * 1024,
        overflow.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let observed = child.try_wait();
        if let Ok(Some(_)) = observed {
            pipe.child_exited(None);
        }
        match observed {
            Ok(Some(status)) if pipe.is_finished() => {
                return if overflow.load(Ordering::Relaxed) || !matches!(status.code(), Some(0 | 1))
                {
                    None
                } else {
                    String::from_utf8(pipe.finish().ok()?).ok()
                };
            }
            Ok(_) if Instant::now() < deadline && !overflow.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return None;
            }
        }
    }
}
fn parse_listeners(
    output: &str,
    parents: &HashMap<u32, u32>,
    owner: u32,
) -> Vec<TerminalServerLink> {
    let mut pid = None;
    let mut seen = BTreeSet::new();
    let mut links = Vec::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        }
        let Some(pid) = pid
            .filter(|pid| owned_ancestor(*pid, parents, |candidate| candidate == owner).is_some())
        else {
            continue;
        };
        let Some(address) = line.strip_prefix('n') else {
            continue;
        };
        let Some((host, port)) = address.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        if port == 0
            || !matches!(host, "*" | "127.0.0.1" | "localhost" | "[::1]" | "[::]")
            || !seen.insert((pid, port))
        {
            continue;
        }
        links.push(TerminalServerLink {
            process_id: pid,
            url: format!(
                "http://{}:{port}",
                if matches!(host, "[::1]" | "[::]") {
                    "[::1]"
                } else {
                    "127.0.0.1"
                }
            ),
            readiness: "Listening; application health not verified".into(),
        });
    }
    links
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_other_processes_remote_binds_and_stale_ancestry() {
        let parents = HashMap::from([(10, 1), (11, 10), (20, 1)]);
        let links = parse_listeners(
            "p11\nn127.0.0.1:3000\nn127.0.0.1:3000\np20\nn*:4000\np11\nn192.0.2.1:5000\nn[::1]:6000\n",
            &parents,
            10,
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].process_id, 11);
        assert_eq!(links[1].url, "http://[::1]:6000");
        assert!(parse_listeners("p11\nn*:3000\n", &HashMap::from([(11, 20)]), 10).is_empty());
    }
}
