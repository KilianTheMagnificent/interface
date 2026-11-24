//! The `iface` pane MCP and the dedicated `cheap_agents` report MCP, each on
//! a fixed owner-only socket per host. `pass_off` opens a new agent pane beside the
//! calling pane holding only the prompt the caller wrote; `read_pane` and
//! `send_to_pane` are the voice assistant's, shared, so the new agent can
//! report back by typing into the pane that sent it.
//!
//! Which pane is calling is never taken from tool input. The connecting
//! process is `nc`, spawned by the pane's CLI, and the kernel's peer pid plus
//! one `/bin/ps` listing walk that chain up to a pane the terminal service owns.
//! Claude and Codex learn the socket path per launch (`bootstrap.rs`); Grok
//! and Antigravity keep one static config entry that reads it from
//! [`PANE_MCP_SOCKET_ENV`], which every pane launch exports. That entry is
//! the owner's to turn on in Settings (protocol 48); Interface never writes
//! it at start.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    ipc::CHANNELS,
    terminal::{
        CreateTerminalRequest, TERMINAL_HANDOFF_TITLE_MAX_BYTES,
        TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES, TERMINAL_HANDOFF_VERSION,
        TERMINAL_LAUNCH_REQUEST_VERSION, TerminalHandoffKind, TerminalHandoffV1, TerminalId,
        TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalSize, TerminalSummary,
    },
};
use interface_terminal::{
    pane_tools::{PaneToolsCli, PaneToolsEntry},
    profile_label,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    bounded_process::{BoundedRunError, Capture, CapturedOutput, run_bounded},
    dispatcher::HostDispatcher,
    mcp_transport::{
        BoundSocket, MAX_UNIX_SOCKET_PATH_BYTES, McpServerSpec, Peer, ToolBackend, ToolOutput,
        bind, remove_bound_socket, serve, tool,
    },
    terminal::TerminalHostDispatcher,
    voice::{
        mcp::{read_pane_tool, send_to_pane_tool},
        tools::{
            ReadPane, SendToPane, arguments, decode, dispatch_message, find_pane, pane_json,
            read_pane, send_to_pane as type_into_pane,
        },
    },
};

/// Every pane launch carries the socket path here; the static Grok and
/// Antigravity config entry expands it.
pub const PANE_MCP_SOCKET_ENV: &str = interface_terminal::pane_tools::PANE_TOOLS_SOCKET_ENV;
/// The `--mcp-config` key and the `mcp__iface__<tool>` prefix.
pub const PANE_MCP_SERVER_NAME: &str = interface_terminal::pane_tools::PANE_TOOLS_SERVER_NAME;
pub const PANE_TOOL_NAMES: [&str; 3] = ["pass_off", "read_pane", "send_to_pane"];

const SOCKET_DIRECTORY: &str = "panes";
const SOCKET_FILE: &str = "iface.sock";
// Hosts before protocol 48 wrote the static entry at first start and left
// `<data root>/panes/iface-mcp-config.v1` behind; nothing reads it now.
const CONFIG_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIG_COMMAND_POLL_CAP: Duration = Duration::from_millis(100);
const CONFIG_STDERR_BYTES: usize = 8 * 1024;
/// A CLI's whole MCP server list; anything longer is unreadable.
const CONFIG_LIST_BYTES: usize = 256 * 1024;
const PANE_COLUMNS: u16 = 120;
const PANE_ROWS: u16 = 40;
pub(crate) const UNKNOWN_CALLER: &str =
    "Interface could not tell which pane called; run the agent inside an Interface terminal pane.";
/// Pane sessions have no pane listing: their ids come from a pass-off.
const PASS_OFF_PANE_ID: &str =
    "The pane_id pass_off returned, or the pane named in the pass-off brief you were given.";
const SEND_OUTSIDE_PASS_OFF: &str = "send_to_pane only reaches the pane on the other end of a pass-off: the pane that passed this task to you, or a pane you passed one to.";
const READ_OUTSIDE_PASS_OFF: &str = "read_pane only reads your own pane or the pane on the other end of a pass-off: the pane that passed this task to you, or a pane you passed one to.";
/// Where this host's pane socket lives; one place for the server, the launch
/// catalog and the pane environment.
#[must_use]
pub fn pane_mcp_socket_path(data_root: &Path) -> PathBuf {
    data_root.join(SOCKET_DIRECTORY).join(SOCKET_FILE)
}

