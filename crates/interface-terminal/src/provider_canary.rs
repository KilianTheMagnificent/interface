//! Provider CLI drift canary (opportunity review F2, 2026-10-15).
//!
//! About six fixes between September and October came from a provider CLI
//! update that broke what Interface depends on: a hook Codex stopped trusting,
//! a listing that got slower, a version probe that hung, a permission mode
//! Codex renamed. This compares each installed CLI's version, and the flags,
//! hook names and capture strings Interface depends on, with what was recorded
//! when they were last verified by hand.
//!
//! It runs only `--version`, `--help`, schema or listing output and reads of
//! the installed files: it never starts a model turn, signs in, or spends
//! quota. Every command is bounded (stdin closed, output capped, the whole
//! process group stopped at its deadline).
//!
//! It runs on demand (`iface doctor providers`, `cargo xtask provider-canary`)
//! and in the host whenever a CLI's version differs from the one it last saw
//! (`apps/interface-host/src/provider_canary.rs`), which Settings → Terminal
//! tools shows as "Changed since last check".
//!
//! When a CLI update is verified by hand (its panes resume, its hooks fire),
//! move `verified` to that version and adjust the expectations in the same
//! commit; [`expectations_fingerprint`] changes with them, so every host
//! checks its CLIs again.

use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use interface_common::process::{
    CapturedPipe, PIPE_IDLE_AFTER_EXIT, poll_backoff, terminate_process_group,
};
use interface_contracts::terminal::TerminalLaunchProfileId;

/// How long one canary command may run.
pub const CANARY_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// The most output one canary command may keep.
const MAX_COMMAND_OUTPUT: usize = 1024 * 1024;
/// The largest executable file the canary reads for its strings.
const MAX_EXECUTABLE_BYTES: u64 = 768 * 1024 * 1024;
/// The largest Codex schema file the canary reads.
const MAX_SCHEMA_BYTES: u64 = 32 * 1024 * 1024;

/// One thing a CLI must still do or say.
#[derive(Debug)]
pub enum CanaryProbe {
    /// `<cli> <args>` answers within its deadline and names each entry.
    Help {
        args: &'static [&'static str],
        names: &'static [&'static str],
    },
    /// `<cli> <args> --version` is still accepted: a flag, subcommand or
    /// choice value Interface passes still parses.
    Accepts { args: &'static [&'static str] },
    /// The installed executable still carries each string: hook event and
    /// field names, config keys, the capture contract.
    Executable { names: &'static [&'static str] },
    /// `codex app-server generate-json-schema` still declares each JSON-RPC
    /// method the Codex adapter sends.
    CodexMethods { names: &'static [&'static str] },
}

/// What Interface depends on in one CLI, as last verified by hand.
#[derive(Debug)]
pub struct CanaryExpectation {
    pub profile: TerminalLaunchProfileId,
    /// The command name Interface looks for.
    pub command: &'static str,
    /// The variable that overrides where the host finds it, when one does.
    pub override_env: Option<&'static str>,
    /// The CLI version these expectations were last checked against.
    pub verified: &'static str,
    pub probes: &'static [CanaryProbe],
    /// What the canary cannot see for this CLI.
    pub note: Option<&'static str>,
}

/// `crates/engine-claude/src/launch.rs`, `worker.rs`, `title.rs`, the pane
/// argv in `apps/interface-host/src/bootstrap.rs` and the hook reader in
/// `claude_terminal_hook.rs`.
const CLAUDE: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &[
            "--resume",
            "--session-id",
            "--fork-session",
            "--settings",
            "--setting-sources",
            "--mcp-config",
            "--strict-mcp-config",
            "--permission-mode",
            "--allow-dangerously-skip-permissions",
            "--output-format",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
            "--no-session-persistence",
            "--verbose",
            "--model",
            "--effort",
            "--add-dir",
            "--allowedTools",
            "--disallowedTools",
            "--tools",
            "--print",
            "--safe-mode",
            "--restricted",
            "--disable-slash-commands",
            "--no-chrome",
            "--system-prompt",
            "--json-schema",
            "--bare",
        ],
    },
    CanaryProbe::Help {
        args: &["auth", "--help"],
        names: &["status", "login"],
    },
    // Each permission mode Interface passes must still parse.
    CanaryProbe::Accepts {
        args: &["--permission-mode", "default"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "acceptEdits"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "plan"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "auto"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "dontAsk"],
    },
    CanaryProbe::Accepts {
        args: &["--permission-mode", "bypassPermissions"],
    },
    CanaryProbe::Accepts {
        args: &[
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
        ],
    },
    // The pane's working signal and session capture.
    CanaryProbe::Executable {
        names: &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "hook_event_name",
            "session_id",
            "transcript_path",
            "background_tasks",
            "terminalSequence",
        ],
    },
];

