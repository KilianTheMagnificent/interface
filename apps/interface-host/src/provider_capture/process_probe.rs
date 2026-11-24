//! Bounded `ps` probes of a pane process's identity: pid, process group,
//! controlling terminal and start time.

use super::{
    Capture, CapturedOutput, TerminalBackendError, TerminalProcessIdentity, invalid_registry,
    run_bounded,
};

pub(super) fn validate_live_process(
    process: &TerminalProcessIdentity,
) -> Result<(), TerminalBackendError> {
    #[cfg(target_os = "macos")]
    let columns = ["pid", "pgid", "tdev", "lstart"];
    #[cfg(not(target_os = "macos"))]
    let columns = ["pid", "pgid", "lstart"];
    let mut command = interface_terminal::PsProbe {
        columns: &columns,
        select: &["-p", &process.process_id.to_string()],
        utc: true,
        stderr: false,
    }
    .command();
    let output = bounded_process_observation(&mut command, std::time::Duration::from_millis(500))?;
    validate_live_process_output(&output, process)
}

pub(super) fn bounded_process_observation(
    command: &mut std::process::Command,
    deadline: std::time::Duration,
) -> Result<Vec<u8>, TerminalBackendError> {
    let capture = Capture {
        output: CapturedOutput::Stdout,
        max_bytes: 4096,
        overflow_fails: true,
    };
    match run_bounded(
        command,
        deadline,
        std::time::Duration::from_millis(5),
        capture,
    ) {
        Ok((status, Ok(output))) if status.success() => Ok(output),
        Ok(_) | Err(_) => Err(invalid_registry()),
    }
}

pub(super) fn validate_reaped_process_identity(
    process: &TerminalProcessIdentity,
) -> Result<(), TerminalBackendError> {
    if process.process_id == 0
        || process.process_group_id != process.process_id
        || normalize_whitespace(&process.process_start).is_empty()
    {
        return Err(invalid_registry());
    }
    #[cfg(target_os = "macos")]
    if process
        .terminal_device
        .as_deref()
        .is_none_or(|terminal| !valid_terminal_device(terminal))
    {
        return Err(invalid_registry());
    }
    Ok(())
}

pub(super) fn validate_live_process_output(
    output: &[u8],
    process: &TerminalProcessIdentity,
) -> Result<(), TerminalBackendError> {
    if process.process_id == 0 || process.process_group_id != process.process_id {
        return Err(invalid_registry());
    }
    let output = std::str::from_utf8(output).map_err(|_| invalid_registry())?;
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or_else(invalid_registry)?;
    if lines.next().is_some() {
        return Err(invalid_registry());
    }
    let mut fields = line.split_whitespace();
    let observed_pid = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(invalid_registry)?;
    let observed_process_group = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(invalid_registry)?;

    #[cfg(target_os = "macos")]
    {
        let expected_terminal = process
            .terminal_device
            .as_deref()
            .filter(|value| valid_terminal_device(value))
            .ok_or_else(invalid_registry)?;
        let observed_terminal = fields
            .next()
            .filter(|value| valid_terminal_device(value))
            .ok_or_else(invalid_registry)?;
        if observed_terminal != expected_terminal {
            return Err(invalid_registry());
        }
    }

    let observed_start = fields.collect::<Vec<_>>().join(" ");
    let expected_start = normalize_whitespace(&process.process_start);
    if observed_pid != process.process_id
        || observed_process_group != process.process_group_id
        || observed_process_group != observed_pid
        || expected_start.is_empty()
        || observed_start != expected_start
    {
        return Err(invalid_registry());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn valid_terminal_device(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| byte == b'?')
}

pub(super) fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
