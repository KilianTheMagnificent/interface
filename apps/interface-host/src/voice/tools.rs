//! The `iface_voice` tool implementations against the host-owned services.

use std::{collections::BTreeSet, path::Path, sync::Arc};

use async_trait::async_trait;
use base64::Engine as _;
use interface_contracts::{
    engine::{EngineEventKind, SessionSummary, session_key},
    ipc::{CHANNELS, HISTORY_LIMIT},
    terminal::{
        CreateTerminalRequest, TERMINAL_LAUNCH_REQUEST_VERSION, TerminalId,
        TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalSize, TerminalSnapshot,
        TerminalState, TerminalSummary,
    },
};
use interface_desktop_services::DesktopService;
use interface_runtime::SessionRuntime;
use interface_terminal::profile_label;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{VoiceShared, mcp::ToolOutput, push_voice_frame, ui_bridge::UiBridge};
use crate::{
    dispatcher::{DispatchError, HostDispatcher, RuntimeCommandHandler},
    mcp_transport::{Peer, ToolBackend},
    protocol::PushFrame,
    terminal::TerminalHostDispatcher,
};

const PANE_COLUMNS: u16 = 120;
const PANE_ROWS: u16 = 40;
const READ_PANE_DEFAULT: u64 = 60;
const READ_PANE_MAX: u64 = 400;
const READ_SESSION_DEFAULT: u64 = 10;
const READ_SESSION_MAX: u64 = 50;
const WAITING_TAIL_LINES: usize = 3;
/// The most sessions `app_state` and `list_sessions` return. The registry
/// keeps every session ever seen, over a thousand on the owner's machine and
/// about 0.5 MB of JSON, and both tools used to hand all of it over per call.
const SESSION_ROWS_MAX: usize = 50;

