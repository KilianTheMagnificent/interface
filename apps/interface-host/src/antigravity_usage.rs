use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::DateTime;
use interface_contracts::engine::{EngineId, EngineUsage, UsageWindow};
use interface_engine::VersionCache;
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex, time::timeout};

use crate::session_management::SupplementalUsageReader;

const MINIMUM_SAFE_VERSION: (u64, u64, u64) = (1, 1, 11);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_STDOUT_BYTES: usize = 1024 * 1024;
/// How long a failed read suppresses another spawn. A signed-out or offline
/// CLI fails every read the same way, and the usage panel asks again every
/// few minutes; without this floor each ask starts the full CLI again.
const FAILURE_BACKOFF: Duration = Duration::from_secs(600);
/// The script every browser-launch route in the child resolves to.
const NO_BROWSER_SCRIPT: &str = "#!/bin/sh\nexit 0\n";

/// Reads quota through Antigravity's own non-turn `/usage` command.
///
/// This read is background and unattended, so it must never interact. When
/// silent auth fails (signed out, expired token, offline) `agy --print`
/// falls back to interactive OAuth: it runs `open` with a Google sign-in URL
/// and waits for a code. Pointing a usage poll at that path opened a fresh
/// Chrome sign-in tab every few minutes. The child's `PATH` therefore leads
/// with a host-private directory whose `open`/`xdg-open` are no-ops, and
/// `BROWSER` names the same no-op; if that directory cannot be prepared the
/// read fails closed without spawning. Signing in stays available where it
/// belongs: an Antigravity terminal the user opened.
pub struct AntigravityUsageReader {
    executable: PathBuf,
    runtime_root: PathBuf,
    backoff: Duration,
    /// Serializes reads; holds when the last one failed.
    state: Mutex<Option<Instant>>,
    /// Whether the executable's `--version` is one whose `/usage` cannot be
    /// taken as a prompt, kept while the path resolves to the same file.
    supported: VersionCache<bool>,
}

impl AntigravityUsageReader {
    /// `runtime_root` must be an existing host-private directory.
    #[must_use]
    pub fn new(executable: PathBuf, runtime_root: PathBuf) -> Self {
        Self {
            executable,
            runtime_root,
            backoff: FAILURE_BACKOFF,
            state: Mutex::new(None),
            supported: VersionCache::default(),
        }
    }

    /// The directory whose browser openers do nothing, prepared for use as
    /// the head of the child's `PATH`.
    fn no_browser_directory(&self) -> Result<PathBuf, ()> {
        let directory = self.runtime_root.join("antigravity-no-browser");
        std::fs::create_dir_all(&directory).map_err(|_| ())?;
        for name in ["open", "xdg-open"] {
            let path = directory.join(name);
            if !std::fs::read(&path).is_ok_and(|bytes| bytes == NO_BROWSER_SCRIPT.as_bytes()) {
                std::fs::write(&path, NO_BROWSER_SCRIPT).map_err(|_| ())?;
            }
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(|_| ())?;
        }
        Ok(directory)
    }