/// The pane's `-c` overrides and hooks (`bootstrap.rs`, `fast_mode.rs`),
/// `codex resume`, the app-server adapter (`crates/engine-codex`) and the
/// restricted worker (`worker.rs`).
const CODEX: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &[
            "resume",
            "app-server",
            "exec",
            "login",
            "features",
            "--config",
            "--enable",
            "--disable",
        ],
    },
    CanaryProbe::Help {
        args: &["resume", "--help"],
        names: &["--last"],
    },
    CanaryProbe::Help {
        args: &["login", "--help"],
        names: &["status"],
    },
    CanaryProbe::Help {
        args: &["exec", "--help"],
        names: &[
            "--ignore-user-config",
            "--strict-config",
            "--sandbox",
            "--skip-git-repo-check",
            "--ephemeral",
            "--json",
            "--color",
            "--model",
            "--output-schema",
        ],
    },
    // Every feature Interface turns on or off by name.
    CanaryProbe::Help {
        args: &["features", "list"],
        names: &[
            "hooks",
            "plugins",
            "apps",
            "browser_use",
            "computer_use",
            "in_app_browser",
            "multi_agent",
            "multi_agent_v2",
            "unified_exec",
            "unified_exec_tty",
            "code_mode",
            "code_mode_host",
            "shell_tool",
            "memories",
            "image_generation",
            "shell_snapshot",
            "skill_search",
            "tool_suggest",
            "view_image",
            "workspace_dependencies",
            "skip_host_skill_discovery",
        ],
    },
    CanaryProbe::Accepts { args: &["resume"] },
    CanaryProbe::CodexMethods {
        names: &[
            "initialize",
            "model/list",
            "account/read",
            "account/logout",
            "account/rateLimits/read",
            "account/usage/read",
            "mcpServerStatus/list",
            "config/read",
            "skills/list",
            "thread/start",
            "thread/resume",
            "thread/list",
            "thread/read",
            "thread/name/set",
            "thread/delete",
            "thread/fork",
            "thread/archive",
            "thread/unarchive",
            "thread/compact/start",
            "review/start",
            "turn/start",
            "turn/steer",
            "turn/interrupt",
        ],
    },
    // The pane's hooks, trust state, title and attention signal.
    CanaryProbe::Executable {
        names: &[
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "hook_event_name",
            "transcript_path",
            "trusted_hash",
            "notification_method",
            "osc9",
            "terminal_title",
            "toggle_fast_mode",
            "session_index.jsonl",
        ],
    },
];

/// The pane argv, the ACP launch (`crates/engine-acp/src/profile.rs`) and
/// the pane tools switch (`pane_tools.rs`).
const GROK: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &["--session-id", "--resume", "--model", "mcp", "agent"],
    },
    CanaryProbe::Help {
        args: &["agent", "--help"],
        names: &["stdio", "--always-approve", "--model", "--reasoning-effort"],
    },
    CanaryProbe::Help {
        args: &["mcp", "--help"],
        names: &["list", "add", "remove"],
    },
    CanaryProbe::Accepts {
        args: &["--no-leader"],
    },
    CanaryProbe::Accepts {
        args: &["--no-auto-update"],
    },
];