/// The dedicated cheap-agent endpoint beside the existing pane-control socket.
#[must_use]
pub fn cheap_agents_mcp_socket_path(pane_socket: &Path) -> PathBuf {
    pane_socket.with_file_name("cheap_agents.sock")
}

/// The bound pane socket; dropping it does not unlink, `stop` does.
pub struct PaneMcpServer {
    path: PathBuf,
    identity: (u64, u64),
    reports_socket: Option<(PathBuf, (u64, u64))>,
    stop: CancellationToken,
    reports: Option<Arc<crate::report_workers::NativeWorkers>>,
}

impl PaneMcpServer {
    /// Binds the host's pane socket and serves it. Must be called inside a
    /// Tokio runtime. `None` when the socket cannot be bound; panes then
    /// launch without the tools and the reason is in the log. Optional report
    /// workers share the native pane transport.
    #[must_use]
    pub fn start_with_reports(
        data_root: &Path,
        terminal: Arc<TerminalHostDispatcher>,
        report_executable: Option<PathBuf>,
    ) -> Option<Self> {
        let path = pane_mcp_socket_path(data_root);
        if path.as_os_str().len() > MAX_UNIX_SOCKET_PATH_BYTES {
            tracing::error!(path = %path.display(), "pane MCP socket path exceeds the Unix socket limit");
            return None;
        }
        let root = path.parent()?;
        let BoundSocket { listener, identity } = match bind(root, &path) {
            Ok(bound) => bound,
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "pane MCP socket could not be bound");
                return None;
            }
        };
        let stop = CancellationToken::new();
        let reports =
            report_executable.and_then(
                |executable| match crate::report_workers::NativeWorkers::open(
                    root.join("report-workers"),
                    executable,
                ) {
                    Ok(service) => Some(Arc::new(service)),
                    Err(error) => {
                        tracing::error!(%error, "native report workers unavailable");
                        None
                    }
                },
            );
        let backend = Arc::new(PaneTools {
            terminal,
            reports: reports.clone(),
        });
        let reports_socket = reports
            .as_ref()
            .and_then(|_| start_cheap_agents_mcp(&path, backend.clone(), stop.clone()));
        serve(
            listener,
            &path,
            Arc::new(McpServerSpec {
                wire_name: PANE_MCP_SERVER_NAME,
                title: "Interface",
                // The worker tools are only kept callable here, never listed,
                // so their instructions belong to the cheap_agents server.
                instructions: "",
                tools: tool_catalogue(),
                compatibility_tools: if reports.is_some() {
                    &interface_collaboration::workers::TOOL_NAMES
                } else {
                    &[]
                },
                max_request_line_bytes: None,
            }),
            backend,
            stop.clone(),
        );
        Some(Self {
            path,
            identity,
            reports_socket,
            stop,
            reports,
        })
    }

    #[cfg(test)]
    pub(crate) fn reports(&self) -> Option<&Arc<crate::report_workers::NativeWorkers>> {
        self.reports.as_ref()
    }

    pub fn stop(&self) {
        self.stop.cancel();
        remove_bound_socket(&self.path, self.identity);
        if let Some((path, identity)) = &self.reports_socket {
            remove_bound_socket(path, *identity);
        }
    }

    pub async fn shutdown(&self) {
        self.stop();
        if let Some(reports) = &self.reports {
            reports.service.shutdown().await;
        }
    }
}

fn start_cheap_agents_mcp(
    pane_socket: &Path,
    backend: Arc<PaneTools>,
    stop: CancellationToken,
) -> Option<(PathBuf, (u64, u64))> {
    let path = cheap_agents_mcp_socket_path(pane_socket);
    let BoundSocket { listener, identity } = match bind(path.parent()?, &path) {
        Ok(bound) => bound,
        Err(error) => {
            tracing::error!(%error, "cheap_agents MCP socket could not be bound");
            return None;
        }
    };
    serve(
        listener,
        &path,
        Arc::new(McpServerSpec {
            wire_name: interface_collaboration::workers::MCP_SERVER_NAME,
            title: interface_collaboration::workers::MCP_SERVER_TITLE,
            instructions: interface_collaboration::workers::INSTRUCTIONS,
            tools: interface_collaboration::workers::catalogue(),
            compatibility_tools: &[],
            max_request_line_bytes: None,
        }),
        backend,
        stop,
    );
    Some((path, identity))
}

