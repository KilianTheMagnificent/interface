//! The hook-side pieces the Claude and Codex `SessionStart` helpers share:
//! which sources a receipt may name, and the hook process's own identity and
//! its ancestors'. Provider-neutral, so neither hook imports the other.

use std::io;

use interface_terminal::TerminalProviderSessionTransition;

use crate::capture_generation::HookProcessIdentity;

/// The `SessionStart` sources a receipt may record, and what each does to the
/// pane's binding.
pub(crate) fn transition_for(source: &str) -> Option<TerminalProviderSessionTransition> {
    Some(match source {
        "startup" => TerminalProviderSessionTransition::Startup,
        "resume" => TerminalProviderSessionTransition::Resume,
        "clear" => TerminalProviderSessionTransition::Clear,
        "compact" => TerminalProviderSessionTransition::Compact,
        "fork" => TerminalProviderSessionTransition::Fork,
        _ => return None,
    })
}

/// Whether `source` is a `SessionStart` source a receipt may record.
pub(crate) fn is_session_source(source: &str) -> bool {
    transition_for(source).is_some()
}

pub(crate) fn parent_process_id() -> io::Result<u32> {
    // getppid(2) through std: the helper already knows its parent, so there
    // is no need to spawn ps for it. Foreign pids still go through ps below.
    Some(std::os::unix::process::parent_id())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::other("Claude terminal session hook parent was unavailable"))
}

/// A foreign process's parent through `ps`. The hooks read parents from
/// [`hook_process_lineage`]; this cross-checks `getppid` in the tests.
#[cfg(test)]
pub(crate) fn parent_process_id_of(process_id: u32) -> io::Result<u32> {
    let output = interface_terminal::PsProbe {
        columns: &["ppid"],
        select: &["-p", &process_id.to_string()],
        utc: false,
        stderr: false,
    }
    .output()?;
    if !output.status.success() || output.stdout.len() > 64 {
        return Err(io::Error::other(
            "Claude terminal session hook parent was unavailable",
        ));
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::other("Claude terminal session hook parent was unavailable"))
}

pub(crate) fn current_hook_process_identity() -> io::Result<HookProcessIdentity> {
    // This process is alive by construction, so the kill(0) probe is skipped.
    read_process_identity(std::process::id(), true)?
        .map(|(identity, _)| identity)
        .ok_or_else(|| {
            io::Error::other("Claude terminal session hook process identity was unavailable")
        })
}

pub(crate) fn hook_process_identity(process_id: u32) -> io::Result<Option<HookProcessIdentity>> {
    Ok(hook_process_lineage(process_id)?.map(|(identity, _)| identity))
}

/// A process's identity and, from the same `ps` listing, its parent's pid
/// (`None` when it has none to name).
pub(crate) fn hook_process_lineage(
    process_id: u32,
) -> io::Result<Option<(HookProcessIdentity, Option<u32>)>> {
    if process_id == 0 || !process_exists(process_id)? {
        return Ok(None);
    }
    read_process_identity(process_id, false)
}

fn read_process_identity(
    process_id: u32,
    is_current_process: bool,
) -> io::Result<Option<(HookProcessIdentity, Option<u32>)>> {
    let output = interface_terminal::PsProbe {
        columns: &["pid", "ppid", "lstart"],
        select: &["-p", &process_id.to_string()],
        utc: true,
        stderr: false,
    }
    .output()?;
    if output.stdout.len() > 256 {
        return Err(io::Error::other(
            "Claude terminal session hook process identity was oversized",
        ));
    }
    if !output.status.success() {
        // The process may have exited between kill(0) and ps. Re-probe so
        // an EPERM or sandbox failure can never masquerade as definitive
        // absence and authorize marker reclamation. The current process
        // cannot have exited, so its failed probe is always ambiguous.
        return if is_current_process || process_exists(process_id)? {
            Err(io::Error::other(
                "Claude terminal session hook process probe was ambiguous",
            ))
        } else {
            Ok(None)
        };
    }
    let output = std::str::from_utf8(&output.stdout).map_err(io::Error::other)?;
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or_else(|| {
        io::Error::other("Claude terminal session hook process identity was empty")
    })?;
    if lines.next().is_some() {
        return Err(io::Error::other(
            "Claude terminal session hook process identity was ambiguous",
        ));
    }
    let mut fields = line.split_whitespace();
    let observed = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|observed| *observed == process_id)
        .ok_or_else(|| io::Error::other("Claude terminal session hook process identity changed"))?;
    let parent = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| io::Error::other("Claude terminal session hook process identity changed"))?;
    let process_start = fields.collect::<Vec<_>>().join(" ");
    if process_start.is_empty()
        || process_start.len() > 128
        || !process_start.is_ascii()
        || process_start.chars().any(char::is_control)
    {
        return Err(io::Error::other(
            "Claude terminal session hook process start was invalid",
        ));
    }
    Ok(Some((
        HookProcessIdentity {
            process_id: observed,
            process_start,
        },
        (parent > 0).then_some(parent),
    )))
}

fn process_exists(process_id: u32) -> io::Result<bool> {
    // kill(2) signal 0, not a /bin/kill fork whose stderr has to be parsed.
    interface_terminal::process_exists(process_id)
}
