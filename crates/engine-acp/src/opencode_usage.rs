//! Read-only `OpenCode` Go allowance through the provider-owned auth boundary.
//!
//! Pinned upstream endpoint and hook contracts: `OpenCode` 1.18.29,
//! `packages/console/app/src/routes/zen/go/v1/usage.ts` and
//! `packages/opencode/src/provider/provider.ts`. Local token spend is not an
//! account quota and must never be divided by advertised plan limits.

use std::{path::PathBuf, process::Stdio, time::Duration};

use interface_contracts::engine::{EngineId, EngineUsage, UsageWindow};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    sync::{Mutex, watch},
    time::timeout,
};

use interface_engine::{VersionCache, child_output, worker::ProcessGroup};

use crate::opencode_worker::{OPENCODE_DISABLED, RuntimeDirectory};

const MAX_STDOUT: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(25);
const PREFIX: &str = "INTERFACE_OPENCODE_GO_USAGE_V1 ";
const TRANSPORT: &str = include_str!("opencode_usage_transport.mjs");
const READ_FAILED: &str =
    "OpenCode Go usage could not be read. Check your Go connection in OpenCode.";

/// A serialized, bounded account read. The CLI retains its own authentication;
/// no credentials, transcripts or provider configuration are read by Interface.
pub struct OpenCodeGoUsageReader {
    executable: PathBuf,
    runtime_root: PathBuf,
    serial: Mutex<()>,
    supported: VersionCache<bool>,
}

impl OpenCodeGoUsageReader {
    /// `runtime_root` must be an existing host-private directory.
    #[must_use]
    pub fn new(executable: PathBuf, runtime_root: PathBuf) -> Self {
        Self {
            executable,
            runtime_root,
            serial: Mutex::new(()),
            supported: VersionCache::default(),
        }
    }

    pub async fn read_usage(&self) -> EngineUsage {
        self.read_with_timeout(READ_TIMEOUT).await
    }

    async fn read_with_timeout(&self, deadline: Duration) -> EngineUsage {
        let Ok(_guard) = self.serial.try_lock() else {
            return unavailable(READ_FAILED);
        };
        timeout(deadline, self.read())
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| unavailable(READ_FAILED))
    }

    async fn read(&self) -> Result<EngineUsage, ()> {
        if !self.executable.is_absolute() || !self.runtime_root.is_absolute() {
            return Err(());
        }
        // Reuse the worker's private, read-only configuration setup, without
        // running a worker or loading user/project plugins or instructions.
        let runtime = RuntimeDirectory::create(&self.runtime_root.join("usage")).map_err(|_| ())?;
        let plugin = runtime.path().join("go-usage-v1.mjs");
        std::fs::write(&plugin, TRANSPORT).map_err(|_| ())?;
        let config = json!({"plugin": [plugin], "enabled_providers": ["opencode-go"],
            "mcp": {}, "instructions": [], "autoupdate": false, "share": "disabled",
            "formatter": false, "lsp": false, "snapshot": false})
        .to_string();
        let command = || probe_command(&self.executable, &runtime, &config);
        let supported = self
            .supported
            .get_or_probe(&self.executable, async {
                let (version, success) = capture(command().arg("--version")).await?;
                Ok(success && supported_version(&version))
            })
            .await?;
        if !supported {
            return Ok(unavailable(
                "OpenCode Go usage requires OpenCode 1.18.29 or a compatible newer 1.x release.",
            ));
        }
        let (bytes, success) = capture(command().args(["models", "opencode-go"])).await?;
        Ok(parse_response(&bytes, success))
    }
}

fn probe_command(
    executable: &std::path::Path,
    runtime: &RuntimeDirectory,
    config: &str,
) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(runtime.path())
        .env("PWD", runtime.path())
        .env("XDG_CONFIG_HOME", runtime.path().join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Authentication's XDG data directory stays with OpenCode. Config/plugin
    // overrides and pane/delegation transports do not enter this read process.
    for (name, _) in std::env::vars_os() {
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("OPENCODE_") || name.starts_with("INTERFACE_"))
        {
            command.env_remove(name);
        }
    }
    command
        .env("OPENCODE_TEST_HOME", runtime.path().join("home"))
        .env("OPENCODE_CONFIG_CONTENT", config);
    for name in OPENCODE_DISABLED {
        command.env(name, "true");
    }
    #[cfg(unix)]
    command.process_group(0);
    command
}