pub(crate) fn tool_catalogue() -> Vec<Value> {
    vec![
        tool(
            "pass_off",
            "Hand a task to another agent: opens a new pane beside yours running the chosen CLI with only this prompt, at your working directory. The new agent works in that same checkout, not a copy. The prompt is everything the new agent will know, so include the files, goal and constraints. Returns the new pane's id; the agent reports back by typing into your pane when done.",
            &json!({
                "profile": {"type": "string", "enum": ["claude", "codex", "grok", "antigravity", "omp", "opencode", "muse", "kimi"]},
                "prompt": {"type": "string", "description": "the complete brief for the new agent"},
                "title": {"type": "string", "description": "pane title; defaults to the CLI's name"}
            }),
            &["profile", "prompt"],
            false,
        ),
        read_pane_tool(
            "The last lines of a pane's output, with terminal colours stripped. Reads only your own pane or the pane on the other end of a pass-off; any other pane is refused.",
            PASS_OFF_PANE_ID,
        ),
        send_to_pane_tool(
            "Type text into the pane on the other end of a pass-off — the pane that passed this task to you (to report back), or a pane you passed one to; presses Enter afterwards unless enter is false. Any other pane is refused.",
            PASS_OFF_PANE_ID,
        ),
    ]
}

pub(crate) struct PaneTools {
    pub(crate) terminal: Arc<TerminalHostDispatcher>,
    reports: Option<Arc<crate::report_workers::NativeWorkers>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PassOff {
    profile: String,
    prompt: String,
    #[serde(default)]
    title: Option<String>,
}

#[async_trait]
impl ToolBackend for PaneTools {
    async fn call(&self, name: &str, input: Value, peer: Peer) -> Result<ToolOutput, String> {
        if interface_collaboration::workers::TOOL_NAMES.contains(&name) || name == "__worker_notice"
        {
            return self
                .reports
                .as_ref()
                .ok_or("brokerUnavailable: report workers unavailable")?
                .call(self, name, arguments(input)?, peer)
                .await;
        }
        match name {
            "pass_off" => self.pass_off(arguments(input)?, peer).await,
            "read_pane" => self.read_linked_pane(arguments(input)?, peer).await,
            "send_to_pane" => self.send_to_pane(arguments(input)?, peer).await,
            _ => Err(format!("unknown tool {name}")),
        }
    }
}

impl PaneTools {
    async fn pass_off(&self, request: PassOff, peer: Peer) -> Result<ToolOutput, String> {
        let profile_id = pass_off_profile(&request.profile)?;
        if request.prompt.trim().is_empty() {
            return Err("prompt is empty; write the complete brief for the new agent".to_owned());
        }
        if request.prompt.contains('\0') {
            return Err("prompt must not contain NUL bytes".to_owned());
        }
        if request.prompt.len() > TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES {
            return Err(format!(
                "prompt is too long; keep it under {TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES} bytes"
            ));
        }
        let caller = self.caller(peer).await?;
        let create = pass_off_request(&caller, profile_id, request.prompt, request.title);
        let payload = serde_json::to_value(create).map_err(|error| error.to_string())?;
        let summary: TerminalSummary = decode(
            self.terminal
                .dispatch(CHANNELS.create_terminal, payload)
                .await
                .map_err(dispatch_message)?,
        )?;
        Ok(ToolOutput::Text(pane_json(&summary, false).to_string()))
    }

    /// Typing into another pane is confined to the pass-off edge, both ways;
    /// the caller is the kernel-named peer, never the request.
    async fn send_to_pane(&self, request: SendToPane, peer: Peer) -> Result<ToolOutput, String> {
        let caller = self.caller(peer).await?;
        let target = find_pane(&self.terminal, &request.pane_id).await?;
        if !may_send_to_pane(&caller, &target) {
            return Err(SEND_OUTSIDE_PASS_OFF.to_owned());
        }
        type_into_pane(&self.terminal, request).await
    }