/// The pane argv (`launch.rs`), the worker (`opencode_worker.rs`), the usage
/// read (`opencode_usage.rs`) and the TUI plugin
/// (`opencode_activity_transport.mjs`).
const OPENCODE: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &["--session", "--prompt", "--auto", "run", "models"],
    },
    CanaryProbe::Help {
        args: &["run", "--help"],
        names: &[
            "--pure",
            "--format",
            "--model",
            "--agent",
            "--title",
            "--dir",
            "--attach",
            "--variant",
        ],
    },
    CanaryProbe::Executable {
        names: &[
            "session.created",
            "session.list",
            "setTerminalTitle",
            "dispatchCommand",
            "route.current",
        ],
    },
];

/// Antigravity (`agy`): the pane argv, the usage read
/// (`antigravity_usage.rs`) and the pane tools switch.
const ANTIGRAVITY: &[CanaryProbe] = &[
    CanaryProbe::Help {
        args: &["--help"],
        names: &[
            "--prompt-interactive",
            "--conversation",
            "--print",
            "--output-format",
            "--print-timeout",
            "mcp",
        ],
    },
    CanaryProbe::Help {
        args: &["plugin", "--help"],
        names: &["install", "list"],
    },
    CanaryProbe::Help {
        args: &["mcp", "--help"],
        names: &["list", "add", "remove"],
    },
];

/// Every CLI the launcher offers, in its order, and Qwen.
pub const EXPECTATIONS: &[CanaryExpectation] = &[
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Claude,
        command: "claude",
        override_env: Some("CLAUDE_CODE_EXECUTABLE"),
        verified: "2.1.300",
        probes: CLAUDE,
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Codex,
        command: "codex",
        override_env: Some("CODEX_EXECUTABLE"),
        verified: "0.162.1",
        probes: CODEX,
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Grok,
        command: "grok",
        override_env: Some("GROK_EXECUTABLE"),
        verified: "1.0.30",
        probes: GROK,
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Gemini,
        command: "agy",
        override_env: Some("ANTIGRAVITY_EXECUTABLE"),
        verified: "1.4.3",
        probes: ANTIGRAVITY,
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Omp,
        command: "omp",
        override_env: None,
        verified: "18.1.16",
        probes: &[],
        note: Some("Interface passes OMP only a prompt; its version is compared."),
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Opencode,
        command: "opencode",
        override_env: None,
        verified: "1.18.34",
        probes: OPENCODE,
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Muse,
        command: "muse",
        override_env: None,
        verified: "1.4.11",
        probes: &[
            CanaryProbe::Help {
                args: &["--help"],
                names: &["--config", "resume"],
            },
            CanaryProbe::Help {
                args: &["resume", "--help"],
                names: &["<session-ref>"],
            },
            // Stopped detection rides on plugin hooks: they are the only
            // hook source a fresh in-process session reloads.
            CanaryProbe::Help {
                args: &["plugins", "--help"],
                names: &["install", "approve"],
            },
        ],
        note: Some(
            "Native hooks and durable startup log readiness also require the disposable resume probe.",
        ),
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Kimi,
        command: "kimi",
        override_env: None,
        verified: "2.3.1",
        // Reopen passes `--session session_<uuid>`; the checkpoint plugin's
        // `kimi.plugin.json` hooks are read from Kimi's own plugin store.
        probes: &[CanaryProbe::Help {
            args: &["--help"],
            names: &["--session"],
        }],
        note: None,
    },
    CanaryExpectation {
        profile: TerminalLaunchProfileId::Qwen,
        command: "qwen",
        override_env: Some("QWEN_EXECUTABLE"),
        verified: "0.22.3",
        probes: &[],
        note: Some("Qwen's --acp flag is hidden from its help; its version is compared."),
    },
];

/// The expectations for one launch profile.
#[must_use]
pub fn expectation(profile: TerminalLaunchProfileId) -> Option<&'static CanaryExpectation> {
    EXPECTATIONS
        .iter()
        .find(|expectation| expectation.profile == profile)
}

/// A stable fingerprint of [`EXPECTATIONS`] (FNV-1a over every string): a
/// host that last checked against other expectations checks again.
#[must_use]
pub fn expectations_fingerprint() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |text: &str| {
        for byte in text.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for expectation in EXPECTATIONS {
        feed(expectation.command);
        feed(expectation.verified);
        for probe in expectation.probes {
            let (kind, args, names): (&str, &[&str], &[&str]) = match probe {
                CanaryProbe::Help { args, names } => ("help", args, names),
                CanaryProbe::Accepts { args } => ("accepts", args, &[]),
                CanaryProbe::Executable { names } => ("executable", &[], names),
                CanaryProbe::CodexMethods { names } => ("codex-methods", &[], names),
            };
            feed(kind);
            args.iter().chain(names).for_each(|text| feed(text));
        }
    }
    format!("{hash:016x}")
}