pub(crate) struct VoiceTools {
    pub(crate) runtime: Arc<SessionRuntime>,
    pub(crate) sessions: Arc<dyn HostDispatcher>,
    pub(crate) terminal: Arc<TerminalHostDispatcher>,
    pub(crate) desktop: Arc<DesktopService>,
    pub(crate) pushes: mpsc::Sender<PushFrame>,
    pub(crate) ui: Arc<UiBridge>,
    pub(crate) shared: Arc<VoiceShared>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartPane {
    profile: String,
    cwd: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendToPane {
    pub(crate) pane_id: String,
    text: String,
    #[serde(default = "default_true")]
    enter: bool,
    #[serde(default)]
    generation: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadPane {
    pane_id: String,
    #[serde(default)]
    lines: Option<u64>,
}

impl ReadPane {
    pub(crate) fn pane_id(&self) -> &str {
        &self.pane_id
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneId {
    pane_id: String,
    #[serde(default)]
    generation: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadSession {
    session_key: String,
    #[serde(default)]
    last_n: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSessions {
    #[serde(default)]
    query: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendToSession {
    session_key: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Notify {
    title: String,
    body: String,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl ToolBackend for VoiceTools {
    async fn call(&self, name: &str, input: Value, _peer: Peer) -> Result<ToolOutput, String> {
        match name {
            "app_state" => self.app_state().map(ToolOutput::Text),
            "list_panes" => self.list_panes().map(ToolOutput::Text),
            "start_pane" => self.start_pane(arguments(input)?).await,
            "list_profiles" => self
                .terminal
                .dispatch(CHANNELS.list_terminal_launch_profiles, json!({}))
                .await
                .map(|value| ToolOutput::Text(value.to_string()))
                .map_err(dispatch_message),
            "send_to_pane" => send_to_pane(&self.terminal, arguments(input)?).await,
            "read_pane" => read_pane(&self.terminal, arguments(input)?).await,
            "close_pane" => self.close_pane(arguments(input)?).await,
            "list_sessions" => self.list_sessions(&arguments(input)?).map(ToolOutput::Text),
            "read_session" => self.read_session(arguments(input)?).await,
            "send_to_session" => self.send_to_session(arguments(input)?).await,
            "notify" => self.notify(arguments(input)?).await,
            "ui_screenshot" => {
                let png_base64 = self.ui.call(name, input).await?;
                let text = png_dimensions(&png_base64).map_or_else(
                    || "captured".to_owned(),
                    |(w, h)| format!("captured {w}x{h}"),
                );
                Ok(ToolOutput::Image { png_base64, text })
            }
            "ui_snapshot" | "ui_act" | "open_project" | "focus_pane" => {
                self.ui.call(name, input).await.map(ToolOutput::Text)
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }
}

impl VoiceTools {
    fn app_state(&self) -> Result<String, String> {
        let (sessions, sessions_total) = recent_sessions(self.session_rows()?, None);
        let panes = self.pane_rows()?;
        let mut roots = self
            .desktop
            .terminal_workspace_roots()
            .into_iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>();
        roots.extend(
            sessions
                .iter()
                .filter_map(|session| session.project_root.clone()),
        );
        roots.extend(panes.iter().map(|(summary, _)| summary.root.clone()));
        let projects = roots
            .iter()
            .map(|root| {
                json!({
                    "root": root,
                    "name": Path::new(root).file_name().map_or_else(|| root.clone(), |name| name.to_string_lossy().into_owned()),
                    "session_keys": sessions
                        .iter()
                        .filter(|session| session.project_root.as_deref() == Some(root.as_str()) || &session.cwd == root)
                        .map(|session| session_key(&session.engine, &session.session_id))
                        .collect::<Vec<_>>(),
                    "pane_ids": panes
                        .iter()
                        .filter(|(summary, _)| &summary.root == root)
                        .map(|(summary, _)| summary.id.to_string())
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "active_project": self.shared.snapshot().cwd,
            "projects": projects,
            "sessions": sessions.iter().map(session_json).collect::<Vec<_>>(),
            "sessions_total": sessions_total,
            "panes": panes.iter().map(|(summary, waiting)| pane_json(summary, *waiting)).collect::<Vec<_>>(),
        })
        .to_string())
    }

    fn list_sessions(&self, request: &ListSessions) -> Result<String, String> {
        let (sessions, total) = recent_sessions(self.session_rows()?, request.query.as_deref());
        Ok(json!({
            "sessions": sessions.iter().map(session_json).collect::<Vec<_>>(),
            "total": total,
        })
        .to_string())
    }

    fn list_panes(&self) -> Result<String, String> {
        let panes = self.pane_rows()?;
        Ok(json!(
            panes
                .iter()
                .map(|(summary, waiting)| pane_json(summary, *waiting))
                .collect::<Vec<_>>()
        )
        .to_string())
    }

    async fn start_pane(&self, request: StartPane) -> Result<ToolOutput, String> {
        let profile_id = parse_profile(&request.profile)?;
        let create = CreateTerminalRequest {
            sign_in: false,
            id: TerminalId::random(),
            cwd: request.cwd,
            size: TerminalSize {
                columns: PANE_COLUMNS,
                rows: PANE_ROWS,
            },
            title: request.title,
            session_key: None,
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id,
            }),
            launch_session: None,
            handoff: None,
            account: None,
        };
        let payload = serde_json::to_value(create).map_err(|error| error.to_string())?;
        let summary: TerminalSummary = decode(
            self.terminal
                .dispatch(CHANNELS.create_terminal, payload)
                .await
                .map_err(dispatch_message)?,
        )?;
        Ok(ToolOutput::Text(pane_json(&summary, false).to_string()))
    }

    async fn close_pane(&self, request: PaneId) -> Result<ToolOutput, String> {
        let pane = find_pane(&self.terminal, &request.pane_id).await?;
        if request
            .generation
            .is_some_and(|generation| generation != pane.generation.0)
        {
            return Err("Pane generation changed; close was not sent.".into());
        }
        self.terminal
            .dispatch(
                CHANNELS.close_terminal,
                json!({"id": pane.id, "generation": pane.generation}),
            )
            .await
            .map_err(dispatch_message)?;
        Ok(ok())
    }

    async fn read_session(&self, request: ReadSession) -> Result<ToolOutput, String> {
        let last_n = request
            .last_n
            .unwrap_or(READ_SESSION_DEFAULT)
            .clamp(1, READ_SESSION_MAX);
        let page = RuntimeCommandHandler::read_provider_history(
            self.runtime.as_ref(),
            &request.session_key,
            HISTORY_LIMIT,
        )
        .await
        .map_err(|error| error.message)?;
        let mut turns = Vec::new();
        let mut assistant = String::new();
        for event in page.events {
            match event.event {
                EngineEventKind::UserMessage { text, .. } => {
                    if !assistant.trim().is_empty() {
                        turns.push(format!("assistant: {}", assistant.trim()));
                    }
                    assistant.clear();
                    turns.push(format!("user: {}", text.trim()));
                }
                EngineEventKind::MessageDelta {
                    text,
                    agent_key: None,
                } => assistant.push_str(&text),
                _ => {}
            }
        }
        if !assistant.trim().is_empty() {
            turns.push(format!("assistant: {}", assistant.trim()));
        }
        let skip = turns
            .len()
            .saturating_sub(usize::try_from(last_n).unwrap_or(usize::MAX));
        Ok(ToolOutput::Text(turns[skip..].join("\n")))
    }

    async fn send_to_session(&self, request: SendToSession) -> Result<ToolOutput, String> {
        self.sessions
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": request.session_key, "text": request.text}),
            )
            .await
            .map_err(dispatch_message)?;
        Ok(ok())
    }

    async fn notify(&self, request: Notify) -> Result<ToolOutput, String> {
        let mut event = self.shared.snapshot();
        event.message = Some(format!("{}: {}", request.title, request.body));
        // Not recorded: the next enable response must not repeat it.
        push_voice_frame(&self.pushes, &event).await;
        Ok(ok())
    }

    fn session_rows(&self) -> Result<Vec<SessionSummary>, String> {
        let mut sessions = RuntimeCommandHandler::session_summaries(self.runtime.as_ref())
            .map_err(|error| error.message)?;
        sessions.retain(|session| !session.archived);
        Ok(sessions)
    }

    fn pane_rows(&self) -> Result<Vec<(TerminalSummary, bool)>, String> {
        let snapshot = pane_snapshot(&self.terminal)?;
        Ok(snapshot
            .terminals
            .into_iter()
            .map(|summary| {
                let waiting = summary.state == TerminalState::Running
                    && self
                        .terminal
                        .read_tail(summary.id, summary.generation, WAITING_TAIL_LINES)
                        .is_some_and(|tail| looks_waiting(&tail));
                (summary, waiting)
            })
            .collect())
    }
}

/// Types text into a pane exactly as renderer keystrokes arrive. Shared with
/// the pane server, where a passed-off agent uses it to report back.
pub(crate) async fn send_to_pane(
    terminal: &TerminalHostDispatcher,
    request: SendToPane,
) -> Result<ToolOutput, String> {
    let pane = find_pane(terminal, &request.pane_id).await?;
    if request
        .generation
        .is_some_and(|generation| generation != pane.generation.0)
    {
        return Err("Pane generation changed; input was not sent.".into());
    }
    if pane.state != TerminalState::Running {
        return Err("Pane is not running; input was not sent.".into());
    }
    let mut bytes = request.text.into_bytes();
    if request.enter {
        bytes.push(b'\r');
    }
    terminal
        .write_input(pane.id, pane.generation, bytes)
        .map_err(dispatch_message)?;
    Ok(ok())
}

/// The last lines of a pane's rendered output. Shared with the pane server.
pub(crate) async fn read_pane(
    terminal: &TerminalHostDispatcher,
    request: ReadPane,
) -> Result<ToolOutput, String> {
    let pane = find_pane(terminal, &request.pane_id).await?;
    let lines = request
        .lines
        .unwrap_or(READ_PANE_DEFAULT)
        .clamp(1, READ_PANE_MAX);
    let lines = usize::try_from(lines).unwrap_or(usize::MAX);
    Ok(ToolOutput::Text(
        terminal
            .read_tail(pane.id, pane.generation, lines)
            .unwrap_or_else(|| "(no output yet)".to_owned()),
    ))
}

pub(crate) fn pane_snapshot(terminal: &TerminalHostDispatcher) -> Result<TerminalSnapshot, String> {
    terminal
        .visible_terminal_snapshot()
        .map_err(dispatch_message)
}

pub(crate) async fn find_pane(
    terminal: &TerminalHostDispatcher,
    pane_id: &str,
) -> Result<TerminalSummary, String> {
    let id = parse_pane_id(pane_id)?;
    terminal
        .visible_terminal(id)
        .map_err(dispatch_message)?
        .ok_or_else(|| format!("unknown pane id {pane_id}"))
}

fn parse_pane_id(pane_id: &str) -> Result<TerminalId, String> {
    uuid::Uuid::parse_str(pane_id)
        .map(TerminalId::new)
        .map_err(|_| format!("unknown pane id {pane_id}"))
}

pub(crate) fn arguments<T: DeserializeOwned>(input: Value) -> Result<T, String> {
    serde_json::from_value(input).map_err(|error| format!("invalid arguments: {error}"))
}

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("unexpected host result: {error}"))
}

pub(crate) fn dispatch_message(error: DispatchError) -> String {
    let DispatchError { code, message, .. } = error;
    format!("{code}: {message}")
}

fn ok() -> ToolOutput {
    ToolOutput::Text(json!({"ok": true}).to_string())
}

pub(crate) fn parse_profile(name: &str) -> Result<TerminalLaunchProfileId, String> {
    Ok(match name {
        "shell" => TerminalLaunchProfileId::Shell,
        "claude" => TerminalLaunchProfileId::Claude,
        "codex" => TerminalLaunchProfileId::Codex,
        "grok" => TerminalLaunchProfileId::Grok,
        "qwen" => TerminalLaunchProfileId::Qwen,
        "antigravity" | "gemini" => TerminalLaunchProfileId::Gemini,
        "omp" => TerminalLaunchProfileId::Omp,
        "opencode" => TerminalLaunchProfileId::Opencode,
        "muse" => TerminalLaunchProfileId::Muse,
        "kimi" => TerminalLaunchProfileId::Kimi,
        other => return Err(format!("unknown profile {other}")),
    })
}

fn profile_name(profile: TerminalLaunchProfileId) -> &'static str {
    match profile {
        TerminalLaunchProfileId::Gemini => "antigravity",
        other => other.as_str(),
    }
}

pub(crate) fn pane_json(summary: &TerminalSummary, waiting: bool) -> Value {
    let profile = summary
        .launch
        .map_or(TerminalLaunchProfileId::Shell, |launch| launch.profile_id);
    json!({
        "pane_id": summary.id.to_string(),
        "generation": summary.generation.0,
        "profile": profile_name(profile),
        "label": profile_label(profile),
        "cwd": summary.cwd,
        "title": summary.title,
        "state": summary.state,
        "waiting": waiting,
    })
}

/// The sessions the assistant is shown: those matching `query` by title,
/// folder or engine (ignoring case), the most recently active first, at most
/// [`SESSION_ROWS_MAX`] of them. The count is every session that matched.
fn recent_sessions(
    mut sessions: Vec<SessionSummary>,
    query: Option<&str>,
) -> (Vec<SessionSummary>, usize) {
    if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
        let query = query.to_lowercase();
        sessions.retain(|session| {
            [
                Some(session.title.as_str()),
                Some(session.cwd.as_str()),
                session.project_root.as_deref(),
                Some(session.engine.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&query))
        });
    }
    let total = sessions.len();
    sessions.sort_by(|left, right| right.last_activity.cmp(&left.last_activity));
    sessions.truncate(SESSION_ROWS_MAX);
    (sessions, total)
}

fn session_json(session: &SessionSummary) -> Value {
    json!({
        "session_key": session_key(&session.engine, &session.session_id),
        "title": session.title,
        "engine": session.engine.as_str(),
        "cwd": session.cwd,
        "project_root": session.project_root,
        "status": session.status,
        "kind": session.kind,
    })
}

/// ponytail: a pane "waits" when its last non-empty line ends like a shell
/// or REPL prompt. Good enough for the assistant to say which pane is idle.
fn looks_waiting(tail: &str) -> bool {
    tail.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            let line = line.trim_end();
            ['$', '%', '>', '#', '❯']
                .iter()
                .any(|prompt| line.ends_with(*prompt))
        })
}

/// Width and height from a base64 PNG's IHDR chunk.
fn png_dimensions(png_base64: &str) -> Option<(u32, u32)> {
    let head = png_base64.get(..png_base64.len().min(44))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(head.trim_end_matches('='))
        .ok()?;
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG") {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_round_trip_and_antigravity_maps_to_the_gemini_wire_id() {
        for name in [
            "shell",
            "claude",
            "codex",
            "grok",
            "qwen",
            "antigravity",
            "omp",
            "opencode",
            "muse",
            "kimi",
        ] {
            assert_eq!(profile_name(parse_profile(name).unwrap()), name);
        }
        assert_eq!(
            parse_profile("antigravity").unwrap(),
            TerminalLaunchProfileId::Gemini
        );
        assert!(parse_profile("bash").is_err());
        assert!(looks_waiting("ls\nfoo bar\n~/x $ "));
        assert!(!looks_waiting("compiling...\n"));
    }

    #[test]
    fn pane_labels_are_the_terminal_catalog_labels_this_module_used_to_copy() {
        for (profile, label) in [
            (TerminalLaunchProfileId::Shell, "Terminal"),
            (TerminalLaunchProfileId::Claude, "Claude Code"),
            (TerminalLaunchProfileId::Codex, "Codex"),
            (TerminalLaunchProfileId::Grok, "Grok"),
            (TerminalLaunchProfileId::Qwen, "Qwen"),
            (TerminalLaunchProfileId::Gemini, "Antigravity"),
            (TerminalLaunchProfileId::Omp, "OMP"),
            (TerminalLaunchProfileId::Opencode, "OpenCode"),
            (TerminalLaunchProfileId::Muse, "Muse Code"),
            (TerminalLaunchProfileId::Kimi, "Kimi Code"),
        ] {
            assert_eq!(profile_label(profile), label);
        }
    }

    fn session(title: &str, cwd: &str, engine: &str, at: &str) -> SessionSummary {
        SessionSummary {
            engine: interface_contracts::engine::EngineId::new(engine).unwrap(),
            session_id: format!("{title}-{at}"),
            title: title.into(),
            cwd: cwd.into(),
            status: interface_contracts::engine::SessionStatus::Idle,
            cloud: false,
            model: None,
            effort: None,
            last_activity: at.into(),
            first_seen: "2026-01-01T00:00:00.000Z".into(),
            archived: false,
            size: None,
            title_from_summary: None,
            origin: None,
            project_root: Some(cwd.into()),
            worktree_home: None,
            worktree_path: None,
            setting_sources: None,
            disallowed_tools: None,
            kind: None,
            permission_mode: None,
        }
    }

    #[test]
    fn the_assistant_sees_the_fifty_most_recent_sessions_and_the_full_count() {
        let sessions = (0..1100)
            .map(|index| {
                session(
                    &format!("Session {index}"),
                    "/work/project",
                    "claude",
                    &format!("2026-09-{:02}T{:02}:00:00.000Z", 1 + index % 28, index % 24),
                )
            })
            .collect::<Vec<_>>();
        let mut newest = sessions
            .iter()
            .map(|session| session.last_activity.clone())
            .collect::<Vec<_>>();
        newest.sort_unstable_by(|left, right| right.cmp(left));
        let (shown, total) = recent_sessions(sessions, None);
        assert_eq!(total, 1100);
        assert_eq!(shown.len(), SESSION_ROWS_MAX);
        assert_eq!(
            shown
                .iter()
                .map(|session| session.last_activity.clone())
                .collect::<Vec<_>>(),
            newest[..SESSION_ROWS_MAX]
        );
        let (blank, total) = recent_sessions(shown.clone(), Some("   "));
        assert_eq!((blank, total), (shown, SESSION_ROWS_MAX));
    }

    #[test]
    fn a_query_finds_older_sessions_by_title_folder_or_engine() {
        let mut sessions = (0..80)
            .map(|index| {
                session(
                    "Busy work",
                    "/work/busy",
                    "claude",
                    &format!("2026-09-02T00:{:02}:00.000Z", index % 60),
                )
            })
            .collect::<Vec<_>>();
        sessions.push(session(
            "Updater feed",
            "/work/interface",
            "claude",
            "2026-01-01T00:00:00.000Z",
        ));
        sessions.push(session(
            "Notes",
            "/work/Kite",
            "claude",
            "2026-01-02T00:00:00.000Z",
        ));
        sessions.push(session(
            "Notes",
            "/work/other",
            "codex",
            "2026-01-03T00:00:00.000Z",
        ));
        let titles = |query| {
            let (shown, total) = recent_sessions(sessions.clone(), Some(query));
            let titles = shown
                .iter()
                .map(|session| format!("{} {}", session.title, session.cwd))
                .collect::<Vec<_>>();
            (titles, total)
        };
        assert!(
            !recent_sessions(sessions.clone(), None)
                .0
                .iter()
                .any(|session| session.title != "Busy work")
        );
        assert_eq!(
            titles("updater"),
            (vec!["Updater feed /work/interface".to_owned()], 1)
        );
        assert_eq!(titles(" kite "), (vec!["Notes /work/Kite".to_owned()], 1));
        assert_eq!(titles("CODEX"), (vec!["Notes /work/other".to_owned()], 1));
        let (busy, total) = titles("busy");
        assert_eq!((busy.len(), total), (SESSION_ROWS_MAX, 80));
    }

    #[test]
    fn png_dimensions_read_the_ihdr_chunk() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&1280_u32.to_be_bytes());
        png.extend_from_slice(&800_u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(png_dimensions(&encoded), Some((1280, 800)));
        assert_eq!(png_dimensions("bm90IGEgcG5n"), None);
    }
}