    async fn invoke(&self, arguments: &[&str]) -> Result<Vec<u8>, ()> {
        let no_browser = self.no_browser_directory()?;
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(no_browser.clone()).chain(std::env::split_paths(&inherited)),
        )
        .map_err(|_| ())?;
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .env("PATH", path)
            .env("BROWSER", no_browser.join("open"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = ProcessGroupChild(command.spawn().map_err(|_| ())?);
        let child = &mut child.0;
        let stdout = child.stdout.take().ok_or(())?;
        let mut bytes = Vec::new();
        stdout
            .take((MAX_STDOUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ())?;
        let status = child.wait().await.map_err(|_| ())?;
        if !status.success() || bytes.len() > MAX_STDOUT_BYTES {
            return Err(());
        }
        Ok(bytes)
    }

    async fn read(&self) -> Result<EngineUsage, ()> {
        let mut last_failure = self.state.try_lock().map_err(|_| ())?;
        if last_failure.is_some_and(|failed_at| failed_at.elapsed() < self.backoff) {
            return Err(());
        }
        let outcome = timeout(PROCESS_TIMEOUT, async {
            let supported = self
                .supported
                .get_or_probe(&self.executable, async {
                    let version = self.invoke(&["--version"]).await?;
                    Ok(supported_version(&version))
                })
                .await?;
            if !supported {
                return Err(());
            }
            let payload = self
                .invoke(&[
                    "--print",
                    "/usage",
                    "--output-format",
                    "json",
                    "--print-timeout",
                    "20s",
                ])
                .await?;
            parse_usage(&payload)
        })
        .await
        .map_err(|_| ())?;
        *last_failure = outcome.is_err().then(Instant::now);
        outcome
    }
}

/// A child leading its own process group. If it is dropped before being
/// reaped (a timeout or read error), the whole group is killed, so helpers
/// the CLI spawned cannot outlive the usage read. `kill_on_drop` then kills
/// and reaps the leader. While the leader is unreaped its pid cannot be
/// reused, so the group id still names only this command's processes.
struct ProcessGroupChild(tokio::process::Child);

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if let Some(leader) = self.0.id() {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{leader}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

#[async_trait]
impl SupplementalUsageReader for AntigravityUsageReader {
    fn engine(&self) -> EngineId {
        gemini_engine()
    }

    async fn read_usage(&self) -> EngineUsage {
        self.read().await.unwrap_or_else(|()| unavailable_usage())
    }
}

fn gemini_engine() -> EngineId {
    EngineId::new("acp:gemini").expect("built-in Antigravity engine id")
}

fn unavailable_usage() -> EngineUsage {
    EngineUsage {
        engine: gemini_engine(),
        available: false,
        plan: None,
        windows: Vec::new(),
        cost_usd: None,
        note: Some(
            "Antigravity could not report Gemini quota. Open an Antigravity terminal to sign in."
                .into(),
        ),
        account: None,
        account_label: None,
    }
}

fn supported_version(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let version = text.trim();
    if version.is_empty() || version.contains(['-', '+']) {
        return false;
    }
    let mut parts = version.split('.');
    let Some(major) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    parts.next().is_none()
        && major == MINIMUM_SAFE_VERSION.0
        && (major, minor, patch) >= MINIMUM_SAFE_VERSION
}

fn zero_counter(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_u64) == Some(0)
}

fn parse_usage(bytes: &[u8]) -> Result<EngineUsage, ()> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if root.get("status").and_then(Value::as_str) != Some("SUCCESS")
        || root.get("conversation_id").and_then(Value::as_str) != Some("")
        || root.get("num_turns").and_then(Value::as_u64) != Some(0)
    {
        return Err(());
    }
    let counters = root.get("usage").ok_or(())?;
    for field in [
        "input_tokens",
        "output_tokens",
        "thinking_tokens",
        "cache_read_tokens",
        "total_tokens",
    ] {
        if !zero_counter(counters, field) {
            return Err(());
        }
    }
    let command = root.get("command").and_then(Value::as_object).ok_or(())?;
    if command.get("name").and_then(Value::as_str) != Some("usage") {
        return Err(());
    }
    let groups = command
        .get("data")
        .and_then(|data| data.get("groups"))
        .and_then(Value::as_array)
        .ok_or(())?;
    let mut windows = Vec::new();
    for (id, label) in [("gemini-5h", "5-hour"), ("gemini-weekly", "Weekly")] {
        let matches = groups
            .iter()
            .filter_map(|group| group.get("buckets").and_then(Value::as_array))
            .flatten()
            .filter(|bucket| bucket.get("id").and_then(Value::as_str) == Some(id))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(());
        }
        let remaining = matches[0]
            .get("remaining_fraction")
            .and_then(Value::as_f64)
            .filter(|fraction| fraction.is_finite() && (0.0..=1.0).contains(fraction))
            .ok_or(())?;
        let reset = matches[0]
            .get("reset_time")
            .and_then(Value::as_str)
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .ok_or(())?;
        windows.push(UsageWindow {
            label: label.into(),
            utilization: Some((1.0 - remaining) * 100.0),
            resets_at: Some(reset.into()),
        });
    }
    Ok(EngineUsage {
        engine: gemini_engine(),
        available: true,
        plan: None,
        windows,
        cost_usd: None,
        note: Some("Quota reported by Antigravity.".into()),
        account: None,
        account_label: None,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{AntigravityUsageReader, parse_usage, supported_version};

    #[tokio::test]
    async fn an_abandoned_read_kills_the_helpers_the_cli_spawned() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let helper_pid = temporary.path().join("helper.pid");
        let script = temporary.path().join("agy");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n",
                helper_pid.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
        let reader = AntigravityUsageReader::new(script, temporary.path().to_path_buf());
        let abandoned =
            tokio::time::timeout(std::time::Duration::from_millis(500), reader.invoke(&[])).await;
        assert!(abandoned.is_err(), "the helper keeps the read open");
        let pid = std::fs::read_to_string(&helper_pid).expect("helper pid");
        let pid = pid.trim();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let alive = std::process::Command::new("/bin/kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .expect("probe")
                .success();
            if !alive {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the helper outlived the abandoned read"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    const PAYLOAD: &str = r#"{
      "status":"SUCCESS","conversation_id":"","num_turns":0,
      "usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,
        "cache_read_tokens":0,"total_tokens":0},
      "response":"rounded display text is deliberately ignored",
      "command":{"name":"usage","data":{"groups":[
        {"buckets":[
          {"id":"gemini-weekly","window":"weekly","remaining_fraction":0.75,
            "reset_time":"2026-09-02T19:59:25Z"},
          {"id":"gemini-5h","window":"5h","remaining_fraction":0.4,
            "reset_time":"2026-08-29T08:15:47Z"}
        ]},
        {"buckets":[
          {"id":"3p-weekly","window":"weekly","remaining_fraction":0.1,
            "reset_time":"2026-09-05T03:15:47Z"}
        ]}
      ]}}}
    "#;

    #[tokio::test]
    async fn the_version_is_asked_once_per_executable_file() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let payload = temporary.path().join("usage.json");
        std::fs::write(&payload, PAYLOAD).expect("payload");
        let log = temporary.path().join("calls");
        let script = |version: &str| {
            format!(
                "#!/bin/sh\necho \"$1\" >> '{log}'\nif [ \"$1\" = --version ]; then echo {version}; else cat '{payload}'; fi\n",
                log = log.display(),
                payload = payload.display(),
            )
        };
        let install = |name: &str, version: &str| {
            let path = temporary.path().join(name);
            std::fs::write(&path, script(version)).expect("script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("script mode");
            path
        };
        let calls = || std::fs::read_to_string(&log).expect("calls");
        let executable = install("agy", "1.1.11");
        let reader =
            AntigravityUsageReader::new(executable.clone(), temporary.path().to_path_buf());

        assert!(reader.read().await.expect("first read").available);
        assert!(reader.read().await.expect("second read").available);
        assert_eq!(calls(), "--version\n--print\n--print\n");

        // An update to an unsupported version installs a new file at the same path.
        std::fs::rename(install("next", "1.1.10"), &executable).expect("update");
        assert!(reader.read().await.is_err());
        assert_eq!(calls(), "--version\n--print\n--print\n--version\n");
    }

    #[tokio::test]
    async fn the_childs_browser_openers_are_no_ops() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let seen = temporary.path().join("seen");
        let script = temporary.path().join("agy");
        // Record how a browser launch from the child would resolve, the two
        // routes `agy` uses for its sign-in URL. Never actually run `open`:
        // if this regresses, resolution is the failure, not a real browser.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf 'browser=%s\\nopen=%s\\n' \"$BROWSER\" \"$(command -v open)\" > '{}'\nif [ \"$1\" = --version ]; then echo 1.1.11; else echo '{{}}'; fi\n",
                seen.display(),
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
        let runtime_root = temporary.path().join("runtime");
        std::fs::create_dir_all(&runtime_root).expect("runtime root");
        let reader = AntigravityUsageReader::new(script, runtime_root.clone());
        let version = reader.invoke(&["--version"]).await.expect("version read");
        assert_eq!(version, b"1.1.11\n");

        let stub = runtime_root.join("antigravity-no-browser").join("open");
        assert_eq!(
            std::fs::read_to_string(&seen).expect("seen"),
            format!("browser={0}\nopen={0}\n", stub.display()),
        );
        let stub_command = std::process::Command::new(&stub)
            .arg("https://accounts.google.com/")
            .status()
            .expect("stub run");
        assert!(stub_command.success(), "the stub quietly does nothing");
    }

    #[tokio::test]
    async fn a_failed_read_backs_off_instead_of_respawning() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let log = temporary.path().join("calls");
        let script = temporary.path().join("agy");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$1\" >> '{}'\nif [ \"$1\" = --version ]; then echo 1.1.11; else exit 1; fi\n",
                log.display(),
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
        let calls = || std::fs::read_to_string(&log).expect("calls");

        let backing_off =
            AntigravityUsageReader::new(script.clone(), temporary.path().to_path_buf());
        assert!(backing_off.read().await.is_err());
        assert!(backing_off.read().await.is_err());
        assert_eq!(calls(), "--version\n--print\n", "second ask never spawns");

        // The same reader without a backoff asks the CLI every time, so the
        // suppression above is the backoff, not the failure itself.
        let mut eager = AntigravityUsageReader::new(script, temporary.path().to_path_buf());
        eager.backoff = std::time::Duration::ZERO;
        assert!(eager.read().await.is_err());
        assert!(eager.read().await.is_err());
        assert_eq!(calls(), "--version\n--print\n--version\n--print\n--print\n");
    }

    #[test]
    fn version_gate_refuses_any_cli_that_could_treat_usage_as_a_prompt() {
        assert!(!supported_version(b"1.1.10\n"));
        assert!(supported_version(b"1.1.11\n"));
        assert!(supported_version(b"1.1.22\n"));
        assert!(!supported_version(b"2.0.0\n"));
        assert!(!supported_version(b"1.1.22-beta\n"));
        assert!(!supported_version(b"unknown\n"));
    }

    #[test]
    fn maps_remaining_quota_to_used_percent_without_mixing_third_party_pools() {
        let usage = parse_usage(PAYLOAD.as_bytes()).expect("valid quota");
        assert_eq!(usage.engine.as_str(), "acp:gemini");
        assert!(usage.available);
        assert_eq!(usage.plan, None);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "5-hour");
        assert_eq!(usage.windows[0].utilization, Some(60.0));
        assert_eq!(usage.windows[1].label, "Weekly");
        assert_eq!(usage.windows[1].utilization, Some(25.0));
    }

    #[test]
    fn fails_closed_if_the_command_was_a_turn_or_the_quota_shape_drifted() {
        let turn = PAYLOAD.replace("\"num_turns\":0", "\"num_turns\":1");
        assert!(parse_usage(turn.as_bytes()).is_err());
        let tokens = PAYLOAD.replace("\"total_tokens\":0", "\"total_tokens\":1");
        assert!(parse_usage(tokens.as_bytes()).is_err());
        let wrong = PAYLOAD.replace("\"name\":\"usage\"", "\"name\":\"prompt\"");
        assert!(parse_usage(wrong.as_bytes()).is_err());
        let missing = PAYLOAD.replace("gemini-5h", "missing-5h");
        assert!(parse_usage(missing.as_bytes()).is_err());
        let invalid = PAYLOAD.replace("\"remaining_fraction\":0.4", "\"remaining_fraction\":1.4");
        assert!(parse_usage(invalid.as_bytes()).is_err());
    }
}