/// Where the CLI is when nothing told the host otherwise: its override
/// variable, then `PATH`, then the usual install folders.
#[must_use]
pub fn locate(
    expectation: &CanaryExpectation,
    home: &Path,
    path_variable: Option<&std::ffi::OsStr>,
    override_value: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let runnable = |path: &Path| {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    if let Some(value) = override_value.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return runnable(&path).then_some(path);
    }
    let from_path = path_variable
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(expectation.command));
    let usual = [
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]
    .into_iter()
    .map(|directory| directory.join(expectation.command));
    from_path.chain(usual).find(|path| runnable(path))
}

/// What the canary found for one CLI.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CanaryReport {
    pub label: String,
    pub executable: Option<PathBuf>,
    /// The first line `--version` printed.
    pub version: Option<String>,
    /// The version number in it.
    pub version_number: Option<String>,
    pub verified: String,
    /// Probes run, the version included.
    pub checks: usize,
    /// What no longer matches, one sentence each.
    pub findings: Vec<String>,
    pub note: Option<String>,
}

impl CanaryReport {
    #[must_use]
    pub fn installed(&self) -> bool {
        self.executable.is_some()
    }

    /// One line for a person.
    #[must_use]
    pub fn summary(&self) -> String {
        let Some(version) = self.version_number.as_deref().or(self.version.as_deref()) else {
            return if self.installed() {
                format!("{}: {}", self.label, self.findings.join(" "))
            } else {
                format!("{}: not installed", self.label)
            };
        };
        let against = if version == self.verified {
            String::new()
        } else {
            format!(" (verified at {})", self.verified)
        };
        if self.findings.is_empty() {
            format!(
                "{} {version}{against}: what Interface depends on still matches ({} check{})",
                self.label,
                self.checks,
                if self.checks == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} {version}{against}: {} {} no longer {}: {}",
                self.label,
                self.findings.len(),
                if self.findings.len() == 1 {
                    "thing Interface depends on"
                } else {
                    "things Interface depends on"
                },
                if self.findings.len() == 1 {
                    "matches"
                } else {
                    "match"
                },
                self.findings.join(" ")
            )
        }
    }
}

/// Run every probe for one CLI at `executable`.
#[must_use]
pub fn run_canary(expectation: &CanaryExpectation, executable: &Path) -> CanaryReport {
    let mut report = CanaryReport {
        label: crate::profile_label(expectation.profile).to_owned(),
        executable: Some(executable.to_path_buf()),
        verified: expectation.verified.to_owned(),
        note: expectation.note.map(str::to_owned),
        ..CanaryReport::default()
    };
    let shown = |args: &[&str]| {
        std::iter::once(expectation.command)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    };
    report.checks += 1;
    match run_bounded(executable, &["--version"], CANARY_COMMAND_TIMEOUT) {
        Ok(answer) if answer.success => {
            report.version = answer
                .text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().filter(|c| !c.is_control()).take(80).collect());
            report.version_number = report.version.as_deref().and_then(version_number);
        }
        Ok(answer) => report.findings.push(format!(
            "`{} --version` failed: {}.",
            expectation.command,
            first_line(&answer.text)
        )),
        Err(error) => report
            .findings
            .push(format!("`{} --version` {error}.", expectation.command)),
    }
    for probe in expectation.probes {
        report.checks += 1;
        let finding = match probe {
            CanaryProbe::Help { args, names } => {
                match run_bounded(executable, args, CANARY_COMMAND_TIMEOUT) {
                    Ok(answer) => missing_sentence(
                        &format!("`{}`", shown(args)),
                        "no longer lists",
                        &missing(&answer.text, names),
                    ),
                    Err(error) => Some(format!("`{}` {error}.", shown(args))),
                }
            }
            CanaryProbe::Accepts { args } => {
                let mut with_version = args.to_vec();
                with_version.push("--version");
                match run_bounded(executable, &with_version, CANARY_COMMAND_TIMEOUT) {
                    Ok(answer) if answer.success => None,
                    Ok(answer) => Some(format!(
                        "`{}` is no longer accepted: {}.",
                        shown(args),
                        first_line(&answer.text)
                    )),
                    Err(error) => Some(format!("`{}` {error}.", shown(with_version.as_slice()))),
                }
            }
            CanaryProbe::Executable { names } => match executable_strings(executable, names) {
                Ok(Some(absent)) => missing_sentence(
                    &format!("The installed {}", expectation.command),
                    "no longer carries",
                    &absent,
                ),
                Ok(None) => {
                    report.checks -= 1;
                    None
                }
                Err(error) => Some(format!(
                    "The installed {} could not be read: {error}.",
                    expectation.command
                )),
            },
            CanaryProbe::CodexMethods { names } => codex_methods(executable, names),
        };
        report.findings.extend(finding);
    }
    report
}