    /// Reading follows the same edge as typing, plus the caller's own pane:
    /// every pane's agent has this tool pre-approved, and a screen can hold
    /// another project's output or a secret printed in a shell.
    async fn read_linked_pane(&self, request: ReadPane, peer: Peer) -> Result<ToolOutput, String> {
        let caller = self.caller(peer).await?;
        let target = find_pane(&self.terminal, request.pane_id()).await?;
        if !may_read_pane(&caller, &target) {
            return Err(READ_OUTSIDE_PASS_OFF.to_owned());
        }
        read_pane(&self.terminal, request).await
    }

    /// The pane the connection came from, or a refusal. The `ps` walk shells
    /// out, so it runs on the blocking pool.
    pub(crate) async fn caller(&self, peer: Peer) -> Result<TerminalSummary, String> {
        let pid = peer.pid.ok_or_else(|| UNKNOWN_CALLER.to_owned())?;
        let terminal = Arc::clone(&self.terminal);
        let (id, generation) =
            tokio::task::spawn_blocking(move || terminal.terminal_for_process_ancestor(pid))
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| UNKNOWN_CALLER.to_owned())?;
        let pane = find_pane(&self.terminal, &id.to_string()).await?;
        if pane.generation != generation {
            return Err(UNKNOWN_CALLER.to_owned());
        }
        Ok(pane)
    }
}

/// Whether `caller` may type into `target`: only across a pass-off edge — the
/// delegate reporting back to the pane that passed it the task, or that pane
/// nudging its own delegate. Everything else, shells included, is refused.
fn may_send_to_pane(caller: &TerminalSummary, target: &TerminalSummary) -> bool {
    caller.passed_off_from == Some(target.id) || target.passed_off_from == Some(caller.id)
}

/// Whether `caller` may read `target`'s screen: its own, or across a pass-off edge.
fn may_read_pane(caller: &TerminalSummary, target: &TerminalSummary) -> bool {
    caller.id == target.id || may_send_to_pane(caller, target)
}

/// Maximum character count accepted by `interface-terminal`'s `normalize_title`.
const MAX_PANE_TITLE_CHARS: usize = 120;

/// Sanitize a caller-supplied pane title so control characters or overlong
/// strings never fail `create_terminal`.
fn requested_pane_title(title: Option<String>, profile_id: TerminalLaunchProfileId) -> String {
    title
        .map(|raw| {
            if raw.chars().any(char::is_control)
                || raw.trim().chars().count() > MAX_PANE_TITLE_CHARS
            {
                raw.chars()
                    .map(|ch| {
                        if matches!(ch, '\n' | '\r' | '\t') {
                            ' '
                        } else {
                            ch
                        }
                    })
                    .filter(|ch| !ch.is_control())
                    .collect::<String>()
                    .trim()
                    .chars()
                    .take(MAX_PANE_TITLE_CHARS)
                    .collect::<String>()
            } else {
                raw
            }
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| profile_label(profile_id).to_owned())
}

/// The create the tool dispatches: the caller's working directory and title,
/// the prompt as a brief, the source id the renderer seats the pane beside.
fn pass_off_request(
    caller: &TerminalSummary,
    profile_id: TerminalLaunchProfileId,
    prompt: String,
    title: Option<String>,
) -> CreateTerminalRequest {
    CreateTerminalRequest {
        sign_in: false,
        id: TerminalId::random(),
        cwd: caller.cwd.clone(),
        size: TerminalSize {
            columns: PANE_COLUMNS,
            rows: PANE_ROWS,
        },
        title: Some(requested_pane_title(title, profile_id)),
        session_key: None,
        launch: Some(TerminalLaunchRequestV1 {
            version: TERMINAL_LAUNCH_REQUEST_VERSION,
            profile_id,
        }),
        // Grok takes its session id on argv (`--session-id=<uuid>`) at first
        // launch rather than reporting one through a hook receipt.
        launch_session: (profile_id == TerminalLaunchProfileId::Grok).then(|| {
            interface_contracts::terminal::TerminalLaunchSessionV1 {
                version: interface_contracts::terminal::TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some(uuid::Uuid::new_v4().to_string()),
                resume: false,
            }
        }),
        handoff: Some(TerminalHandoffV1 {
            version: TERMINAL_HANDOFF_VERSION,
            source_terminal_id: caller.id,
            transcript: prompt,
            from_title: handoff_title(&caller.title),
            kind: Some(TerminalHandoffKind::Brief),
        }),
        account: None,
    }
}

/// The profiles a prompt can be bound to; the shell and Qwen are refused by
/// the terminal service too, this just says so in the tool's own words.
fn pass_off_profile(name: &str) -> Result<TerminalLaunchProfileId, String> {
    Ok(match name {
        "claude" => TerminalLaunchProfileId::Claude,
        "codex" => TerminalLaunchProfileId::Codex,
        "grok" => TerminalLaunchProfileId::Grok,
        "omp" => TerminalLaunchProfileId::Omp,
        "opencode" => TerminalLaunchProfileId::Opencode,
        "muse" => TerminalLaunchProfileId::Muse,
        "kimi" => TerminalLaunchProfileId::Kimi,
        "antigravity" | "gemini" => TerminalLaunchProfileId::Gemini,
        other => {
            return Err(format!(
                "unknown profile {other}; choose claude, codex, grok, antigravity, omp, opencode, muse or kimi"
            ));
        }
    })
}

/// The caller's title as the terminal service accepts it, or none: a title it
/// would refuse fails the whole create, and the pass-off is worth more than
/// the provenance line. Same rules as the renderer's `passOffTitle`.
fn handoff_title(title: &str) -> Option<String> {
    let trimmed = title.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= TERMINAL_HANDOFF_TITLE_MAX_BYTES
        && !trimmed.chars().any(char::is_control))
    .then(|| trimmed.to_owned())
}