async fn capture(command: &mut Command) -> Result<(Vec<u8>, bool), ()> {
    let mut child = command.spawn().map_err(|_| ())?;
    let pid = child.id();
    let mut group = ProcessGroup::new(pid);
    let mut stdout = child.stdout.take().ok_or(())?;
    let mut bytes = Vec::new();
    // Ends when the command and its group are done, not when every copy of
    // its stdout closes; an over-long answer stops at once.
    let (tell, exited) = watch::channel(false);
    let (status, ()) = tokio::try_join!(
        child_output::wait_and_tell(&mut child, tell),
        child_output::read_until_done(&mut stdout, exited, pid, |chunk| {
            if bytes.len() + chunk.len() > MAX_STDOUT {
                return Err(std::io::Error::other("opencode output too large"));
            }
            bytes.extend_from_slice(chunk);
            Ok(())
        }),
    )
    .map_err(|_| ())?;
    group.disarm();
    Ok((bytes, status.success()))
}

fn supported_version(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let parts: Option<Vec<u64>> = text
        .trim()
        .split('.')
        .map(|part| part.parse().ok())
        .collect();
    parts.is_some_and(|v| v.len() == 3 && v[0] == 1 && (v[1], v[2]) >= (18, 29))
}

fn unavailable(note: &str) -> EngineUsage {
    EngineUsage {
        engine: EngineId::new("acp:opencode").expect("built-in OpenCode usage id"),
        available: false,
        plan: Some("Go".into()),
        windows: Vec::new(),
        cost_usd: None,
        note: Some(note.into()),
        account: None,
        account_label: None,
    }
}

fn parse_response(bytes: &[u8], success: bool) -> EngineUsage {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return unavailable(READ_FAILED);
    };
    let mut ready = false;
    let mut response = None;
    for line in text.lines().filter_map(|line| line.strip_prefix(PREFIX)) {
        let Ok(frame) = serde_json::from_str::<Value>(line) else {
            return unavailable(READ_FAILED);
        };
        if frame["version"].as_u64() != Some(1) {
            return unavailable(READ_FAILED);
        }
        match frame["type"].as_str() {
            Some("ready") if !ready && response.is_none() => ready = true,
            Some("response") if ready && response.is_none() => response = Some(frame),
            _ => return unavailable(READ_FAILED),
        }
    }
    let Some(frame) = response.filter(|_| success) else {
        return unavailable(READ_FAILED);
    };
    match frame["status"].as_u64() {
        Some(200) => parse_go_usage(&frame["body"]),
        Some(401) => unavailable(
            "OpenCode Go authentication was rejected. Reconnect with /connect in OpenCode.",
        ),
        Some(403) => unavailable("No active OpenCode Go subscription was reported."),
        _ => unavailable(READ_FAILED),
    }
}

/// Project provider percentages directly into the existing usage contract.
/// Missing or malformed windows stay unknown; local spend cannot fill them in.
#[must_use]
pub fn parse_go_usage(payload: &Value) -> EngineUsage {
    let mut result = unavailable("OpenCode Go did not report its usage limits.");
    let Some(usage) = payload.get("usage").and_then(Value::as_object) else {
        return result;
    };
    let windows: Vec<_> = [
        ("rolling", "5-hour"),
        ("weekly", "Weekly"),
        ("monthly", "Monthly"),
    ]
    .into_iter()
    .map(|(key, label)| {
        let window = usage.get(key);
        let recognized = window
            .and_then(|w| w.get("status"))
            .and_then(Value::as_str)
            .is_some_and(|status| status == "ok" || status == "rate-limited");
        let percent = window
            .and_then(|w| w.get("percent"))
            .and_then(Value::as_f64)
            .filter(|percent| recognized && percent.is_finite() && *percent >= 0.0);
        let resets_at = window
            .and_then(|w| w.get("resetsAt"))
            .and_then(Value::as_str)
            .filter(|at| percent.is_some() && valid_reset(at))
            .map(str::to_owned);
        UsageWindow {
            label: label.into(),
            utilization: percent,
            resets_at,
        }
    })
    .collect();
    if windows.iter().any(|window| window.utilization.is_some()) {
        result.available = true;
        result.note = Some("Subscription usage reported by OpenCode Go.".into());
        result.windows = windows;
    }
    result
}

fn valid_reset(at: &str) -> bool {
    // The endpoint emits JavaScript toISOString(), always millisecond UTC.
    let bytes = at.as_bytes();
    if bytes.len() != 24
        || !bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            19 => *b == b'.',
            23 => *b == b'Z',
            _ => b.is_ascii_digit(),
        })
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        at.get(range)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(-1)
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && number(11..13) < 24
        && number(14..16) < 60
        && number(17..19) < 60
        && interface_common::time::civil_from_days(interface_common::time::days_from_civil(
            year, month, day,
        )) == (year, month, day)
}

#[cfg(test)]
#[path = "opencode_usage_tests.rs"]
mod tests;