/// The canary for every CLI the launcher offers, each found the way
/// [`locate`] does, run one after another.
#[must_use]
pub fn run_all(
    home: &Path,
    path_variable: Option<&std::ffi::OsStr>,
    variable: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<CanaryReport> {
    EXPECTATIONS
        .iter()
        .map(|expectation| {
            let override_value = expectation.override_env.and_then(&variable);
            match locate(expectation, home, path_variable, override_value.as_deref()) {
                Some(executable) => run_canary(expectation, &executable),
                None => CanaryReport {
                    label: crate::profile_label(expectation.profile).to_owned(),
                    verified: expectation.verified.to_owned(),
                    note: expectation.note.map(str::to_owned),
                    ..CanaryReport::default()
                },
            }
        })
        .collect()
}

/// The text report: one line per CLI, then what could not be checked.
#[must_use]
pub fn render_text(reports: &[CanaryReport]) -> String {
    let mut text =
        String::from("provider CLIs (versions, flags, hooks and capture; no model turns):\n");
    for report in reports {
        text.push_str("  ");
        text.push_str(&report.summary());
        text.push('\n');
    }
    let notes = reports
        .iter()
        .filter(|report| report.installed())
        .filter_map(|report| report.note.as_deref())
        .collect::<Vec<_>>();
    for note in notes {
        text.push_str("  note: ");
        text.push_str(note);
        text.push('\n');
    }
    text
}

fn missing<'a>(text: &str, names: &[&'a str]) -> Vec<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| !text.contains(name))
        .collect()
}

fn missing_sentence(subject: &str, verb: &str, absent: &[impl AsRef<str>]) -> Option<String> {
    (!absent.is_empty()).then(|| {
        let names = absent.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        format!("{subject} {verb} {}.", names.join(", "))
    })
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output");
    line.chars().filter(|c| !c.is_control()).take(160).collect()
}

/// The first dotted number in a `--version` line: `2.1.300` in
/// `2.1.300 (Claude Code)`, `18.1.16` in `omp/18.1.16`.
#[must_use]
pub fn version_number(line: &str) -> Option<String> {
    line.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|part| part.trim_matches('.'))
        .find(|part| part.contains('.') && part.split('.').all(|piece| !piece.is_empty()))
        .map(str::to_owned)
}

struct Answer {
    success: bool,
    text: String,
}