/// What `cli`'s own config holds for Interface's tools, read through its
/// `mcp list`. Run from `/`, so no project config applies.
#[must_use]
pub fn read_pane_tools_entry(cli: PaneToolsCli, executable: &Path) -> PaneToolsEntry {
    let mut command = Command::new(executable);
    command
        .args(cli.list_arguments().iter().map(OsStr::new))
        .current_dir("/")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let capture = Capture {
        output: CapturedOutput::Stdout,
        max_bytes: CONFIG_LIST_BYTES,
        overflow_fails: true,
    };
    match run_bounded(
        &mut command,
        CONFIG_COMMAND_TIMEOUT,
        CONFIG_COMMAND_POLL_CAP,
        capture,
    ) {
        Ok((status, Ok(stdout))) if status.success() => {
            cli.read_entry(&String::from_utf8_lossy(&stdout))
        }
        Ok(_) | Err(_) => {
            tracing::warn!(cli = cli.label(), "could not read the CLI's MCP servers");
            PaneToolsEntry::Unreadable
        }
    }
}

/// Why turning Interface's tools on or off did not happen.
#[derive(Debug, Eq, PartialEq)]
pub enum PaneToolsChangeError {
    /// The CLI's list of MCP servers could not be read, so nothing ran.
    Unreadable,
    /// The CLI already has an `iface` entry that is not Interface's.
    Taken,
    /// The CLI refused or could not start: nothing changed.
    Refused(String),
    /// The CLI did not answer in time; the entry may or may not have changed.
    Unknown(String),
}

/// Turns `cli`'s use of Interface's tools on (writes Interface's entry into
/// its user config) or off (removes that entry, and only when it is exactly
/// Interface's), then reads it again.
///
/// # Errors
///
/// See [`PaneToolsChangeError`]: an owner's own `iface` entry is never
/// replaced or removed.
pub fn set_pane_tools_entry(
    cli: PaneToolsCli,
    executable: &Path,
    enabled: bool,
) -> Result<PaneToolsEntry, PaneToolsChangeError> {
    let current = read_pane_tools_entry(cli, executable);
    let arguments = match (current, enabled) {
        (PaneToolsEntry::Unreadable, _) => return Err(PaneToolsChangeError::Unreadable),
        (PaneToolsEntry::Foreign, true) => return Err(PaneToolsChangeError::Taken),
        (PaneToolsEntry::Ours { enabled: true }, true)
        | (PaneToolsEntry::Absent | PaneToolsEntry::Foreign, false) => return Ok(current),
        (PaneToolsEntry::Absent | PaneToolsEntry::Ours { enabled: false }, true) => {
            cli.add_arguments()
        }
        (PaneToolsEntry::Ours { .. }, false) => cli.remove_arguments(),
    };
    match run_config_command(executable, &arguments, CONFIG_COMMAND_TIMEOUT) {
        Ok(()) => {
            tracing::info!(
                cli = cli.label(),
                enabled,
                "changed the iface MCP server entry"
            );
            Ok(read_pane_tools_entry(cli, executable))
        }
        Err(ConfigCommandError::NotRun(message)) => Err(PaneToolsChangeError::Refused(message)),
        Err(ConfigCommandError::Unknown(message)) => Err(PaneToolsChangeError::Unknown(message)),
    }
}

