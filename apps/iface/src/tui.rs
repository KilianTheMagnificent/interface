mod agent_control;
mod approval;
mod collaboration;
mod commands;
mod controller;
mod effects;
mod orchestration;
mod reducer;
mod settings;
mod terminal;
use approval::PendingApproval;
use commands::{app_command_takes_argument, is_handled_app_command, parsed_app_command};
use effects::{EffectOutcome, Effects, HeldInput, is_quit_key};
use reducer::{
    context_telemetry_sample, controlled_from_live_summary, controlled_from_summary,
    merge_registry_summary, push_message, push_notice, reduce_event, replace_provider_history,
    scope_from_setting_sources,
};
pub(crate) use terminal::ShutdownSignals;
use terminal::{ShutdownSignalAction, TerminalSession, accept_shutdown_signal, run_terminal_loop};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::Duration,
};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use interface_contracts::engine::{
    ContextUsage, EngineEvent, EngineEventKind, EngineId, EngineInfo, LaunchOptions, ModelOption,
    NoticeTone, PlanEntry, PlanEntryPriority, PlanEntryStatus, ProviderCommand,
    ProviderCommandCatalogue, ProviderCommandScope, ResumeOptions, SessionStatus, SessionSummary,
    context_auto_compact_remaining, context_usage_change, context_window_remaining,
    format_context_tokens,
};
use interface_contracts::ipc::{
    CHANNELS, CollaborationDraftStateUpdate, CollaborationEventDto, CollaborationMode,
    CollaborationState, CollaborationStatusDto, HISTORY_MAX_LIMIT,
};
use interface_contracts::provider_commands::{
    app_owned_provider_command_error, is_app_owned_provider_command, parse_provider_command_like,
    provider_command_draft,
};
use interface_engine::DeliveryOutcome;
use interface_host::{
    OrchestrationHostControl, PreparedOrchestrationHost, collaboration::ProjectedSendOutcome,
    protocol::PushFrame,
};
use interface_runtime::{ProviderReadiness, RuntimeEventFrame, SessionOpenOutcome, SessionRuntime};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::{
    agent_protocol::{
        AGENT_RESULT_BUDGET_BYTES, AgentCapabilities, AgentErrorCode, AgentProtocolError,
        AgentStatus, AgentTaskMetadata, encoded_len,
    },
    agent_server::{
        AgentController, AgentDelivery, AgentDeliveryKind, AgentSessionState, StartAgentInput,
        start_agent_server,
    },
    app::canonical_directory,
    argv::{CliArgv, LaunchScope, TuiMode},
    collaboration_runtime::CliCollaborationRuntime,
    composer::{self, Composer},
    config::CliConfigStore,
    diagnostics::DiagnosticsSink,
    history::PromptHistory,
    paths::CliPaths,
    sanitize::for_display,
    steer_history::{
        MAX_SESSION_STEER_HISTORY_ENTRIES, SteerHistoryEntry, SteerHistoryStore, SteerOutcome,
        SteerSettledOutcome,
    },
};