/// Run one command with stdin closed, its output capped and its whole process
/// group stopped at `timeout`.
fn run_bounded(executable: &Path, args: &[&str], timeout: Duration) -> Result<Answer, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start ({error})"))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        terminate_process_group(child);
        return Err("could not be read".to_owned());
    };
    let stdout = CapturedPipe::spawn(stdout, MAX_COMMAND_OUTPUT, Arc::clone(&overflow));
    let stderr = CapturedPipe::spawn(stderr, MAX_COMMAND_OUTPUT, overflow);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(poll_backoff(started.elapsed(), Duration::from_millis(50)));
            }
            Ok(None) | Err(_) => {
                terminate_process_group(child);
                return Err(format!("did not answer within {} s", timeout.as_secs()));
            }
        }
    };
    let group = Some(child.id());
    stdout.child_exited(group);
    stderr.child_exited(group);
    let settle = Instant::now();
    while !(stdout.is_finished() && stderr.is_finished())
        && settle.elapsed() < PIPE_IDLE_AFTER_EXIT * 10
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut bytes = stdout.finish().unwrap_or_default();
    bytes.extend(stderr.finish().unwrap_or_default());
    Ok(Answer {
        success: status.success(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Which of `names` the installed executable no longer carries; `None` when
/// it is a script (its code lives elsewhere), so the probe does not apply.
fn executable_strings(executable: &Path, names: &[&str]) -> io::Result<Option<Vec<String>>> {
    let resolved = fs::canonicalize(executable)?;
    let mut file = File::open(&resolved)?;
    let length = file.metadata()?.len();
    if length > MAX_EXECUTABLE_BYTES {
        return Err(io::Error::other(format!(
            "it is larger than {} MiB",
            MAX_EXECUTABLE_BYTES / 1024 / 1024
        )));
    }
    let mut head = [0_u8; 2];
    if file.read(&mut head)? == 2 && head == *b"#!" {
        return Ok(None);
    }
    let longest = names.iter().map(|name| name.len()).max().unwrap_or(0);
    let finders = names
        .iter()
        .map(|name| memchr::memmem::Finder::new(name.as_bytes()))
        .collect::<Vec<_>>();
    let mut found = vec![false; names.len()];
    let mut window = head.to_vec();
    let mut chunk = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        window.extend_from_slice(&chunk[..count]);
        for (index, finder) in finders.iter().enumerate() {
            if !found[index] && finder.find(&window).is_some() {
                found[index] = true;
            }
        }
        if found.iter().all(|found| *found) {
            break;
        }
        let keep = window.len().min(longest.saturating_sub(1));
        window.drain(..window.len() - keep);
    }
    Ok(Some(
        names
            .iter()
            .zip(found)
            .filter(|(_, found)| !found)
            .map(|(name, _)| (*name).to_owned())
            .collect(),
    ))
}

/// Generate Codex's app-server schema into a private temporary folder and
/// check its client requests still include each method.
fn codex_methods(executable: &Path, names: &[&str]) -> Option<String> {
    let folder = std::env::temp_dir().join(format!(
        "interface-provider-canary-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    ));
    {
        use std::os::unix::fs::DirBuilderExt as _;
        if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&folder) {
            return Some(format!(
                "The Codex app-server schema could not be generated ({error})."
            ));
        }
    }
    let shown = "`codex app-server generate-json-schema`";
    let out = folder.to_string_lossy().into_owned();
    let result = match run_bounded(
        executable,
        &["app-server", "generate-json-schema", "--out", &out],
        CANARY_COMMAND_TIMEOUT,
    ) {
        Ok(answer) if answer.success => {
            let schema = folder.join("ClientRequest.json");
            match fs::metadata(&schema) {
                Ok(metadata) if metadata.len() <= MAX_SCHEMA_BYTES => {
                    match fs::read_to_string(&schema) {
                        Ok(text) => {
                            let quoted = names
                                .iter()
                                .map(|name| format!("\"{name}\""))
                                .collect::<Vec<_>>();
                            let absent = names
                                .iter()
                                .zip(&quoted)
                                .filter(|(_, quoted)| !text.contains(quoted.as_str()))
                                .map(|(name, _)| *name)
                                .collect::<Vec<_>>();
                            missing_sentence(
                                "Codex's app-server schema",
                                "no longer declares",
                                &absent,
                            )
                        }
                        Err(error) => {
                            Some(format!("{shown} wrote an unreadable schema ({error})."))
                        }
                    }
                }
                Ok(_) => Some(format!("{shown} wrote an oversized schema.")),
                Err(_) => Some(format!("{shown} no longer writes ClientRequest.json.")),
            }
        }
        Ok(answer) => Some(format!("{shown} failed: {}.", first_line(&answer.text))),
        Err(error) => Some(format!("{shown} {error}.")),
    };
    let _ = fs::remove_dir_all(&folder);
    result
}

#[cfg(test)]
#[path = "provider_canary_tests.rs"]
mod tests;