/// A config command that did not succeed: `NotRun` changed nothing (it did
/// not start, or the CLI refused); `Unknown` may have.
#[derive(Debug)]
enum ConfigCommandError {
    NotRun(String),
    Unknown(String),
}

/// Runs one config command from `/` with no stdin, a capped stderr, and a
/// deadline.
fn run_config_command(
    executable: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<(), ConfigCommandError> {
    let mut command = Command::new(executable);
    command
        .args(arguments.iter().map(OsStr::new))
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let capture = Capture {
        output: CapturedOutput::Stderr,
        max_bytes: CONFIG_STDERR_BYTES,
        overflow_fails: false,
    };
    let (status, stderr) = run_bounded(&mut command, timeout, CONFIG_COMMAND_POLL_CAP, capture)
        .map_err(|error| match error {
            BoundedRunError::Spawn(error) => {
                ConfigCommandError::NotRun(format!("could not start: {error}"))
            }
            BoundedRunError::TimedOut | BoundedRunError::Wait | BoundedRunError::Overflow => {
                ConfigCommandError::Unknown(format!("did not finish within {}s", timeout.as_secs()))
            }
        })?;
    let stderr = stderr
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(ConfigCommandError::NotRun(format!(
            "exited with {status}: {stderr}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, time::Instant};

    use interface_contracts::terminal::{TerminalGeneration, TerminalState};

    use super::*;

    fn caller() -> TerminalSummary {
        TerminalSummary {
            id: TerminalId::random(),
            generation: TerminalGeneration::initial(),
            title: "Claude Code".to_owned(),
            root: "/tmp/project".to_owned(),
            cwd: "/tmp/project/crates".to_owned(),
            session_key: None,
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            launch_session: None,
            state: TerminalState::Running,
            size: TerminalSize::default(),
            created_at: "2026-09-05T00:00:00.000Z".to_owned(),
            updated_at: "2026-09-05T00:00:00.000Z".to_owned(),
            exit: None,
            passed_off_from: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            replaces: None,
        }
    }

    #[test]
    fn pass_off_builds_a_brief_create_at_the_callers_cwd_pointing_back_at_it() {
        let caller = caller();
        let create = pass_off_request(
            &caller,
            TerminalLaunchProfileId::Codex,
            "Rename Foo to Bar".to_owned(),
            None,
        );
        assert_eq!(create.cwd, "/tmp/project/crates");
        assert_eq!(create.title.as_deref(), Some("Codex"));
        assert_eq!(
            create.launch.map(|launch| launch.profile_id),
            Some(TerminalLaunchProfileId::Codex)
        );
        assert!(create.launch_session.is_none());
        let handoff = create.handoff.expect("handoff");
        assert_eq!(handoff.kind, Some(TerminalHandoffKind::Brief));
        assert_eq!(handoff.source_terminal_id, caller.id);
        assert_eq!(handoff.transcript, "Rename Foo to Bar");
        assert_eq!(handoff.from_title.as_deref(), Some("Claude Code"));

        let titled = pass_off_request(
            &caller,
            TerminalLaunchProfileId::Gemini,
            "x".to_owned(),
            Some("  Rename job ".to_owned()),
        );
        assert_eq!(titled.title.as_deref(), Some("  Rename job "));
        let blank = pass_off_request(
            &caller,
            TerminalLaunchProfileId::Grok,
            "x".to_owned(),
            Some("  ".to_owned()),
        );
        assert_eq!(blank.title.as_deref(), Some("Grok"));
        assert!(
            blank
                .launch_session
                .as_ref()
                .is_some_and(|session| session.id.is_some() && !session.resume)
        );
        let sanitized = pass_off_request(
            &caller,
            TerminalLaunchProfileId::Codex,
            "x".to_owned(),
            Some(format!("Fix\nbug\u{7} {}", "x".repeat(140))),
        );
        let sanitized_title = sanitized.title.expect("sanitized title");
        assert!(sanitized_title.starts_with("Fix bug "));
        assert_eq!(sanitized_title.chars().count(), MAX_PANE_TITLE_CHARS);
        assert!(!sanitized_title.chars().any(char::is_control));

        let mut odd = caller;
        odd.title = "bad\u{1}title".to_owned();
        assert!(
            pass_off_request(&odd, TerminalLaunchProfileId::Claude, "x".to_owned(), None)
                .handoff
                .unwrap()
                .from_title
                .is_none()
        );
        assert_eq!(handoff_title(&"t".repeat(201)), None);
        assert_eq!(handoff_title("   "), None);

        assert_eq!(
            pass_off_profile("antigravity"),
            Ok(TerminalLaunchProfileId::Gemini)
        );
        assert_eq!(pass_off_profile("muse"), Ok(TerminalLaunchProfileId::Muse));
        assert!(pass_off_profile("shell").is_err());
        assert!(pass_off_profile("qwen").is_err());
        let names = tool_catalogue()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, PANE_TOOL_NAMES);
    }

    // Protocol v55: the host types a Kimi pass-off into its TUI, so the
    // pass_off tool offers Kimi Code by name.
    #[test]
    fn pass_off_tool_offers_kimi_code() {
        assert_eq!(pass_off_profile("kimi"), Ok(TerminalLaunchProfileId::Kimi));
        let pass_off = tool_catalogue()
            .into_iter()
            .find(|tool| tool["name"] == "pass_off")
            .expect("pass_off tool");
        assert!(
            pass_off["inputSchema"]["properties"]["profile"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("kimi"))
        );
    }

    #[test]
    fn send_to_pane_only_crosses_a_pass_off_edge_in_either_direction() {
        let source = caller();
        let mut delegate = caller();
        delegate.id = TerminalId::random();
        delegate.title = "Codex".to_owned();
        delegate.passed_off_from = Some(source.id);
        // The delegate reports back; the source nudges its delegate.
        assert!(may_send_to_pane(&delegate, &source));
        assert!(may_send_to_pane(&source, &delegate));

        let mut unrelated = caller();
        unrelated.id = TerminalId::random();
        assert!(!may_send_to_pane(&source, &unrelated));
        assert!(!may_send_to_pane(&delegate, &unrelated));
        assert!(!may_send_to_pane(&unrelated, &delegate));

        let mut shell = caller();
        shell.id = TerminalId::random();
        shell.title = "zsh".to_owned();
        shell.launch = None;
        assert!(!may_send_to_pane(&source, &shell));
        assert!(!may_send_to_pane(&delegate, &shell));
        // A pane is not on a pass-off edge with itself.
        assert!(!may_send_to_pane(&source, &source));
        // Reading adds only the caller's own pane to the same edge.
        assert!(may_read_pane(&source, &source));
        assert!(may_read_pane(&delegate, &source));
        assert!(may_read_pane(&source, &delegate));
        assert!(!may_read_pane(&source, &unrelated));
        assert!(!may_read_pane(&delegate, &shell));
    }

    fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A stand-in Grok: `mcp list --json` prints the state file (or `[]`),
    /// `mcp add` writes Interface's entry there, `mcp remove` deletes it;
    /// every call is logged.
    fn fake_grok(directory: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let state = directory.join("grok-mcp.json");
        let log = directory.join("grok-calls.log");
        let ours = serde_json::json!([{
            "command": "/bin/sh", "args": interface_terminal::pane_tools::PANE_TOOLS_ARGS,
            "enabled": true, "name": "iface", "scope": "user"
        }]);
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$1 $2\" in\n\
             'mcp list') [ -f '{state}' ] && cat '{state}' || printf '[]\\n' ;;\n\
             'mcp add') printf '%s\\n' '{ours}' > '{state}' ;;\n\
             'mcp remove') rm -f '{state}' ;;\nesac\n",
            log = log.display(),
            state = state.display(),
        );
        (script(directory, "grok", &body), state, log)
    }

    #[test]
    fn pane_tools_are_written_only_when_switched_on_and_only_interfaces_entry_is_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let (grok, state, log) = fake_grok(temporary.path());
        let cli = PaneToolsCli::Grok;
        let calls = || {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.starts_with("mcp list"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };

        // Reading never writes: a fresh install has no entry and stays so.
        assert_eq!(read_pane_tools_entry(cli, &grok), PaneToolsEntry::Absent);
        assert!(calls().is_empty());

        // On writes Interface's user entry; asking again changes nothing.
        assert_eq!(
            set_pane_tools_entry(cli, &grok, true),
            Ok(PaneToolsEntry::Ours { enabled: true })
        );
        assert_eq!(
            set_pane_tools_entry(cli, &grok, true),
            Ok(PaneToolsEntry::Ours { enabled: true })
        );
        assert_eq!(
            calls(),
            [format!(
                "mcp add --scope user iface -- /bin/sh -c {}",
                interface_terminal::pane_tools::PANE_TOOLS_ARGS[1]
            )]
        );

        // Off removes it: an entry an earlier host wrote is switched off the same way.
        assert_eq!(
            set_pane_tools_entry(cli, &grok, false),
            Ok(PaneToolsEntry::Absent)
        );
        assert_eq!(calls().last().unwrap(), "mcp remove --scope user iface");
        assert!(!state.exists());

        // The owner's own `iface` server is never replaced or removed.
        std::fs::write(
            &state,
            r#"[{"command":"/usr/local/bin/my-iface","args":[],"enabled":true,"name":"iface","scope":"user"}]"#,
        )
        .unwrap();
        let before = calls().len();
        assert_eq!(read_pane_tools_entry(cli, &grok), PaneToolsEntry::Foreign);
        assert_eq!(
            set_pane_tools_entry(cli, &grok, true),
            Err(PaneToolsChangeError::Taken)
        );
        assert_eq!(
            set_pane_tools_entry(cli, &grok, false),
            Ok(PaneToolsEntry::Foreign)
        );
        assert_eq!(calls().len(), before);
        assert!(state.exists());

        // A listing that cannot be read changes nothing either.
        std::fs::write(&state, "leader not running").unwrap();
        assert_eq!(
            set_pane_tools_entry(cli, &grok, false),
            Err(PaneToolsChangeError::Unreadable)
        );
        assert_eq!(calls().len(), before);
        let refusing = script(
            temporary.path(),
            "agy",
            "#!/bin/sh\ncase \"$2\" in list) echo 'No MCP servers configured.' ;; *) echo nope >&2; exit 3 ;; esac\n",
        );
        assert!(matches!(
            set_pane_tools_entry(PaneToolsCli::Antigravity, &refusing, true),
            Err(PaneToolsChangeError::Refused(message)) if message.contains("nope")
        ));

        let slow = script(temporary.path(), "slow", "#!/bin/sh\nsleep 5\n");
        let started = Instant::now();
        assert!(run_config_command(&slow, &[], Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        // A helper left holding the inherited stderr cannot hold the install
        // thread past the deadline either.
        let lingering = script(
            temporary.path(),
            "lingering",
            "#!/bin/sh\nsleep 5 &\nexit 0\n",
        );
        let started = Instant::now();
        assert!(run_config_command(&lingering, &[], Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            run_config_command(Path::new("/nonexistent/cli"), &[], Duration::from_secs(1)).is_err()
        );
    }
}

#[cfg(test)]
mod catalogue_tests {
    use super::*;
    #[test]
    fn pane_tools_name_only_ids_pane_sessions_can_obtain() {
        // Pane sessions have no app_state or list_panes; their pane ids come
        // from pass_off or the pass-off brief.
        for tool in tool_catalogue() {
            let text = tool.to_string();
            assert!(
                !text.contains("app_state") && !text.contains("list_panes"),
                "{text}"
            );
        }
    }
}