const TRANSCRIPT_LIMIT: usize = 10_000;
const QUEUED_PROMPT_LIMIT: usize = 64;
const QUEUED_FAILURE_LIMIT: usize = 32;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const PICKER_MATCH_LIMIT: usize = 200;
const MAX_ACTIVITY_CHARACTERS: usize = 96;
const HELP_TEXT: &str = "Enter sends when idle and queues while busy.
Ctrl+T steers only providers that advertise mid-turn steering.
Esc interrupts. A/D answer plain approvals; questions use 1-4 and Enter.
/sessions /cwd /engine /model /effort /fast /access /consult /handoff /collab /peer /steers /advisor /agents /plan /orchestrate /spawn /send /inbox /monitor /graph /doctor /commands /queue /details /window /resume /new /quit
Precise role and evidence authoring uses the stable one-shot iface orchestrate command. Worktree escrow is unavailable in iface.
Fullscreen: ↑/↓ scroll, End follow, Ctrl+Y dump to native scrollback.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellTone {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ToolResultCell {
    ok: bool,
    summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TranscriptCell {
    Message {
        role: &'static str,
        markdown: String,
    },
    ImageAttachments {
        count: usize,
    },
    Peer {
        source_engine: String,
        objective: String,
        markdown: String,
    },
    Tool {
        call_id: String,
        tool: String,
        summary: String,
        input_preview: String,
        result: Option<ToolResultCell>,
        command_activity: bool,
    },
    Diff {
        call_id: String,
        path: String,
        before: Option<String>,
        after: Option<String>,
        withheld: bool,
        /// Text is missing: a side was cut to `DIFF_SIDE_LIMIT_BYTES`, or,
        /// with both sides `None`, the transcript released it to stay within
        /// its diff text budget.
        truncated: bool,
    },
    Notice {
        message: String,
        tone: CellTone,
    },
    Compact {
        trigger: String,
        before_tokens: Option<u64>,
        after_tokens: Option<u64>,
        ok: Option<bool>,
    },
    Retry {
        message: String,
        attempt: u64,
        max_attempts: u64,
        delay_ms: Option<u64>,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    Subagent {
        agent_key: String,
        label: String,
        description: String,
        model: Option<String>,
        result: Option<ToolResultCell>,
    },
}

#[derive(Clone, Debug)]
struct ProviderProbe {
    engine: EngineId,
    info: Option<EngineInfo>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct CommandCatalogue {
    known: bool,
    commands: Vec<ProviderCommand>,
}

fn catalogue_from<E>(result: Result<ProviderCommandCatalogue, E>) -> CommandCatalogue {
    result.map_or_else(
        |_error| CommandCatalogue {
            known: false,
            commands: Vec::new(),
        },
        |catalogue| CommandCatalogue {
            known: catalogue.available,
            commands: catalogue.commands,
        },
    )
}

#[derive(Clone, Debug)]
struct ControlledSession {
    state: AgentSessionState,
    transcript: Transcript,
    queued: VecDeque<String>,
    pending_approval: Option<PendingApproval>,
    effort: Option<String>,
    access: Option<String>,
    fast_mode: bool,
    models: Vec<ModelOption>,
    commands: CommandCatalogue,
    setting_sources: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    context: Option<ContextUsage>,
    previous_context: Option<ContextUsage>,
    last_activity: String,
    telemetry_startup_recorded: bool,
    telemetry_pending: bool,
    telemetry_recorded: Option<(u64, u64)>,
}

/// What `/cwd` found off the drawing worker: the typed directory itself, or
/// the completion candidates inside a directory.
enum CwdScan {
    Directory(PathBuf),
    Candidates(PathBuf, Vec<PathBuf>),
}

#[derive(Clone, Debug)]
struct LaunchDefaults {
    engine: String,
    cwd: String,
    model: Option<String>,
    effort: Option<String>,
    access: Option<String>,
    scope: LaunchScope,
    fast_mode: bool,
}

/// A session to start: Enter fills it from the launch defaults, the agent
/// socket's `start` from its request.
struct NewSession {
    engine: String,
    cwd: String,
    prompt: String,
    model: Option<String>,
    effort: Option<String>,
    access: Option<String>,
    fast_mode: bool,
    scope: LaunchScope,
    name: Option<String>,
    task: Option<AgentTaskMetadata>,
}

#[derive(Default)]
struct ControllerState {
    sessions: BTreeMap<String, ControlledSession>,
    active: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationRuntimeEventPush {
    key: String,
    event: EngineEvent,
}

#[derive(Clone)]
pub struct TuiController {
    runtime: SessionRuntime,
    collaboration: Option<CliCollaborationRuntime>,
    orchestration: Option<OrchestrationHostControl>,
    diagnostics: Option<DiagnosticsSink>,
    state: Arc<RwLock<ControllerState>>,
    launch: Arc<RwLock<LaunchDefaults>>,
    probes: Arc<RwLock<BTreeMap<String, ProviderProbe>>>,
    launch_commands: Arc<RwLock<CommandCatalogue>>,
    launch_command_revision: Arc<AtomicU64>,
    prompt_history: PromptHistory,
    steer_history: SteerHistoryStore,
    notifications: Arc<RwLock<VecDeque<TerminalNotification>>>,
    queued_failures: Arc<RwLock<VecDeque<QueuedSendFailure>>>,
    /// The last draft revision projected. Held while projecting, so callers
    /// reach the projection in the order they were numbered.
    draft_revision: Arc<Mutex<u64>>,
    revision: watch::Sender<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalNotification {
    title: String,
    body: String,
}

/// A queued prompt whose automatic send did not reach the provider. The
/// controller records it; the UI decides whether the text can go back to the
/// composer or has to be surfaced in the transcript instead.
#[derive(Clone, Debug, Eq, PartialEq)]
struct QueuedSendFailure {
    key: String,
    text: String,
    kind: AgentDeliveryKind,
    message: Option<String>,
}

impl QueuedSendFailure {
    fn reason(&self) -> String {
        self.message.clone().unwrap_or_else(|| {
            if self.kind == AgentDeliveryKind::DefinitelyUnsent {
                "The provider was not reached.".into()
            } else {
                "The provider may or may not have received it.".into()
            }
        })
    }

    fn transcript_notice(&self) -> String {
        if self.kind == AgentDeliveryKind::DefinitelyUnsent {
            format!(
                "Queued prompt was not sent. {} Nothing reached the provider — copy it and send it again:\n{}",
                self.reason(),
                self.text
            )
        } else {
            format!(
                "Queued prompt was not resent. {} Interface will not send it twice — copy it if the provider never answered:\n{}",
                self.reason(),
                self.text
            )
        }
    }

    fn composer_notice(&self) -> String {
        format!(
            "Queued prompt was not sent; it is back in the composer. {}",
            self.reason()
        )
    }

    fn deferred_notice(&self) -> String {
        if self.kind == AgentDeliveryKind::DefinitelyUnsent {
            "A queued prompt was not sent. It is in the transcript, not the composer, so your draft is untouched.".into()
        } else {
            "A queued prompt may already have reached the provider. It is in the transcript, unsent, so nothing is sent twice.".into()
        }
    }
}

/// Records an auto-send that did not reach the provider. Confirmed sends carry
/// nothing back to the UI.
fn queued_send_failure(
    key: &str,
    text: String,
    delivery: &AgentDelivery,
) -> Option<QueuedSendFailure> {
    (delivery.kind != AgentDeliveryKind::AdapterAccepted).then(|| QueuedSendFailure {
        key: key.to_owned(),
        text,
        kind: delivery.kind,
        message: delivery.message.clone(),
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TuiView {
    engine: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    access: Option<String>,
    fast_mode: bool,
    cwd: Option<String>,
    status: Option<AgentStatus>,
    context: Option<ContextUsage>,
    previous_context: Option<ContextUsage>,
}

use interface_contracts::command_catalog::APP_COMMANDS;

#[derive(Clone, Debug, Eq, PartialEq)]
enum PaletteAction {
    ResumeSession(String),
    SetCwd(String),
    SetEngine(String),
    SetModel {
        engine: String,
        model: Option<String>,
    },
    SetEffort(Option<String>),
    SetFast(bool),
    SetAccess(String),
    SetDraft(String),
    ToolDetails(String),
    SteerDetails(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PickerItem {
    id: String,
    label: String,
    description: Option<String>,
    badge: Option<String>,
    action: PaletteAction,
    preview: Option<String>,
    haystack: String,
    label_lower: String,
}

impl PickerItem {
    fn new(
        id: String,
        label: String,
        description: Option<String>,
        badge: Option<String>,
        action: PaletteAction,
        preview: Option<String>,
    ) -> Self {
        let haystack = format!(
            "{} {} {}",
            label,
            badge.as_deref().unwrap_or_default(),
            description.as_deref().unwrap_or_default()
        )
        .to_ascii_lowercase();
        let label_lower = label.to_ascii_lowercase();
        Self {
            id,
            label,
            description,
            badge,
            action,
            preview,
            haystack,
            label_lower,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaletteKind {
    Sessions,
    Cwd,
    Engine,
    Models,
    Effort,
    Fast,
    Access,
    Commands,
    Tools,
    Steers,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PaletteState {
    kind: PaletteKind,
    title: String,
    items: Vec<PickerItem>,
    query: String,
    active_index: usize,
    preview: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OverlayState {
    Help,
    Picker(PaletteState),
    Details {
        title: String,
        lines: Vec<String>,
        scroll: u16,
        /// Where each line's wrapped rows start, kept for the width it was
        /// measured at; the lines never change once shown.
        layout: Option<DetailsLayout>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DetailsLayout {
    width: u16,
    row_starts: Vec<usize>,
}

/// The styled, exactly wrapped transcript at one width. Cells are restyled
/// only from the first change the transcript reports, and a frame renders
/// only the lines in view.
struct TranscriptLayout {
    /// `(transcript id, version)` laid out; `None` for the welcome text.
    source: Option<(u64, u64)>,
    inner_width: u16,
    units: Vec<LayoutUnit>,
    lines: Vec<Line<'static>>,
    /// Wrapped rows before each line, plus the total as the last entry.
    row_starts: Vec<usize>,
    /// The top row that shows the newest rows, as last drawn.
    maximum: usize,
}

/// Where the fullscreen transcript view sits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TranscriptScroll {
    /// The newest rows stay in view as output arrives (End).
    #[default]
    Follow,
    /// Scrolled back: the row `rows` into the display unit that starts at
    /// absolute cell `cell` of transcript `transcript` stays at the top, so
    /// the rows in view hold still while output arrives or old cells drop.
    Held {
        transcript: u64,
        cell: u64,
        rows: usize,
    },
}

/// One display unit: a cell, or a run of command-activity tool cells that
/// render as one summary (and may grow while the run continues).
struct LayoutUnit {
    /// Absolute transcript index of the unit's first cell.
    first: u64,
    cells: usize,
    command_run: bool,
    lines: usize,
}

struct TuiApp {
    controller: TuiController,
    config: CliConfigStore,
    input: Composer,
    delivery_unknown: Option<UnknownDelivery>,
    notice: Option<String>,
    overlay: Option<OverlayState>,
    slash_index: usize,
    slash_dismissed: bool,
    pending_access_confirmation: Option<(String, String, Option<String>)>,
    logs: PathBuf,
    diagnostics: Option<DiagnosticsSink>,
    scroll: TranscriptScroll,
    /// Ctrl+Y's plain transcript, written to native scrollback before the
    /// next frame.
    scrollback_dump: Option<String>,
    mode: TuiMode,
    focused: bool,
    quit: bool,
    transcript_layout: Option<TranscriptLayout>,
    effects: Effects,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UnknownDelivery {
    /// The session it was sent to; `None` for the first prompt of a start
    /// that never attached, whose resend starts a session again.
    key: Option<String>,
    text: String,
    message: String,
}

impl TuiApp {
    /// Drains the auto-sends that never reached the provider. A prompt proven
    /// unsent goes back to the composer when that cannot overwrite anything the
    /// user typed or retarget it at another session; otherwise — and always for
    /// an unknown outcome, which could still be in flight — the text is kept in
    /// the transcript for the user to copy rather than resent.
    fn apply_queued_send_failures(&mut self) {
        for failure in self.controller.take_queued_send_failures() {
            let restorable = failure.kind == AgentDeliveryKind::DefinitelyUnsent
                && self.input.is_empty()
                && self.delivery_unknown.is_none()
                && self.controller.active_key().as_deref() == Some(failure.key.as_str());
            if restorable {
                self.input.replace(failure.text.clone());
                self.project_composer_state();
                self.notice = Some(failure.composer_notice());
            } else {
                self.controller.note_unsent_queued_prompt(&failure);
                self.notice = Some(failure.deferred_notice());
            }
        }
    }

    async fn submit(&mut self) {
        if self.delivery_unknown.is_some() {
            return;
        }
        if self.input.is_empty() {
            return;
        }
        let text = self.input.take();
        self.project_composer_state();
        self.slash_index = 0;
        self.slash_dismissed = false;
        if text.trim().is_empty() {
            return;
        }
        if self.collaboration_input_blocked(&text).await {
            self.restore_prompt(text);
            return;
        }
        if self.handle_app_command(&text).await {
            return;
        }
        if let Some(problem) = self.provider_command_problem(&text) {
            self.notice = Some(problem);
            self.restore_prompt(text);
            return;
        }
        if let Some(key) = self.controller.active_key() {
            let controller = self.controller.clone();
            self.queue_effect("Sending…", async move {
                let result = controller.send_agent(&key, text.clone()).await;
                EffectOutcome::Send { key, text, result }
            });
        } else {
            self.queue_start(text);
        }
    }

    /// Starts a new session with `text` as its first prompt, off the key
    /// handler. Also the resend of a start whose outcome is unknown.
    fn queue_start(&mut self, text: String) {
        let controller = self.controller.clone();
        let launch = self.controller.launch();
        self.queue_effect(
            format!("Starting {}…", engine_display_name(&launch.engine)),
            async move {
                let result = controller
                    .open_new_classified(NewSession {
                        engine: launch.engine,
                        cwd: launch.cwd,
                        prompt: text.clone(),
                        model: launch.model,
                        effort: launch.effort,
                        access: launch.access,
                        fast_mode: launch.fast_mode,
                        scope: launch.scope,
                        name: None,
                        task: None,
                    })
                    .await;
                EffectOutcome::Start { text, result }
            },
        );
    }

    /// Puts a prompt that never reached a provider back in the composer, and
    /// re-projects the composer's draft state. The refusal notice is kept.
    fn restore_prompt(&mut self, text: String) {
        let notice = self.notice.take();
        self.input.replace(text);
        self.project_composer_state();
        if notice.is_some() {
            self.notice = notice;
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_app_command(&mut self, text: &str) -> bool {
        let Some((name, argument)) = parsed_app_command(text) else {
            return false;
        };
        if !is_handled_app_command(&name) {
            return false;
        }
        match name.as_str() {
            "quit" | "exit" => self.quit = true,
            "help" => self.overlay = Some(OverlayState::Help),
            "new" => {
                self.controller.set_active(None);
                {
                    let mut launch = write_lock(&self.controller.launch);
                    launch.fast_mode = false;
                    launch.access = self.config.get().access_mode_for_launch(&launch.engine);
                }
                self.controller.refresh_launch_commands().await;
                self.notice = Some("Choose engine/model/access, then enter a prompt.".into());
            }
            "sessions" => self.open_session_picker(),
            "resume" => self.resume_command(&argument),
            "stop" => match self.controller.active_key() {
                Some(key) => self.queue_stop(key, true),
                None => self.notice = Some("No active session.".into()),
            },
            "cwd" => self.cwd_command(&argument).await,
            "engine" => self.engine_command(&argument).await,
            "model" | "models" | "set-model" => {
                if argument.is_empty() {
                    self.open_model_picker();
                } else {
                    self.set_model((argument != "default").then_some(argument))
                        .await;
                }
            }
            "effort" | "set-effort" | "reasoning-effort" => {
                if argument.is_empty() {
                    self.open_effort_picker();
                } else {
                    self.set_effort((argument != "default").then_some(argument))
                        .await;
                }
            }
            "fast" | "fast-mode" => {
                if argument.is_empty() {
                    self.open_fast_picker();
                } else {
                    let enabled = match argument.as_str() {
                        "on" => Some(true),
                        "off" => Some(false),
                        "toggle" => Some(!self.controller.launch().fast_mode),
                        _ => None,
                    };
                    if let Some(enabled) = enabled {
                        self.set_fast_mode(enabled).await;
                    } else {
                        self.notice =
                            Some("Use /fast, /fast on, /fast off, or /fast toggle.".into());
                    }
                }
            }
            "access" | "access-mode" | "permission" | "permissions" | "permission-mode"
            | "set-access" | "set-permission" => {
                if argument.is_empty() {
                    self.open_access_picker();
                } else {
                    self.set_access(argument).await;
                }
            }
            "commands" => self.open_command_picker(""),
            "consult" => {
                self.run_collaboration(CollaborationMode::Consult, &argument, false)
                    .await;
            }
            "handoff" => {
                self.run_collaboration(CollaborationMode::Handoff, &argument, false)
                    .await;
            }
            "collab" => self.collaboration_command(&argument).await,
            "peer" => self.peer_command(&argument).await,
            "steers" => self.open_steer_history(),
            "details" => self.open_tool_picker(),
            "window" => self.show_context_window(),
            "notifications" => self.notifications_command(&argument).await,
            "diagnostics" => self.show_diagnostics(),
            "doctor" => self.doctor_command().await,
            "advisor" => self.advisor_command(&argument).await,
            "agents" => self.agents_command(&argument).await,
            "queue" => self.queue_command(&argument),
            "orchestrate" => self.orchestration_status_command().await,
            "spawn" => self.spawn_orchestration_agent(&argument).await,
            "send" => self.send_orchestration_message(&argument).await,
            "inbox" => self.orchestration_inbox_command(&argument).await,
            "monitor" => self.monitor_orchestration(&argument).await,
            _ => self.notice = Some(format!("Unknown command: /{name}")),
        }
        true
    }

    #[allow(clippy::cast_precision_loss)]
    fn show_context_window(&mut self) {
        let view = self.controller.view();
        let Some(usage) = view
            .context
            .filter(|usage| usage.used.is_some() && usage.size.is_some())
        else {
            self.notice = Some("This provider has not reported session-window occupancy.".into());
            return;
        };
        let used = usage.used.unwrap_or_default();
        let size = usage.size.unwrap_or_default();
        let percent = usage
            .percent
            .map_or_else(|| "—".into(), |value| format!("{value:.0}%"));
        let mut lines = vec![format!(
            "Active window  {} / {} · {percent}",
            format_context_tokens(used as f64),
            format_context_tokens(size as f64)
        )];
        if let Some(remaining) = context_window_remaining(&usage) {
            lines.push(format!(
                "Window remaining  {}",
                format_context_tokens(remaining as f64)
            ));
        }
        if let Some(change) = context_usage_change(view.previous_context.as_ref(), &usage) {
            let sign = match change.cmp(&0) {
                std::cmp::Ordering::Greater => "+",
                std::cmp::Ordering::Less => "-",
                std::cmp::Ordering::Equal => "",
            };
            lines.push(format!(
                "Since prior report  {sign}{}",
                format_context_tokens(change.unsigned_abs() as f64)
            ));
        }
        if let Some(categories) = &usage.categories {
            lines.extend(categories.iter().map(|category| {
                format!(
                    "{}  {}",
                    for_display(&category.name),
                    format_context_tokens(category.tokens as f64)
                )
            }));
        }
        if let Some(enabled) = usage.auto_compact {
            lines.push(format!(
                "Auto-compact  {}",
                if enabled { "on" } else { "off" }
            ));
        }
        if let Some(threshold) = usage.auto_compact_threshold {
            let suffix = context_auto_compact_remaining(&usage).map_or_else(String::new, |left| {
                if left == 0 {
                    " · threshold reached".into()
                } else {
                    format!(" · {} remaining", format_context_tokens(left as f64))
                }
            });
            lines.push(format!(
                "Auto-compact threshold  {}{suffix}",
                format_context_tokens(threshold as f64)
            ));
        }
        let can_compact = self
            .controller
            .command_catalogue()
            .commands
            .iter()
            .any(|command| {
                command.name.eq_ignore_ascii_case("compact")
                    || command
                        .aliases
                        .iter()
                        .flatten()
                        .any(|alias| alias.eq_ignore_ascii_case("compact"))
            });
        if can_compact {
            lines.push(format!(
                "Run /compact to ask {} to compact now.",
                engine_display_name(view.engine.as_deref().unwrap_or_default())
            ));
        }
        lines.push("Provider reported".into());
        self.show_details("Session context", lines);
    }

    async fn notifications_command(&mut self, argument: &str) {
        let enabled = match argument {
            "on" => true,
            "off" => false,
            _ => {
                self.notice = Some("Use /notifications on or /notifications off.".into());
                return;
            }
        };
        self.config.set_notifications(enabled);
        self.save_config().await;
        if self.notice.is_none() {
            self.notice = Some(if enabled {
                "Terminal notifications enabled.".into()
            } else {
                "Terminal notifications disabled.".into()
            });
        }
    }

    fn show_diagnostics(&mut self) {
        let entries = self
            .diagnostics
            .as_ref()
            .map_or_else(Vec::new, DiagnosticsSink::entries);
        let lines = if entries.is_empty() {
            vec!["No diagnostics.".into()]
        } else {
            entries
                .iter()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|entry| {
                    format!(
                        "{} {} {}",
                        for_display(&entry.timestamp),
                        entry.level.as_str().to_ascii_uppercase(),
                        for_display(&entry.text)
                    )
                })
                .collect()
        };
        self.show_details("Sanitized diagnostics", lines);
    }

    async fn doctor_command(&mut self) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.flush().await;
        }
        let view = self.controller.view();
        let cwd = PathBuf::from(view.cwd.unwrap_or_else(|| self.controller.launch().cwd));
        let logs = self.logs.clone();
        // The runtime's probes say whether each CLI is signed in.
        let probed = self
            .controller
            .probes()
            .into_iter()
            .map(|probe| {
                let engine = probe.engine.to_string();
                crate::doctor::engine_probe_row(
                    &engine,
                    probe.info.as_ref().and_then(|info| info.auth),
                    probe.info.is_some(),
                    probe.error.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        // The inventory reads instruction files and walks skill folders, which
        // can take seconds on an evicted or network directory: off the worker.
        let snapshot = tokio::task::spawn_blocking(move || {
            let user_root = interface_workspace::default_customization_user_root();
            let inventory =
                interface_workspace::read_customization_inventory(&cwd, Some(&user_root));
            let mut snapshot = crate::doctor::snapshot_from_inventory(
                &cwd,
                inventory,
                crate::doctor::read_context_telemetry(&logs),
            );
            crate::doctor::apply_engine_rows(&mut snapshot, probed);
            snapshot
        })
        .await;
        let Ok(snapshot) = snapshot else {
            self.notice = Some("The inventory could not be read.".into());
            return;
        };
        let report = crate::doctor::render(&snapshot, crate::argv::OutputFormat::Text);
        self.show_details(
            "Configured engine and extension inventory",
            report.trim_end().lines().map(str::to_owned).collect(),
        );
    }

    fn queue_command(&mut self, argument: &str) {
        let Some(key) = self.controller.active_key() else {
            self.notice = Some("Select a session before editing its queue.".into());
            return;
        };
        let mut state = write_lock(&self.controller.state);
        let Some(session) = state.sessions.get_mut(&key) else {
            return;
        };
        if let Some(number) = argument
            .strip_prefix("remove ")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .and_then(|value| value.checked_sub(1))
        {
            if number >= session.queued.len() {
                drop(state);
                self.notice = Some("No queued follow-up has that number.".into());
                return;
            }
            session.queued.remove(number);
            drop(state);
            self.controller.changed();
            self.notice = Some("Unsent follow-up removed.".into());
            return;
        }
        if argument.is_empty() {
            let mut lines = if session.queued.is_empty() {
                vec!["The queue is empty.".into()]
            } else {
                session
                    .queued
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        format!(
                            "{}. [run afterward · unsent] {}",
                            index + 1,
                            for_display(text)
                        )
                    })
                    .collect()
            };
            lines.push("/queue <number> <replacement text> · /queue remove <number> · /peer accept|discard [number]".into());
            drop(state);
            self.show_details("Queued prompts", lines);
            return;
        }
        let Some((number, replacement)) = argument.split_once(char::is_whitespace) else {
            drop(state);
            self.notice = Some("Use /queue <number> <replacement text>.".into());
            return;
        };
        let index = number
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1));
        let Some(entry) = index.and_then(|index| session.queued.get_mut(index)) else {
            drop(state);
            self.notice = Some("Use /queue <number> <replacement text>.".into());
            return;
        };
        if replacement.trim().is_empty() {
            drop(state);
            self.notice = Some("Use /queue <number> <replacement text>.".into());
            return;
        }
        replacement.clone_into(entry);
        let displayed = index.unwrap_or_default() + 1;
        drop(state);
        self.controller.changed();
        self.notice = Some(format!("Queued prompt {displayed} was replaced."));
    }

    /// The durable write (fsync + rename) runs on the blocking pool and is
    /// awaited, so saves stay ordered.
    async fn save_config(&mut self) {
        let config = self.config.clone();
        let saved = tokio::task::spawn_blocking(move || config.save())
            .await
            .map_err(|error| error.to_string())
            .and_then(|saved| saved);
        if let Err(error) = saved {
            self.notice = Some(format!("CLI configuration could not be saved: {error}"));
        }
    }

    fn resume_command(&mut self, target: &str) {
        if target.is_empty() {
            let Some(key) = self.controller.active_key() else {
                self.notice = Some("Select a session first or supply an id prefix.".into());
                return;
            };
            self.queue_resume(key, false, None);
            return;
        }
        let matches = self
            .controller
            .agent_sessions()
            .into_iter()
            .filter(|session| {
                session.session_id.starts_with(target) || session.key.starts_with(target)
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => self.notice = Some(format!("No session matches {target}.")),
            [_first, _second, ..] => {
                self.notice = Some(format!("Session prefix {target} is ambiguous."));
            }
            [session] => {
                self.queue_resume(session.key.clone(), true, Some(session.session_id.clone()));
            }
        }
    }

    /// Resumes `key` off the key handler. `select` makes it the active,
    /// remembered session once the provider confirms.
    fn queue_resume(&mut self, key: String, select: bool, shown_id: Option<String>) {
        let controller = self.controller.clone();
        self.queue_effect("Resuming…", async move {
            let result = controller.resume_agent(&key).await;
            EffectOutcome::Resume {
                select: select.then_some(key),
                shown_id,
                result,
            }
        });
    }

    /// Interrupts `key` off the key handler; `command` reports success as `/stop` does.
    fn queue_stop(&mut self, key: String, command: bool) {
        let controller = self.controller.clone();
        self.queue_effect("Stopping…", async move {
            let result = controller.stop_agent(&key).await;
            EffectOutcome::Stop { command, result }
        });
    }

    fn open_session_picker(&mut self) {
        let mut sessions = {
            let state = read_lock(&self.controller.state);
            state
                .sessions
                .values()
                .map(|session| {
                    (
                        session.last_activity.clone(),
                        session.state.key.clone(),
                        session.state.title.clone(),
                        session.state.cwd.clone(),
                        session.state.engine.clone(),
                        session.state.status,
                        session.transcript.snapshot(),
                    )
                })
                .collect::<Vec<_>>()
        };
        sessions.sort_by(|left, right| right.0.cmp(&left.0));
        let items = sessions
            .into_iter()
            .map(
                |(last_activity, key, title, cwd, engine, status, transcript)| {
                    let digest = last_transcript_chars(transcript.as_slice(), 2_048);
                    PickerItem::new(
                        format!("session:{key}"),
                        for_display(&title),
                        Some(for_display(&cwd)),
                        Some(format!("{} · {}", engine, status.as_str())),
                        PaletteAction::ResumeSession(key),
                        Some(format!(
                            "{} · {} · {}\n{}\nlast active {}\n\n{}",
                            for_display(&title),
                            engine,
                            status.as_str(),
                            for_display(&cwd),
                            for_display(&last_activity),
                            digest
                        )),
                    )
                },
            )
            .collect();
        self.open_picker(PaletteKind::Sessions, "Sessions", items, "");
    }

    fn slash_items(&self) -> Vec<PickerItem> {
        let catalog = interface_contracts::command_catalog::command_catalog();
        let mut items = APP_COMMANDS
            .iter()
            .filter(|command| command.name != "fast" || self.controller.supports_fast_mode())
            .map(|command| {
                PickerItem::new(
                    format!("app:{}", command.name),
                    format!("/{}", command.name),
                    Some(command.description.into()),
                    Some("iface".into()),
                    PaletteAction::SetDraft(format!(
                        "/{}{}",
                        command.name,
                        if app_command_takes_argument(command.name) {
                            " "
                        } else {
                            ""
                        }
                    )),
                    catalog
                        .iter()
                        .find(|entry| entry.name == command.name)
                        .map(|entry| {
                            format!(
                                "{}\nTarget: {}\nExample: {}",
                                entry.description, entry.target, entry.example
                            )
                        }),
                )
            })
            .collect::<Vec<_>>();
        let app_names = APP_COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<BTreeSet<_>>();
        items.extend(
            self.controller
                .command_catalogue()
                .commands
                .into_iter()
                .enumerate()
                .filter(|(_, command)| {
                    !is_app_owned_provider_command(&command.name)
                        && !app_names.contains(command.name.to_ascii_lowercase().as_str())
                })
                .map(|(index, command)| {
                    PickerItem::new(
                        format!("provider:{index}"),
                        format!("/{}", for_display(&command.name)),
                        Some(for_display(&command.description)),
                        Some("provider".into()),
                        PaletteAction::SetDraft(provider_command_draft(&command)),
                        None,
                    )
                }),
        );
        items
    }

    fn open_command_picker(&mut self, initial_query: &str) {
        self.open_picker(
            PaletteKind::Commands,
            "Slash commands",
            self.slash_items(),
            initial_query,
        );
    }

    fn open_tool_picker(&mut self) {
        let Some(key) = self.controller.active_key() else {
            self.notice = Some("No tool call is available in this transcript.".into());
            return;
        };
        let state = read_lock(&self.controller.state);
        let Some(session) = state.sessions.get(&key) else {
            self.notice = Some("No tool call is available in this transcript.".into());
            return;
        };
        let items = session
            .transcript
            .iter()
            .rev()
            .filter_map(|cell| match cell {
                TranscriptCell::Tool {
                    call_id,
                    tool,
                    summary,
                    result,
                    ..
                } => Some(PickerItem::new(
                    format!("tool:{call_id}"),
                    for_display(tool),
                    Some(for_display(summary)),
                    Some(
                        match result {
                            None => "running",
                            Some(result) if result.ok => "ok",
                            Some(_) => "failed",
                        }
                        .into(),
                    ),
                    PaletteAction::ToolDetails(call_id.clone()),
                    None,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        drop(state);
        if items.is_empty() {
            self.notice = Some("No tool call is available in this transcript.".into());
            return;
        }
        self.open_picker(PaletteKind::Tools, "Tool calls", items, "");
    }

    fn open_steer_history(&mut self) {
        let Some(key) = self.controller.active_key() else {
            self.notice = Some("Select a session to inspect its steering attempts.".into());
            return;
        };
        let entries = self
            .controller
            .steer_history
            .list_for_session(&key, MAX_SESSION_STEER_HISTORY_ENTRIES);
        if entries.is_empty() {
            self.notice = Some("No steering attempts are recorded for this session.".into());
            return;
        }
        let items = entries
            .into_iter()
            .map(|entry| {
                PickerItem::new(
                    format!("steer:{}", entry.id),
                    steer_time_label(&entry.requested_at),
                    Some(if entry.prompt_preview.is_empty() {
                        "(empty preview)".into()
                    } else {
                        entry.prompt_preview.clone()
                    }),
                    Some(steer_outcome_label(entry.outcome).into()),
                    PaletteAction::SteerDetails(entry.id),
                    None,
                )
            })
            .collect();
        self.open_picker(PaletteKind::Steers, "Steer history", items, "");
    }

    fn show_details(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.overlay = Some(OverlayState::Details {
            title: for_display(&title.into()),
            lines: lines.into_iter().map(|line| for_display(&line)).collect(),
            scroll: 0,
            layout: None,
        });
    }

    fn open_picker(
        &mut self,
        kind: PaletteKind,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        query: impl Into<String>,
    ) {
        self.overlay = Some(OverlayState::Picker(PaletteState {
            kind,
            title: title.into(),
            items,
            query: query.into(),
            active_index: 0,
            preview: None,
        }));
    }

    fn provider_command_problem(&self, text: &str) -> Option<String> {
        if let Some(error) = app_owned_provider_command_error(text) {
            return Some(error);
        }
        let parsed = parse_provider_command_like(text)?;
        if parsed.multiline {
            return Some("Provider slash commands must be a single line.".into());
        }
        let catalogue = self.controller.command_catalogue();
        if !catalogue.known {
            return Some("Provider commands are refreshing; nothing was sent.".into());
        }
        let found = catalogue.commands.iter().any(|command| {
            command.name.eq_ignore_ascii_case(&parsed.name)
                || command
                    .aliases
                    .iter()
                    .flatten()
                    .any(|alias| alias.eq_ignore_ascii_case(&parsed.name))
        });
        (!found).then(|| {
            format!(
                "/{} is not in this session's provider command catalogue.",
                parsed.name
            )
        })
    }

    async fn execute_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::ResumeSession(key) => self.queue_resume(key, true, None),
            PaletteAction::SetCwd(path) => self.set_cwd(PathBuf::from(path)).await,
            PaletteAction::SetEngine(engine) => self.set_engine(engine).await,
            PaletteAction::SetModel { engine, model } => {
                self.set_model_for_engine(engine, model).await;
            }
            PaletteAction::SetEffort(effort) => self.set_effort(effort).await,
            PaletteAction::SetFast(enabled) => self.set_fast_mode(enabled).await,
            PaletteAction::SetAccess(mode) => self.set_access(mode).await,
            PaletteAction::SetDraft(draft) => {
                self.input.replace(draft);
                self.slash_dismissed = true;
            }
            PaletteAction::ToolDetails(call_id) => self.show_tool_details(&call_id),
            PaletteAction::SteerDetails(id) => self.show_steer_details(&id),
        }
    }

    fn show_steer_details(&mut self, id: &str) {
        let Some(key) = self.controller.active_key() else {
            return;
        };
        let Some(entry) = self
            .controller
            .steer_history
            .list_for_session(&key, MAX_SESSION_STEER_HISTORY_ENTRIES)
            .into_iter()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        self.show_details("Steering attempt", steer_details(&entry));
    }

    fn show_tool_details(&mut self, call_id: &str) {
        let Some(key) = self.controller.active_key() else {
            return;
        };
        let state = read_lock(&self.controller.state);
        let Some(session) = state.sessions.get(&key) else {
            return;
        };
        let Some((tool, summary, input_preview, result)) =
            session.transcript.iter().find_map(|cell| match cell {
                TranscriptCell::Tool {
                    call_id: candidate,
                    tool,
                    summary,
                    input_preview,
                    result,
                    ..
                } if candidate == call_id => Some((tool, summary, input_preview, result)),
                _ => None,
            })
        else {
            return;
        };
        let mut lines = vec![
            "summary".into(),
            if summary.is_empty() {
                "No summary.".into()
            } else {
                for_display(summary)
            },
            String::new(),
            "input".into(),
            if input_preview.is_empty() {
                "No input preview.".into()
            } else {
                for_display(input_preview)
            },
        ];
        if let Some(result) = result {
            lines.extend([
                String::new(),
                format!("result · {}", if result.ok { "ok" } else { "failed" }),
                for_display(&result.summary),
            ]);
        }
        for diff in session.transcript.iter() {
            let TranscriptCell::Diff {
                call_id: candidate,
                path,
                before,
                after,
                withheld,
                truncated,
            } = diff
            else {
                continue;
            };
            if candidate != call_id {
                continue;
            }
            lines.extend([String::new(), format!("diff · {}", for_display(path))]);
            if *withheld {
                lines.push("content withheld".into());
            } else if *truncated && before.is_none() && after.is_none() {
                lines.push("text released: the transcript keeps only its newest diffs".into());
            } else {
                if *truncated {
                    lines.push(format!(
                        "truncated: each side keeps its first {} KiB",
                        reducer::DIFF_SIDE_LIMIT_BYTES / 1024
                    ));
                }
                lines.push("--- before".into());
                lines.extend(
                    before
                        .as_deref()
                        .unwrap_or_default()
                        .lines()
                        .map(for_display),
                );
                lines.push("+++ after".into());
                lines.extend(
                    after
                        .as_deref()
                        .unwrap_or_default()
                        .lines()
                        .map(for_display),
                );
            }
        }
        let title = format!("{} details · main", for_display(tool));
        drop(state);
        self.show_details(title, lines);
    }

    #[allow(clippy::too_many_lines)]
    async fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if is_quit_key(&key) {
            self.quit = true;
            return;
        }
        if self.handle_unknown_delivery_key(key) {
            return;
        }
        if self.handle_overlay_key(key).await {
            return;
        }
        if self.handle_approval_key(key).await {
            return;
        }
        if self.handle_slash_key(key).await {
            return;
        }
        // Cursor-only movements share no key with any arm below.
        if let Some(movement) = composer::navigation(key.code, key.modifiers) {
            movement(&mut self.input);
            return;
        }
        match (key.code, key.modifiers) {
            // 0.1.169 bound Ctrl+Y in fullscreen only.
            (KeyCode::Char('y' | 'Y'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && self.mode == TuiMode::Fullscreen =>
            {
                self.dump_transcript();
            }
            (KeyCode::Char('t' | 'T'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(active) = self.controller.active_key() {
                    let text = self.input.take();
                    if !text.is_empty() {
                        let controller = self.controller.clone();
                        self.queue_effect("Steering…", async move {
                            let result = controller.steer_with_history(&active, text).await;
                            EffectOutcome::Steer { result }
                        });
                    }
                }
            }
            (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                self.cycle_access().await;
            }
            (KeyCode::Esc, _) => {
                if let Some(active) = self.controller.active_key() {
                    self.queue_stop(active, false);
                }
            }
            (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.input.insert_char('\n');
            }
            (KeyCode::Enter, _) => self.submit().await,
            (KeyCode::Char('u' | 'U'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.composer_changed(false);
            }
            (KeyCode::Backspace | KeyCode::Delete, modifiers)
                if modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.input.clear();
                self.composer_changed(false);
            }
            // As in 0.1.169, End moves the composer cursor and, in
            // fullscreen, also returns the transcript to the newest output.
            (KeyCode::End, _) => {
                self.input.move_end();
                if self.mode == TuiMode::Fullscreen {
                    self.scroll = TranscriptScroll::Follow;
                }
            }
            (KeyCode::Backspace, _) => {
                self.input.backspace();
                self.composer_changed(false);
            }
            (KeyCode::Delete, _) => {
                self.input.delete();
                self.composer_changed(false);
            }
            (KeyCode::Char(character), modifiers)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                let was_slash_query = is_slash_query(self.input.text());
                self.input.insert_char(character);
                self.composer_changed(was_slash_query);
            }
            (KeyCode::Up | KeyCode::PageUp | KeyCode::Down | KeyCode::PageDown, _)
                if self.mode == TuiMode::Fullscreen =>
            {
                self.scroll_transcript(match key.code {
                    KeyCode::Up => -1,
                    KeyCode::PageUp => -8,
                    KeyCode::Down => 1,
                    _ => 8,
                });
            }
            _ => {}
        }
    }

    /// Moves the transcript view `rows` rows (negative is older), clamped to
    /// what the last frame laid out. Reaching the newest rows follows output
    /// again, as it did in 0.1.169.
    fn scroll_transcript(&mut self, rows: isize) {
        let Some(layout) = &self.transcript_layout else {
            return;
        };
        let maximum = layout.maximum;
        let top = layout
            .held_top(self.scroll)
            .map_or(maximum, |top| top.min(maximum))
            .saturating_add_signed(rows);
        self.scroll = if top >= maximum {
            TranscriptScroll::Follow
        } else {
            layout.hold_at(top).unwrap_or_default()
        };
    }

    fn composer_changed(&mut self, was_slash_query: bool) {
        self.slash_index = 0;
        let slash_query = is_slash_query(self.input.text());
        if !slash_query || !was_slash_query {
            self.slash_dismissed = false;
        }
        self.project_composer_state();
    }

    fn project_composer_state(&mut self) {
        let Some(key) = self.controller.active_key() else {
            return;
        };
        if let Err(error) = self.controller.project_draft(&key, self.input.is_empty()) {
            self.notice = Some(error);
        }
    }

    fn paste(&mut self, text: &str) {
        if self.delivery_unknown.is_some() {
            return;
        }
        let was_slash_query = is_slash_query(self.input.text());
        self.input.insert(text);
        self.composer_changed(was_slash_query);
    }

    fn handle_unknown_delivery_key(&mut self, key: KeyEvent) -> bool {
        let Some(pending) = self.delivery_unknown.clone() else {
            return false;
        };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return true;
        }
        match key.code {
            KeyCode::Char('x' | 'X') => {
                self.delivery_unknown = None;
                self.notice = Some("Uncertain prompt discarded locally.".into());
            }
            KeyCode::Char('r' | 'R') => {
                if let Some(key) = pending.key.clone() {
                    let controller = self.controller.clone();
                    self.queue_effect("Resending…", async move {
                        let result = controller.send_agent(&key, pending.text.clone()).await;
                        EffectOutcome::Resend { pending, result }
                    });
                } else {
                    self.queue_start(pending.text);
                }
            }
            _ => {}
        }
        true
    }

    async fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        let Some(overlay) = self.overlay.as_mut() else {
            return false;
        };
        let mut selected = None;
        match overlay {
            OverlayState::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    self.overlay = None;
                }
            }
            OverlayState::Details { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter => self.overlay = None,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(8),
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::PageDown => *scroll = scroll.saturating_add(8),
                KeyCode::Home => *scroll = 0,
                KeyCode::End => *scroll = u16::MAX,
                _ => {}
            },
            OverlayState::Picker(palette) => {
                let matches = filtered_picker_indices(&palette.items, &palette.query);
                let selected_index = palette.active_index.min(matches.len().saturating_sub(1));
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Up => {
                        palette.active_index = selected_index.saturating_sub(1);
                        palette.preview = None;
                    }
                    KeyCode::Down => {
                        palette.active_index =
                            (selected_index + 1).min(matches.len().saturating_sub(1));
                        palette.preview = None;
                    }
                    KeyCode::Enter => {
                        selected = matches
                            .get(selected_index)
                            .and_then(|index| palette.items.get(*index))
                            .map(|item| item.action.clone());
                        self.overlay = None;
                    }
                    KeyCode::Char(' ') => {
                        let preview = matches
                            .get(selected_index)
                            .and_then(|index| palette.items.get(*index))
                            .and_then(|item| item.preview.clone());
                        if preview.is_some() {
                            palette.preview = preview;
                        } else if !key.modifiers.intersects(
                            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                        ) {
                            palette.query.push(' ');
                            palette.active_index = 0;
                        }
                    }
                    KeyCode::Backspace | KeyCode::Delete => {
                        palette.query.pop();
                        palette.active_index = 0;
                        palette.preview = None;
                    }
                    KeyCode::Char(character)
                        if !key.modifiers.intersects(
                            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                        ) =>
                    {
                        palette.query.push(character);
                        palette.active_index = 0;
                        palette.preview = None;
                    }
                    _ => {}
                }
            }
        }
        if let Some(action) = selected {
            self.execute_palette_action(action).await;
        }
        true
    }

    async fn handle_slash_key(&mut self, key: KeyEvent) -> bool {
        if self.slash_dismissed || !is_slash_query(self.input.text()) {
            return false;
        }
        if matches!(key.code, KeyCode::BackTab)
            || matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            return false;
        }
        let items = self.slash_items();
        if items.is_empty() {
            return false;
        }
        let query = self.input.text().strip_prefix('/').unwrap_or_default();
        let matches = slash_picker_indices(&items, query);
        self.slash_index = self.slash_index.min(matches.len().saturating_sub(1));
        match key.code {
            KeyCode::Esc => {
                self.slash_dismissed = true;
                true
            }
            KeyCode::Up
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
            {
                self.slash_index = self.slash_index.saturating_sub(1);
                true
            }
            KeyCode::Down
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
            {
                self.slash_index = (self.slash_index + 1).min(matches.len().saturating_sub(1));
                true
            }
            KeyCode::Enter | KeyCode::Tab => {
                let Some(item) = matches
                    .get(self.slash_index)
                    .and_then(|index| items.get(*index))
                    .cloned()
                else {
                    return true;
                };
                let exact = item.label.eq_ignore_ascii_case(&format!("/{query}"));
                self.execute_palette_action(item.action).await;
                if key.code == KeyCode::Enter && exact {
                    self.submit().await;
                }
                true
            }
            _ => false,
        }
    }

    /// Ctrl+Y: queues the plain transcript for the terminal's native
    /// scrollback (the terminal loop writes it before the next frame) and
    /// leaves only a one-line count in the footer.
    fn dump_transcript(&mut self) {
        let lines = transcript_plain_lines(&self.controller.transcript_snapshot());
        self.notice = Some(format!(
            "Dumped {} line{} to scrollback.",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        ));
        self.scrollback_dump = Some(for_display(&lines.join("\n")));
    }
}

/// Runs the interactive terminal and its local agent control socket.
/// `app::interactive_command`, its only caller, has already refused a
/// non-interactive stdin or stdout.
///
/// # Errors
///
/// Returns an error when startup or rendering fails, or clean server teardown
/// fails.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn run(
    runtime: SessionRuntime,
    collaboration: Option<CliCollaborationRuntime>,
    collaboration_diagnostic: Option<String>,
    diagnostics: DiagnosticsSink,
    prepared_orchestration: PreparedOrchestrationHost,
    paths: &CliPaths,
    parsed: &CliArgv,
    cwd: &Path,
    mut signals: ShutdownSignals,
) -> Result<u8, String> {
    let mut config_store = CliConfigStore::new(paths.config.clone())?;
    let config = config_store.load();
    let mode = parsed.tui.unwrap_or(config.tui_mode);
    if parsed
        .tui
        .is_some_and(|requested| requested != config.tui_mode)
    {
        config_store.set_tui_mode(mode);
        config_store.save()?;
    }
    let engine = parsed
        .engine
        .clone()
        .or_else(|| config.last_engine.clone())
        .unwrap_or_else(|| "claude".into());
    let (model, effort) = config.provider_preferences(&engine);
    let access = config.access_mode_for_launch(&engine);
    let launch = LaunchDefaults {
        engine,
        cwd: cwd.to_string_lossy().into_owned(),
        model,
        effort,
        access,
        scope: parsed.scope.unwrap_or(config.scope),
        // 0.1.169 did not persist Fast mode. Omission is the provider-default
        // service tier; only an explicit in-process /fast choice turns it on.
        fast_mode: false,
    };
    if let Some(code) = signals.pending() {
        return Ok(code);
    }
    let prompt_history = PromptHistory::new(paths.history.clone())?;
    let steer_history = SteerHistoryStore::new(paths.steers.clone())?;
    steer_history.load();
    let controller = TuiController::new(
        runtime,
        collaboration,
        launch,
        prompt_history,
        steer_history,
        diagnostics.clone(),
        prepared_orchestration,
    )
    .await?;
    let mut app = TuiApp {
        controller: controller.clone(),
        config: config_store,
        input: Composer::new(String::new()),
        delivery_unknown: None,
        notice: None,
        overlay: None,
        slash_index: 0,
        slash_dismissed: false,
        pending_access_confirmation: None,
        logs: paths.logs.clone(),
        diagnostics: Some(diagnostics.clone()),
        scroll: TranscriptScroll::Follow,
        scrollback_dump: None,
        mode,
        focused: true,
        quit: false,
        transcript_layout: None,
        effects: Effects::default(),
    };
    if let Some(diagnostic) = collaboration_diagnostic {
        app.notice = Some(format!(
            "collaboration is unavailable; starting providers without cross-model tools: {diagnostic}"
        ));
    }
    if let Some(code) = signals.pending() {
        controller.shutdown().await;
        return Ok(code);
    }
    // Explicit resume/continue needs a current provider listing to resolve the
    // requested identity. The ordinary empty launch screen needs only cache.
    let discover_after_paint = parsed.resume.is_none() && !parsed.continue_session;
    if !discover_after_paint {
        controller.discover_providers().await;
    }
    if let Err(error) = restore_initial_selection(&mut app, parsed).await {
        controller.shutdown().await;
        return Err(error);
    }
    if let Some(code) = signals.pending() {
        controller.shutdown().await;
        return Ok(code);
    }
    let server = match start_agent_server(
        &paths.data_dir,
        &paths.agent_socket,
        &paths.agent_names,
        Arc::new(controller.clone()),
    )
    .await
    {
        Ok(server) => server,
        Err(error) => {
            controller.shutdown().await;
            return Err(error);
        }
    };
    if let Some(code) = signals.pending() {
        let _ = server.close().await;
        controller.shutdown().await;
        return Ok(code);
    }
    let terminal = match TerminalSession::start(mode) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = server.close().await;
            controller.shutdown().await;
            return Err(error);
        }
    };
    let loop_exit =
        match run_terminal_loop(terminal, &mut app, &mut signals, discover_after_paint).await {
            Ok(exit) => exit,
            Err(error) => {
                let _ = server.close().await;
                controller.shutdown().await;
                return Err(error);
            }
        };
    let cleanup = async {
        let server_result = server.close().await;
        controller.shutdown().await;
        server_result
    };
    tokio::pin!(cleanup);
    // run_terminal_loop took the TerminalSession by value, so its Drop has
    // already left raw mode and sent the mode-correct teardown (no
    // LeaveAlternateScreen inline). The force exits below rely on that: keep
    // the terminal out of this drain, or restore it here mode-aware.
    let mut signal_exit = loop_exit;
    let mut deadline = signal_exit.map(|_| Instant::now() + SHUTDOWN_TIMEOUT);
    loop {
        tokio::select! {
            result = &mut cleanup => return result.map(|()| signal_exit.unwrap_or(0)),
            code = signals.next() => {
                match accept_shutdown_signal(&mut signal_exit, code) {
                    ShutdownSignalAction::BeginDrain => {
                        deadline = Some(Instant::now() + SHUTDOWN_TIMEOUT);
                    }
                    ShutdownSignalAction::ForceExit(code) => std::process::exit(code.into()),
                }
            }
            () = wait_for_deadline(deadline), if deadline.is_some() => {
                std::process::exit(signal_exit.unwrap_or(1).into());
            }
        }
    }
}

async fn restore_initial_selection(app: &mut TuiApp, parsed: &CliArgv) -> Result<(), String> {
    let target = if let Some(prefix) = &parsed.resume {
        let matches = app
            .controller
            .agent_sessions()
            .into_iter()
            .filter(|session| session.session_id.starts_with(prefix))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => return Err(format!("No session matches id prefix \"{prefix}\".")),
            [_first, _second, ..] => {
                return Err(format!("Session id prefix \"{prefix}\" is ambiguous."));
            }
            [session] => Some(session.key.clone()),
        }
    } else if parsed.continue_session {
        app.controller.continue_target()
    } else {
        None
    };
    if parsed.continue_session && target.is_none() {
        return Err(format!(
            "no session is available for {}.",
            app.controller.launch().cwd
        ));
    }
    if let Some(key) = target {
        app.controller
            .resume_key(&key)
            .await
            .map_err(|error| error.message)?;
        app.controller.set_active(Some(key));
    }
    Ok(())
}

/// Orders two `last_activity` values oldest first, as 0.1.169 sorted them:
/// by instant when both parse and differ, otherwise by text.
fn activity_order(left: &str, right: &str) -> std::cmp::Ordering {
    let instant = |value: &str| chrono::DateTime::parse_from_rfc3339(value).ok();
    match (instant(left), instant(right)) {
        (Some(left), Some(right)) if left != right => left.cmp(&right),
        _ => left.cmp(right),
    }
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        std::future::pending::<()>().await;
    }
}

mod transcript;
mod view;
use transcript::Transcript;
use view::transcript_plain_lines;

/// A start that failed. `unknown` means its prompt may have reached the
/// provider, so it is offered for an explicit resend, never put back.
#[derive(Debug)]
struct StartFailure {
    error: AgentProtocolError,
    unknown: bool,
}

impl StartFailure {
    const fn unsent(error: AgentProtocolError) -> Self {
        Self {
            error,
            unknown: false,
        }
    }

    const fn unknown(error: AgentProtocolError) -> Self {
        Self {
            error,
            unknown: true,
        }
    }
}

/// `ready_session` for a start, keeping whether its prompt can have left.
fn started_session(
    outcome: SessionOpenOutcome,
) -> Result<interface_runtime::RuntimeSession, StartFailure> {
    let unknown = matches!(
        outcome,
        SessionOpenOutcome::OutcomeUnknown(_)
            | SessionOpenOutcome::ProviderReadyLocalFailure { .. }
    );
    ready_session(outcome).map_err(|error| StartFailure { error, unknown })
}

fn ready_session(
    outcome: SessionOpenOutcome,
) -> Result<interface_runtime::RuntimeSession, AgentProtocolError> {
    match outcome {
        SessionOpenOutcome::Ready(session) => Ok(session),
        SessionOpenOutcome::DefinitelyUnsent(error)
        | SessionOpenOutcome::DefinitelyRejected(error)
        | SessionOpenOutcome::OutcomeUnknown(error)
        | SessionOpenOutcome::ProviderReadyLocalFailure { error, .. } => {
            Err(protocol(AgentErrorCode::Failed, error.message))
        }
    }
}

fn delivery_from_runtime(delivery: interface_runtime::RuntimeDelivery) -> AgentDelivery {
    match delivery.outcome {
        DeliveryOutcome::Confirmed(()) => AgentDelivery::accepted(false),
        DeliveryOutcome::DefinitelyUnsent(error) | DeliveryOutcome::DefinitelyRejected(error) => {
            AgentDelivery {
                kind: AgentDeliveryKind::DefinitelyUnsent,
                message: Some(error.message),
                queued: false,
            }
        }
        DeliveryOutcome::OutcomeUnknown(error) => AgentDelivery {
            kind: AgentDeliveryKind::OutcomeUnknown,
            message: Some(error.message),
            queued: false,
        },
    }
}

fn definitely_unsent(message: String) -> AgentDelivery {
    AgentDelivery {
        kind: AgentDeliveryKind::DefinitelyUnsent,
        message: Some(message),
        queued: false,
    }
}

fn require_confirmed(delivery: AgentDelivery) -> Result<(), String> {
    if delivery.kind == AgentDeliveryKind::AdapterAccepted {
        Ok(())
    } else {
        Err(delivery
            .message
            .unwrap_or_else(|| "Provider operation failed.".into()))
    }
}

async fn push_accepted_steer(
    controller: &TuiController,
    key: &str,
    state: &AgentSessionState,
    text: &str,
) {
    if let Some(row) = write_lock(&controller.state).sessions.get_mut(key) {
        push_message(row, "user", text);
    }
    controller.changed();
    if let Ok(engine) = EngineId::new(state.engine.clone()) {
        controller.record_prompt(&engine, &state.cwd, text).await;
    }
}

fn capabilities_for(engine: &EngineId) -> AgentCapabilities {
    capabilities_for_text(engine.as_str())
}

fn capabilities_for_text(engine: &str) -> AgentCapabilities {
    AgentCapabilities {
        reference: true,
        transcript: true,
        message: true,
        steer: matches!(engine, "claude" | "codex"),
        stop: true,
        inbox: true,
        monitor: true,
    }
}

fn last_transcript_chars(cells: &[TranscriptCell], maximum: usize) -> String {
    // Walk back from the newest cell like agent_text_tail: only the rows that can
    // reach the character budget are formatted, never the whole transcript.
    let mut rows = Vec::new();
    let mut characters = 0usize;
    'cells: for cell in cells.iter().rev() {
        let Some((role, text)) = agent_text(cell) else {
            continue;
        };
        for line in text.lines().rev() {
            let row = format!("{role}: {}", for_display(line));
            characters = characters.saturating_add(row.chars().count().saturating_add(1));
            rows.push(row);
            if characters > maximum {
                break 'cells;
            }
        }
    }
    if rows.is_empty() {
        return "No transcript text is available.".into();
    }
    rows.reverse();
    let digest = rows.join("\n");
    let start = digest
        .char_indices()
        .rev()
        .nth(maximum.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    digest[start..].to_owned()
}

fn agent_text(cell: &TranscriptCell) -> Option<(&str, &str)> {
    let (role, text) = match cell {
        TranscriptCell::Message { role, markdown } => (*role, markdown.as_str()),
        TranscriptCell::Notice {
            message,
            tone: CellTone::Error,
        } => ("error", message.as_str()),
        TranscriptCell::Notice { message, .. } => ("notice", message.as_str()),
        _ => return None,
    };
    Some((role, text))
}

/// Only the transcript-shape test reads whole cells now; the pickers walk the
/// tail through [`agent_text`] instead.
#[cfg(test)]
fn agent_text_rows(cell: &TranscriptCell) -> Vec<String> {
    let Some((role, text)) = agent_text(cell) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| format!("{role}: {}", for_display(line)))
        .collect()
}

/// The last `maximum` transcript lines, oldest first. Walks back from the
/// newest line and also stops at the response byte budget, so a huge
/// `--lines` neither formats the whole transcript under the state lock nor
/// builds a reply too large to send. The newest line is always included.
fn agent_text_tail(cells: &[TranscriptCell], maximum: usize) -> Vec<String> {
    if maximum == 0 {
        return Vec::new();
    }
    let mut tail = Vec::new();
    let mut bytes = 0_usize;
    'cells: for cell in cells.iter().rev() {
        let Some((role, text)) = agent_text(cell) else {
            continue;
        };
        for line in text.lines().rev() {
            let row = format!("{role}: {}", for_display(line));
            bytes = bytes.saturating_add(encoded_len(&row).saturating_add(1));
            if bytes > AGENT_RESULT_BUDGET_BYTES && !tail.is_empty() {
                break 'cells;
            }
            tail.push(row);
            if tail.len() == maximum {
                break 'cells;
            }
        }
    }
    tail.reverse();
    tail
}

fn engine_display_name(engine: &str) -> String {
    match engine {
        "claude" => "Claude".into(),
        "codex" => "GPT/Codex".into(),
        "acp:grok" => "Grok".into(),
        "acp:qwen" => "Qwen".into(),
        _ => engine.into(),
    }
}

fn effort_display_name(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => "None".into(),
        "minimal" => "Minimal".into(),
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "Extra high".into(),
        "max" => "Max".into(),
        "ultra" => "Ultra".into(),
        "ultracode" => "Ultracode".into(),
        _ => {
            let words = value.trim().replace(['_', '-'], " ");
            let mut characters = words.chars();
            characters.next().map_or_else(
                || "Effort".into(),
                |first| first.to_uppercase().chain(characters).collect(),
            )
        }
    }
}

fn steer_time_label(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp).map_or_else(
        |_| for_display(timestamp),
        |parsed| {
            parsed
                .with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        },
    )
}

const fn steer_outcome_label(outcome: SteerOutcome) -> &'static str {
    match outcome {
        SteerOutcome::AdapterAccepted => "adapter accepted",
        SteerOutcome::DefinitelyUnsent => "not sent",
        SteerOutcome::OutcomeUnknown => "outcome unknown",
        SteerOutcome::Pending => "settlement missing",
    }
}

fn steer_details(entry: &SteerHistoryEntry) -> Vec<String> {
    let explanation = match entry.outcome {
        SteerOutcome::AdapterAccepted => {
            "The adapter accepted the request; this does not prove how the model applied it."
        }
        SteerOutcome::DefinitelyUnsent => {
            "iface proved the request did not cross the provider boundary."
        }
        SteerOutcome::OutcomeUnknown => {
            "The request may have crossed the provider boundary; iface will not retry it automatically."
        }
        SteerOutcome::Pending => {
            "The request was recorded before provider contact, but no final local outcome was saved."
        }
    };
    let mut lines = vec![
        format!("requested · {}", entry.requested_at),
        format!(
            "settled · {}",
            entry.settled_at.as_deref().unwrap_or("not recorded")
        ),
        format!("outcome · {}", steer_outcome_label(entry.outcome)),
        explanation.into(),
        format!("session · {}", entry.session_key),
        format!("engine · {}", entry.engine),
        format!(
            "model · {}",
            entry
                .model
                .as_deref()
                .unwrap_or("provider default / unknown")
        ),
        format!("digest · {}", entry.prompt_digest),
        String::new(),
        "preview".into(),
        if entry.prompt_preview.is_empty() {
            "(empty preview)".into()
        } else {
            entry.prompt_preview.clone()
        },
    ];
    if let Some(detail) = &entry.detail {
        lines.extend([String::new(), "detail".into(), detail.clone()]);
    }
    lines
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME").map_or_else(|| PathBuf::from(path), PathBuf::from);
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let mut wanted = needle.chars();
    let mut next = wanted.next();
    for character in haystack.chars() {
        if next == Some(character) {
            next = wanted.next();
            if next.is_none() {
                return true;
            }
        }
    }
    next.is_none()
}

fn filtered_picker_indices(items: &[PickerItem], query: &str) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| query.is_empty() || fuzzy_match(&item.haystack, &query))
        .take(PICKER_MATCH_LIMIT)
        .map(|(index, _)| index)
        .collect()
}

fn slash_picker_indices(items: &[PickerItem], query: &str) -> Vec<usize> {
    let query = query.to_ascii_lowercase();
    let prefix = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.label_lower
                .strip_prefix('/')
                .is_some_and(|name| name.starts_with(&query))
        })
        .take(PICKER_MATCH_LIMIT)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if prefix.is_empty() {
        filtered_picker_indices(items, &query)
    } else {
        prefix
    }
}

fn is_slash_query(draft: &str) -> bool {
    draft.starts_with('/')
        && !draft
            .chars()
            .any(|character| matches!(character, ' ' | '\t' | '\n' | '\r'))
}

fn protocol(code: AgentErrorCode, message: impl Into<String>) -> AgentProtocolError {
    AgentProtocolError::new(code, message)
}

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tests;
