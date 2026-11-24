use interface_contracts::engine::{EngineCapabilities, EngineModels};
use interface_contracts::ipc::CollaborationEngine;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

use super::approval::{ApprovalKind, answers_for_questions, build_approval};
use super::collaboration::{
    ParsedCollaborationRequest, parse_advisor_argument, parse_collaboration_request,
    select_collaboration_row,
};
use super::commands::{ParsedSendArgument, parse_send_argument, parse_spawn_argument};
use super::reducer::{find_tool_mut, push_cell};
use super::terminal::{
    REVISION_FRAME_INTERVAL, RevisionFrameCap, notification_part, terminal_notification_sequence,
};
use super::view::*;
use super::*;
use crate::argv::is_engine_id;

/// A test value whose stores live in a temporary directory that is removed
/// only when the fixture is dropped, after the value itself.
struct Fixture<T> {
    value: T,
    temp: tempfile::TempDir,
}

impl<T> std::ops::Deref for Fixture<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> std::ops::DerefMut for Fixture<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

fn controller() -> Fixture<TuiController> {
    let temp = tempfile::tempdir().expect("temp");
    std::fs::set_permissions(
        temp.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("private temp");
    Fixture {
        value: controller_in(temp.path()),
        temp,
    }
}

fn controller_in(temp: &Path) -> TuiController {
    let sessions = interface_registry::SessionRegistry::open(temp).expect("registry");
    let history_root = temp.join("history");
    std::fs::create_dir(&history_root).expect("history root");
    std::fs::set_permissions(
        &history_root,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("private history");
    let history = interface_runtime::EngineHistoryStore::open(
        &history_root,
        interface_runtime::HistoryLimits::default(),
    )
    .expect("history");
    let runtime =
        SessionRuntime::builder(interface_runtime::RuntimeServices::new(sessions, history))
            .build()
            .expect("runtime");
    let (revision, _) = watch::channel(0);
    TuiController {
        runtime,
        collaboration: None,
        orchestration: None,
        diagnostics: None,
        state: Arc::new(RwLock::new(ControllerState::default())),
        launch: Arc::new(RwLock::new(LaunchDefaults {
            engine: "claude".into(),
            cwd: "/repo".into(),
            model: None,
            effort: None,
            access: Some("default".into()),
            scope: LaunchScope::All,
            fast_mode: false,
        })),
        probes: Arc::new(RwLock::new(BTreeMap::new())),
        launch_commands: Arc::new(RwLock::new(CommandCatalogue::default())),
        launch_command_revision: Arc::new(AtomicU64::new(0)),
        prompt_history: PromptHistory::new(temp.join("prompt-history")).expect("prompt history"),
        steer_history: SteerHistoryStore::new(temp.join("steers.json")).expect("steer history"),
        notifications: Arc::new(RwLock::new(VecDeque::new())),
        queued_failures: Arc::new(RwLock::new(VecDeque::new())),
        draft_revision: Arc::new(std::sync::Mutex::new(0)),
        revision,
    }
}

fn app(mode: TuiMode) -> Fixture<TuiApp> {
    let Fixture {
        value: controller,
        temp,
    } = controller();
    let value = TuiApp {
        controller,
        config: CliConfigStore::new(temp.path().join("config.json")).expect("config"),
        input: Composer::new("review this".into()),
        delivery_unknown: None,
        notice: None,
        overlay: None,
        slash_index: 0,
        slash_dismissed: false,
        pending_access_confirmation: None,
        logs: temp.path().join("logs"),
        diagnostics: None,
        scroll: TranscriptScroll::Follow,
        scrollback_dump: None,
        mode,
        focused: true,
        quit: false,
        transcript_layout: None,
        effects: Effects::default(),
    };
    Fixture { value, temp }
}

/// Runs each queued provider effect to completion and applies it, as the
/// terminal loop does between frames.
async fn settle(app: &mut TuiApp) {
    while let Some(run) = app.next_effect() {
        let outcome = run.await;
        app.finish_effect(outcome).await;
    }
}

fn rendered(width: u16, height: u16, app: &mut TuiApp) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| render_tui(frame, app)).expect("draw");
    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn session(key: &str, status: AgentStatus) -> ControlledSession {
    let (engine, session_id) = key.split_once('#').expect("session key");
    ControlledSession {
        state: AgentSessionState {
            key: key.into(),
            engine: engine.into(),
            session_id: session_id.into(),
            status,
            cwd: "/repo".into(),
            title: "Fixture session".into(),
            model: None,
            task: None,
            capabilities: Some(capabilities_for_text(engine)),
            live: true,
        },
        transcript: Transcript::default(),
        queued: VecDeque::new(),
        pending_approval: None,
        effort: None,
        access: Some("default".into()),
        fast_mode: false,
        models: Vec::new(),
        commands: CommandCatalogue::default(),
        setting_sources: None,
        disallowed_tools: None,
        context: None,
        previous_context: None,
        last_activity: "2026-08-27T00:00:00.000Z".into(),
        telemetry_startup_recorded: false,
        telemetry_pending: false,
        telemetry_recorded: None,
    }
}

fn command(name: &str, aliases: &[&str]) -> ProviderCommand {
    ProviderCommand {
        name: name.into(),
        description: format!("{name} description"),
        argument_hint: Some("target".into()),
        aliases: (!aliases.is_empty())
            .then(|| aliases.iter().map(|alias| (*alias).to_owned()).collect()),
    }
}

#[test]
fn discovery_preserves_changed_live_and_removed_sessions() {
    let controller = controller();
    let summary = |id: &str| -> SessionSummary {
        serde_json::from_value(serde_json::json!({
            "engine": "claude", "sessionId": id, "title": "Provider title",
            "cwd": "/repo", "status": "idle", "cloud": false, "archived": false,
            "lastActivity": "2026-09-04T00:00:00.000Z",
            "firstSeen": "2026-09-04T00:00:00.000Z"
        }))
        .expect("summary")
    };
    for id in ["unchanged", "changed", "live", "removed"] {
        let row = controlled_from_summary(format!("claude#{id}"), summary(id));
        write_lock(&controller.state)
            .sessions
            .insert(row.state.key.clone(), row);
    }
    let initial = read_lock(&controller.state)
        .sessions
        .iter()
        .map(|(key, row)| (key.clone(), row.state.clone()))
        .collect();
    {
        let mut state = write_lock(&controller.state);
        state
            .sessions
            .get_mut("claude#changed")
            .unwrap()
            .state
            .title = "Local edit".into();
        state.sessions.get_mut("claude#live").unwrap().state.live = true;
        state.sessions.get_mut("claude#live").unwrap().state.model = Some("Live model".into());
        state.sessions.remove("claude#removed");
    }
    let summaries = ["unchanged", "changed", "live", "removed", "new"]
        .into_iter()
        .map(|id| {
            let mut row = summary(id);
            row.title = "Refreshed provider title".into();
            row
        })
        .collect();
    controller.merge_discovered_sessions(&initial, summaries);
    let state = read_lock(&controller.state);
    assert_eq!(
        state.sessions["claude#unchanged"].state.title,
        "Refreshed provider title"
    );
    assert_eq!(state.sessions["claude#changed"].state.title, "Local edit");
    assert_eq!(
        state.sessions["claude#live"].state.model.as_deref(),
        Some("Live model")
    );
    assert!(!state.sessions.contains_key("claude#removed"));
    assert!(state.sessions.contains_key("claude#new"));
}

fn model(value: &str, efforts: &[&str], fast: bool) -> ModelOption {
    ModelOption {
        value: value.into(),
        display_name: format!("{value} display"),
        description: Some(format!("{value} description")),
        supports_effort: !efforts.is_empty(),
        effort_levels: efforts.iter().map(|value| (*value).to_owned()).collect(),
        is_default: Some(value == "default-model"),
        supports_fast_mode: Some(fast),
    }
}

fn install_probe(
    controller: &TuiController,
    engine: &str,
    model_catalog: Vec<ModelOption>,
    efforts: &[&str],
    permission_modes: &[&str],
) {
    let engine_id = EngineId::new(engine).expect("engine");
    write_lock(&controller.probes).insert(
        engine.into(),
        ProviderProbe {
            engine: engine_id,
            info: Some(EngineInfo {
                auth: None,
                binary_path: format!("/fixture/{engine}"),
                version: "1.0.0".into(),
                login_identity: None,
                capabilities: EngineCapabilities {
                    streaming: true,
                    approvals: true,
                    resume: true,
                    fork: true,
                    cloud_sessions: false,
                    models: EngineModels::Dynamic("provider".into()),
                    effort_levels: efforts.iter().map(|value| (*value).to_owned()).collect(),
                    permission_modes: permission_modes
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                    consultation_safety: None,
                    steer: Some(true),
                },
                display_name: Some(engine_display_name(engine)),
                model_catalog: Some(model_catalog),
            }),
            error: None,
        },
    );
}

#[test]
fn normal_and_narrow_render_keep_composer_and_help_reachable() {
    let normal = rendered(80, 24, &mut app(TuiMode::Fullscreen));
    assert!(normal.contains("Interface"));
    assert!(normal.contains("❯ review this"));
    assert!(normal.contains("/help"));
    let narrow = rendered(30, 10, &mut app(TuiMode::Inline));
    assert!(narrow.contains("iface"));
    assert!(narrow.contains("❯ review this"));
}

#[test]
fn transcript_layout_tracks_cells_not_unrelated_state_updates() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#transcript";
    let mut row = session(key, AgentStatus::Working);
    push_cell(
        &mut row,
        TranscriptCell::Message {
            role: "user",
            markdown: "first turn".into(),
        },
    );
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));
    let area = Rect::new(0, 0, 40, 8);
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("terminal");
    let draw = |terminal: &mut Terminal<TestBackend>, app: &mut TuiApp| {
        terminal
            .draw(|frame| render_transcript(frame, area, app))
            .expect("draw");
        (0..8)
            .map(|row| {
                (0..40)
                    .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    assert!(draw(&mut terminal, &mut app).contains("first turn"));
    let source = app.transcript_layout.as_ref().expect("layout").source;
    let cells = app.controller.transcript_snapshot();
    assert_eq!(
        Arc::strong_count(&cells),
        2,
        "the layout must not retain transcript cells"
    );
    drop(cells);
    app.controller.changed();
    app.input.insert(" an unrelated draft edit");
    assert!(draw(&mut terminal, &mut app).contains("first turn"));
    assert_eq!(
        app.transcript_layout.as_ref().expect("layout").source,
        source
    );

    push_cell(
        write_lock(&app.controller.state)
            .sessions
            .get_mut(key)
            .expect("session"),
        TranscriptCell::Message {
            role: "user",
            markdown: "second turn".into(),
        },
    );
    // No controller notification is needed to distinguish the changed cells.
    assert!(draw(&mut terminal, &mut app).contains("second turn"));
    assert_ne!(
        app.transcript_layout.as_ref().expect("layout").source,
        source
    );
    app.controller.set_active(None);
    assert!(!draw(&mut terminal, &mut app).contains("second turn"));
    assert!(draw(&mut terminal, &mut app).contains("Welcome to iface"));
    app.controller.set_active(Some(key.into()));
    assert!(draw(&mut terminal, &mut app).contains("second turn"));
}

#[test]
fn transcript_in_place_edits_are_marked_without_cloning_cells() {
    let mut row = session("claude#cache", AgentStatus::Working);
    push_message(&mut row, "assistant", "existing text");
    let version = row.transcript.version();
    let cells = row.transcript.as_ptr();
    let event = EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: " appended".into(),
            agent_key: None,
        },
    };
    reduce_event(&mut row, &event);
    assert_eq!(row.transcript.first_change_since(version), Some(0));
    assert_eq!(row.transcript.as_ptr(), cells);
    assert!(
        matches!(&row.transcript[0], TranscriptCell::Message { markdown, .. }
        if markdown == "existing text appended")
    );
}

#[test]
fn overlay_rect_centers_the_panel_and_yields_to_small_terminals() {
    let area = Rect::new(0, 0, 100, 50);
    // 90% x 82% of the area, centred; the odd leftover row goes above the panel.
    assert_eq!(overlay_rect(area), Rect::new(5, 5, 90, 41));
    let overlay = overlay_rect(area);
    assert!(overlay.right() <= area.right() && overlay.bottom() <= area.bottom());
    let small = Rect::new(0, 0, 35, 11);
    assert_eq!(overlay_rect(small), small);
}

#[test]
fn wrapped_rows_count_word_wrapping_on_display_width() {
    assert_eq!(wrapped_rows(&Line::raw("abcde"), 4), 2);
    // Five double-width characters occupy ten columns, so they wrap three times
    // at width four even though five characters would fit in two rows.
    assert_eq!(wrapped_rows(&Line::raw("四四四四四"), 4), 3);
    assert_eq!(wrapped_rows(&Line::raw(""), 4), 1);
    // Whole words move to the next row: a character count says 3 rows.
    assert_eq!(wrapped_rows(&Line::raw("aaaa bbbb cccc zzzz"), 7), 4);
}

#[test]
fn help_copy_matches_the_legacy_local_surface() {
    let mut app = app(TuiMode::Fullscreen);
    app.overlay = Some(OverlayState::Help);
    let output = rendered(100, 28, &mut app);
    assert!(output.contains("Enter sends when idle and queues while busy."));
    assert!(output.contains("Ctrl+T steers only providers"));
    assert!(output.contains("Fullscreen:"));
}

#[test]
fn provider_command_preflight_is_catalogue_bound_and_fail_closed() {
    let app = app(TuiMode::Fullscreen);
    *write_lock(&app.controller.launch_commands) = CommandCatalogue {
        known: true,
        commands: vec![command("review", &["check"])],
    };

    assert_eq!(app.provider_command_problem("/review main"), None);
    assert_eq!(app.provider_command_problem(" /CHECK   main "), None);
    assert_eq!(
        app.provider_command_problem("/review\nmain").as_deref(),
        Some("Provider slash commands must be a single line.")
    );
    assert_eq!(
        app.provider_command_problem("/missing").as_deref(),
        Some("/missing is not in this session's provider command catalogue.")
    );
    assert_eq!(
        app.provider_command_problem("/model opus").as_deref(),
        Some(
            "Interface owns /model. Use its visible model, effort, access, title, or session controls instead; nothing was sent to the provider."
        )
    );
    write_lock(&app.controller.launch_commands).known = false;
    assert_eq!(
        app.provider_command_problem("/review main").as_deref(),
        Some("Provider commands are refreshing; nothing was sent.")
    );
}

fn queued_failure(key: &str, text: &str, kind: AgentDeliveryKind) -> QueuedSendFailure {
    QueuedSendFailure {
        key: key.into(),
        text: text.into(),
        kind,
        message: None,
    }
}

fn transcript_notices(app: &TuiApp, key: &str) -> Vec<String> {
    read_lock(&app.controller.state).sessions[key]
        .transcript
        .iter()
        .filter_map(|cell| match cell {
            TranscriptCell::Notice { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn queued_failure_app(key: &str, failure: QueuedSendFailure) -> Fixture<TuiApp> {
    let app = app(TuiMode::Fullscreen);
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), session(key, AgentStatus::Idle));
    app.controller.set_active(Some(key.into()));
    write_lock(&app.controller.queued_failures).push_back(failure);
    app
}

#[test]
fn only_an_unconfirmed_auto_send_hands_the_queued_prompt_back() {
    assert!(
        queued_send_failure(
            "claude#queued",
            "hello".into(),
            &AgentDelivery::accepted(false)
        )
        .is_none()
    );
    let unsent = queued_send_failure(
        "claude#queued",
        "hello".into(),
        &AgentDelivery {
            kind: AgentDeliveryKind::DefinitelyUnsent,
            message: Some("The engine is gone.".into()),
            queued: false,
        },
    )
    .expect("a proven refusal is handed back to the UI");
    assert_eq!(unsent.text, "hello");
    assert!(unsent.transcript_notice().contains("hello"));
    assert!(unsent.transcript_notice().contains("The engine is gone."));
}

#[test]
fn a_queued_prompt_proven_unsent_returns_to_an_empty_composer() {
    let key = "claude#queued";
    let mut app = queued_failure_app(
        key,
        queued_failure(key, "queued prompt", AgentDeliveryKind::DefinitelyUnsent),
    );
    app.input.clear();

    app.apply_queued_send_failures();

    assert_eq!(app.input.text(), "queued prompt");
    assert!(transcript_notices(&app, key).is_empty());
    assert!(
        app.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("back in the composer")),
        "{:?}",
        app.notice
    );
}

#[test]
fn a_queued_prompt_never_clobbers_a_draft_and_is_never_resent_when_unknown() {
    let key = "claude#queued";
    let mut app = queued_failure_app(
        key,
        queued_failure(key, "queued prompt", AgentDeliveryKind::DefinitelyUnsent),
    );
    app.input.replace("typed since".into());

    app.apply_queued_send_failures();

    assert_eq!(app.input.text(), "typed since");
    let notices = transcript_notices(&app, key);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("queued prompt"), "{notices:?}");

    let mut app = queued_failure_app(
        key,
        queued_failure(key, "queued prompt", AgentDeliveryKind::OutcomeUnknown),
    );
    app.input.clear();

    app.apply_queued_send_failures();

    assert!(
        app.input.is_empty(),
        "an unknown outcome must never be auto-restored for resend"
    );
    let notices = transcript_notices(&app, key);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("queued prompt"), "{notices:?}");
    assert!(notices[0].contains("not resent"), "{notices:?}");
    assert!(read_lock(&app.controller.queued_failures).is_empty());
}

#[tokio::test]
async fn provider_dispatch_preserves_exact_text_and_rejects_unknown_before_queueing() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#provider-command";
    let mut row = session(key, AgentStatus::Working);
    row.commands = CommandCatalogue {
        known: true,
        commands: vec![command("review", &["check"])],
    };
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));

    let exact = " /CHECK   main ";
    app.input.replace(exact.into());
    app.submit().await;
    settle(&mut app).await;
    assert_eq!(
        read_lock(&app.controller.state).sessions[key].queued,
        VecDeque::from([exact.to_owned()])
    );
    assert_eq!(
        app.notice.as_deref(),
        Some("Prompt queued for the active session.")
    );

    app.input.replace("/not-advertised".into());
    app.submit().await;
    assert!(
        !app.effect_pending(),
        "a refused command reaches no provider"
    );
    assert_eq!(
        read_lock(&app.controller.state).sessions[key].queued,
        VecDeque::from([exact.to_owned()])
    );
    assert_eq!(
        app.notice.as_deref(),
        Some("/not-advertised is not in this session's provider command catalogue.")
    );
    assert_eq!(
        app.input.text(),
        "/not-advertised",
        "a refused command goes back to the composer"
    );
}

#[tokio::test]
async fn a_start_that_never_reached_a_provider_gives_the_prompt_back() {
    // The fixture runtime has no adapters: the start is refused unsent.
    let mut app = app(TuiMode::Fullscreen);
    app.input.replace("a long first prompt".into());
    app.submit().await;
    assert!(app.input.is_empty(), "the prompt left the composer at once");
    settle(&mut app).await;

    assert_eq!(app.input.text(), "a long first prompt");
    assert!(app.delivery_unknown.is_none());
    assert_eq!(
        app.notice.as_deref(),
        Some("engine claude is not registered")
    );
}

#[tokio::test]
async fn a_send_refused_before_the_provider_gives_the_prompt_back() {
    for (status, live, notice) in [
        (AgentStatus::Idle, false, "Agent session is not live."),
        (
            AgentStatus::Blocked,
            true,
            "Agent session is blocked on an approval.",
        ),
    ] {
        let mut app = app(TuiMode::Fullscreen);
        let key = "claude#refused";
        let mut row = session(key, status);
        row.state.live = live;
        write_lock(&app.controller.state)
            .sessions
            .insert(key.into(), row);
        app.controller.set_active(Some(key.into()));
        app.input.replace("keep me".into());

        app.submit().await;
        settle(&mut app).await;

        assert_eq!(app.input.text(), "keep me", "{notice}");
        assert_eq!(app.notice.as_deref(), Some(notice));
        assert!(app.delivery_unknown.is_none());
    }
}

#[test]
fn a_start_is_unknown_unless_proven_unsent_or_rejected() {
    use interface_engine::EffectCertainty;
    use interface_runtime::{RuntimeError, RuntimeErrorKind, SessionOpenOutcome};
    let error =
        |certainty| RuntimeError::new(RuntimeErrorKind::Lifecycle, "start", false, certainty);
    let unknown = |outcome| {
        started_session(outcome)
            .expect_err("a failed start")
            .unknown
    };
    assert!(!unknown(SessionOpenOutcome::DefinitelyUnsent(error(
        EffectCertainty::DefinitelyUnsent
    ))));
    assert!(!unknown(SessionOpenOutcome::DefinitelyRejected(error(
        EffectCertainty::DefinitelyRejected
    ))));
    assert!(unknown(SessionOpenOutcome::OutcomeUnknown(error(
        EffectCertainty::OutcomeUnknown
    ))));
    assert!(unknown(SessionOpenOutcome::ProviderReadyLocalFailure {
        provider: interface_runtime::ProviderSessionRef {
            engine: EngineId::new("claude").expect("engine"),
            session_id: "started".into(),
        },
        error: error(EffectCertainty::DefinitelyUnsent),
    }));
}

#[tokio::test]
async fn an_unknown_start_shows_the_banner_and_its_resend_starts_again() {
    let mut app = app(TuiMode::Fullscreen);
    app.input.replace(String::new());
    app.finish_effect(EffectOutcome::Start {
        text: "first prompt".into(),
        result: Err(StartFailure::unknown(protocol(
            AgentErrorCode::Failed,
            "provider readiness ended without a result",
        ))),
    })
    .await;

    assert!(app.input.is_empty(), "an unknown outcome is never put back");
    assert_eq!(
        app.delivery_unknown,
        Some(UnknownDelivery {
            key: None,
            text: "first prompt".into(),
            message: "provider readiness ended without a result".into(),
        })
    );
    let screen = rendered(80, 20, &mut app);
    assert!(screen.contains("Delivery outcome unknown"), "{screen}");
    assert!(screen.contains("r resend · x discard"), "{screen}");

    // `r` starts again; the fixture refuses it unsent, so the prompt comes
    // back and the banner goes.
    app.terminal_input(key_event(KeyCode::Char('r'), KeyModifiers::NONE))
        .await;
    assert!(app.effect_pending());
    settle(&mut app).await;
    assert!(app.delivery_unknown.is_none());
    assert_eq!(app.input.text(), "first prompt");
    assert_eq!(
        app.notice.as_deref(),
        Some("engine claude is not registered")
    );
}

fn key_event(code: KeyCode, modifiers: KeyModifiers) -> HeldInput {
    HeldInput::Key(KeyEvent::new(code, modifiers))
}

async fn type_text(app: &mut TuiApp, text: &str) {
    for character in text.chars() {
        app.terminal_input(key_event(KeyCode::Char(character), KeyModifiers::NONE))
            .await;
    }
}

#[tokio::test]
async fn a_send_leaves_the_key_handler_and_later_input_waits_in_order() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#effects";
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), session(key, AgentStatus::Working));
    app.controller.set_active(Some(key.into()));
    app.input.replace("first".into());

    app.terminal_input(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    assert!(app.effect_pending());
    assert_eq!(app.effect_label(), Some("Sending…"));
    assert!(app.input.is_empty(), "the prompt left the composer at once");
    assert!(
        read_lock(&app.controller.state).sessions[key]
            .queued
            .is_empty(),
        "the key handler did not run the provider operation"
    );

    // Typed while the send is pending, as a terminal would buffer it: a
    // second Enter on the same prompt, then a new prompt and its Enter.
    app.terminal_input(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    type_text(&mut app, "second").await;
    app.terminal_input(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    assert!(app.input.is_empty(), "held keys wait for the result");

    settle(&mut app).await;
    assert_eq!(
        read_lock(&app.controller.state).sessions[key].queued,
        VecDeque::from(["first".to_owned(), "second".to_owned()]),
        "one provider operation per prompt, in the order typed"
    );
    assert!(!app.effect_pending());
    assert_eq!(
        app.notice.as_deref(),
        Some("Prompt queued for the active session.")
    );
}

#[tokio::test]
async fn a_failed_auto_send_is_restored_after_the_result_and_before_held_typing() {
    let key = "claude#queued";
    let mut app = queued_failure_app(
        key,
        queued_failure(key, "queued prompt", AgentDeliveryKind::DefinitelyUnsent),
    );
    write_lock(&app.controller.state)
        .sessions
        .get_mut(key)
        .expect("session")
        .state
        .status = AgentStatus::Working;
    app.input.replace("first".into());
    app.terminal_input(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    type_text(&mut app, "x").await;

    settle(&mut app).await;
    // As before: the result, then the frame that restored the unsent
    // prompt into the empty composer, then the key typed meanwhile.
    assert_eq!(app.input.text(), "queued promptx");
    assert!(
        app.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("back in the composer")),
        "{:?}",
        app.notice
    );
}

#[tokio::test]
async fn ctrl_c_quits_while_an_effect_is_pending_and_starts_report_their_engine() {
    let mut app = app(TuiMode::Fullscreen);
    app.input.replace("hello".into());
    app.terminal_input(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    assert_eq!(app.effect_label(), Some("Starting Claude…"));
    assert!(read_lock(&app.controller.state).sessions.is_empty());

    type_text(&mut app, "held").await;
    app.terminal_input(key_event(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .await;
    assert!(app.quit, "Ctrl+C does not wait for the provider");

    // The start still settles through the UI path it always used: this
    // runtime registers no engines, so the refusal becomes the notice. The
    // prompt was never sent, so it comes back, ahead of the keys held
    // meanwhile.
    app.quit = false;
    settle(&mut app).await;
    assert!(
        app.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("not registered")),
        "{:?}",
        app.notice
    );
    assert_eq!(app.input.text(), "helloheld");
}

#[test]
fn slash_catalogue_filters_provider_collisions_and_keeps_argument_drafts() {
    let app = app(TuiMode::Fullscreen);
    *write_lock(&app.controller.launch_commands) = CommandCatalogue {
        known: true,
        commands: vec![
            command("review", &["check"]),
            command("model", &[]),
            command("sessions", &[]),
        ],
    };
    let items = app.slash_items();
    let provider = items
        .iter()
        .filter(|item| item.badge.as_deref() == Some("provider"))
        .collect::<Vec<_>>();
    assert_eq!(provider.len(), 1);
    assert_eq!(provider[0].label, "/review");
    assert_eq!(
        provider[0].action,
        PaletteAction::SetDraft("/review ".into())
    );
    assert_eq!(
        slash_picker_indices(&items, "rev")
            .into_iter()
            .map(|index| items[index].label.as_str())
            .collect::<Vec<_>>(),
        ["/review"]
    );
}

#[tokio::test]
async fn every_requested_launch_palette_is_reachable_and_narrow_safe() {
    let mut app = app(TuiMode::Fullscreen);
    install_probe(
        &app.controller,
        "claude",
        vec![model("default-model", &["low", "high"], true)],
        &["low", "high"],
        &["default", "plan", "bypassPermissions"],
    );
    install_probe(
        &app.controller,
        "codex",
        vec![model("shared-model", &["medium"], false)],
        &["medium"],
        &["untrusted", "on-request", "never"],
    );

    app.open_model_picker();
    let OverlayState::Picker(models) = app.overlay.as_ref().expect("model picker") else {
        panic!("expected model picker");
    };
    assert_eq!(models.kind, PaletteKind::Models);
    assert!(
        models
            .items
            .iter()
            .any(|item| item.label.contains("default-model display"))
    );

    app.open_effort_picker();
    let OverlayState::Picker(efforts) = app.overlay.as_ref().expect("effort picker") else {
        panic!("expected effort picker");
    };
    assert_eq!(efforts.kind, PaletteKind::Effort);
    assert!(efforts.items.iter().any(|item| item.label == "High"));

    app.open_access_picker();
    let OverlayState::Picker(access) = app.overlay.as_ref().expect("access picker") else {
        panic!("expected access picker");
    };
    assert_eq!(access.kind, PaletteKind::Access);
    assert!(access.items.iter().any(|item| {
        item.label == "bypassPermissions"
            && item.description.as_deref() == Some("explicit confirmation required")
    }));
    assert!(rendered(38, 12, &mut app).contains("Access mode"));

    app.engine_command("").await;
    let OverlayState::Picker(engines) = app.overlay.as_ref().expect("engine picker") else {
        panic!("expected engine picker");
    };
    assert_eq!(engines.kind, PaletteKind::Engine);
    assert_eq!(engines.items.len(), 2);

    let root = tempfile::tempdir().expect("cwd temp");
    let alpha = root.path().join("Alpha Project");
    std::fs::create_dir(&alpha).expect("alpha");
    std::os::unix::fs::symlink(&alpha, root.path().join("Alpha Link")).expect("symlink");
    write_lock(&app.controller.launch).cwd = root.path().to_string_lossy().into_owned();
    app.cwd_command("Al").await;
    let OverlayState::Picker(cwd) = app.overlay.as_ref().expect("cwd picker") else {
        panic!("expected cwd picker");
    };
    assert_eq!(cwd.kind, PaletteKind::Cwd);
    assert_eq!(
        cwd.items.len(),
        1,
        "symlink directories are not completions"
    );
    assert_eq!(cwd.items[0].label, "Alpha Project");
}

#[test]
fn session_picker_filter_preview_and_window_follow_selection() {
    let mut app = app(TuiMode::Fullscreen);
    let mut first = session("claude#one", AgentStatus::Idle);
    first.state.title = "One session".into();
    first.transcript.push(TranscriptCell::Message {
        role: "assistant",
        markdown: "first digest".into(),
    });
    let mut second = session("codex#two", AgentStatus::NotLive);
    second.state.title = "Two session".into();
    second.last_activity = "2026-08-27T01:00:00.000Z".into();
    write_lock(&app.controller.state).sessions.extend([
        (first.state.key.clone(), first),
        (second.state.key.clone(), second),
    ]);
    app.open_session_picker();
    let OverlayState::Picker(picker) = app.overlay.as_ref().expect("session picker") else {
        panic!("expected session picker");
    };
    assert_eq!(picker.kind, PaletteKind::Sessions);
    assert_eq!(picker.items[0].label, "Two session");
    assert_eq!(filtered_picker_indices(&picker.items, "claude"), [1]);
    assert!(
        picker.items[1]
            .preview
            .as_deref()
            .is_some_and(|preview| preview.contains("first digest"))
    );
    assert_eq!(picker_window(&[0, 1, 2, 3, 4, 5], 4, 3), (vec![2, 3, 4], 2));
}

#[tokio::test]
async fn unsafe_access_requires_the_exact_second_selection() {
    let mut app = app(TuiMode::Fullscreen);
    install_probe(
        &app.controller,
        "claude",
        Vec::new(),
        &[],
        &["default", "bypassPermissions"],
    );
    app.set_access("bypassPermissions".into()).await;
    assert_eq!(
        app.pending_access_confirmation,
        Some(("claude".into(), "bypassPermissions".into(), None))
    );
    assert_eq!(app.controller.launch().access.as_deref(), Some("default"));
    assert_eq!(
        app.notice.as_deref(),
        Some(
            "Select or enter bypassPermissions once more. It is not auto-restorable and may allow actions without asking."
        )
    );
}

#[tokio::test]
async fn steer_history_picker_and_details_use_exact_session_records() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#steers";
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), session(key, AgentStatus::Working));
    app.controller.set_active(Some(key.into()));
    let record = app
        .controller
        .steer_history
        .begin(
            key,
            &EngineId::new("claude").expect("engine"),
            Some("fable"),
            "change direction",
            Some("2026-08-24T12:00:00.000Z"),
        )
        .expect("begin");
    app.controller
        .steer_history
        .settle(
            &record.id,
            SteerSettledOutcome::OutcomeUnknown,
            Some("2026-08-24T12:00:01.000Z"),
            Some("transport closed"),
        )
        .expect("settle");
    app.controller
        .steer_history
        .begin(
            "claude#other",
            &EngineId::new("claude").expect("engine"),
            None,
            "other session",
            Some("2026-08-24T12:00:02.000Z"),
        )
        .expect("other begin");

    app.open_steer_history();
    let OverlayState::Picker(picker) = app.overlay.as_ref().expect("steer picker") else {
        panic!("expected steer picker");
    };
    assert_eq!(picker.kind, PaletteKind::Steers);
    assert_eq!(picker.items.len(), 1);
    assert_eq!(
        picker.items[0].description.as_deref(),
        Some("change direction")
    );
    assert_eq!(picker.items[0].badge.as_deref(), Some("outcome unknown"));
    let action = picker.items[0].action.clone();
    app.execute_palette_action(action).await;
    let OverlayState::Details { title, lines, .. } = app.overlay.as_ref().expect("details") else {
        panic!("expected steer details");
    };
    assert_eq!(title, "Steering attempt");
    assert!(lines.iter().any(|line| line == "outcome · outcome unknown"));
    assert!(lines.iter().any(|line| line == "detail"));
    assert!(lines.iter().any(|line| line == "transport closed"));
}

#[tokio::test]
async fn details_overlay_scroll_offset_counts_down_from_the_top() {
    let mut app = app(TuiMode::Fullscreen);
    app.show_details(
        "Report",
        (0..40).map(|index| format!("line {index}")).collect(),
    );
    let scroll = |app: &TuiApp| {
        let Some(OverlayState::Details { scroll, .. }) = app.overlay.as_ref() else {
            panic!("expected details");
        };
        *scroll
    };

    assert_eq!(scroll(&app), 0);
    app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .await;
    assert_eq!(scroll(&app), 0);
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await;
    assert_eq!(scroll(&app), 1);
    app.key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .await;
    assert_eq!(scroll(&app), 9);
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
        .await;
    assert_eq!(scroll(&app), 0);
    app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
        .await;
    assert_eq!(scroll(&app), u16::MAX);
}

#[tokio::test]
async fn question_choices_preserve_raw_answers_and_modifier_keys_do_nothing() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#questions";
    let raw_question = "Pick\u{1b}]52;c;hidden\u{7} now";
    let raw_label = "One\u{1b}[2Jraw";
    let approval = build_approval(
        "approval-raw",
        "AskUserQuestion",
        "Choose",
        &json!({"questions":[
            {"question":raw_question,"header":"Choice","multiSelect":false,"options":[
                {"label":raw_label,"description":"First"},
                {"label":"Two","description":"Second"}
            ]},
            {"question":"More?","header":"Multiple","multiSelect":true,"options":[
                {"label":"Alpha","description":"First"},
                {"label":"Beta","description":"Second"}
            ]}
        ]}),
    );
    let mut row = session(key, AgentStatus::Blocked);
    row.pending_approval = Some(approval);
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));

    app.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL))
        .await;
    app.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
        .await;
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await;
    app.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
        .await;
    app.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE))
        .await;

    {
        let state = read_lock(&app.controller.state);
        let approval = state.sessions[key]
            .pending_approval
            .as_ref()
            .expect("approval");
        let ApprovalKind::Questions {
            questions,
            selections,
            active_question,
        } = &approval.kind
        else {
            panic!("expected valid questions");
        };
        assert_eq!(*active_question, 1);
        assert_eq!(selections.get(&0), Some(&vec![0]));
        assert_eq!(selections.get(&1), Some(&vec![0, 1]));
        assert_eq!(
            answers_for_questions(questions, selections),
            Some(vec![
                (raw_question.into(), raw_label.into()),
                ("More?".into(), "Alpha,Beta".into())
            ])
        );
    }
    app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .await;
    let output = rendered(64, 16, &mut app);
    assert!(output.contains("Pick now"));
    assert!(output.contains("1. (●) Oneraw"));
    assert!(!output.contains("hidden"));
}

#[test]
fn malformed_questions_are_deny_only() {
    let approval = build_approval(
        "bad",
        "AskUserQuestion",
        "Choose",
        &json!({"questions":[{"question":"Which?","header":"Choice","multiSelect":false,
            "options":[{"label":"Only","description":"one"}]}]}),
    );
    let ApprovalKind::InvalidQuestions { reason } = approval.kind else {
        panic!("expected deny-only approval");
    };
    assert!(reason.contains("requires between 2 and 4 options"));
}

#[test]
fn orchestration_slash_argument_parsers_match_the_legacy_grammar() {
    assert_eq!(
        parse_spawn_argument("  acp:grok reviewer  inspect  this\ncarefully  "),
        Some(commands::SpawnArgument {
            engine: "acp:grok",
            name: "reviewer",
            prompt: "inspect  this\ncarefully",
            model: None,
            effort: None
        })
    );
    assert_eq!(parse_spawn_argument("codex reviewer"), None);
    assert_eq!(parse_spawn_argument("codex reviewer   "), None);

    assert_eq!(
        parse_send_argument(" recipient  --from sender --expect-reply  report   status "),
        Some(ParsedSendArgument {
            recipient_agent_id: "recipient".into(),
            body: "report status".into(),
            expect_reply: true,
            sender_agent_id: Some("sender".into()),
        })
    );
    assert_eq!(
        parse_send_argument("recipient first --expect-reply second --expect-reply"),
        Some(ParsedSendArgument {
            recipient_agent_id: "recipient".into(),
            body: "first second --expect-reply".into(),
            expect_reply: true,
            sender_agent_id: None,
        })
    );
    assert_eq!(
        parse_send_argument("recipient --from --expect-reply body"),
        None
    );
    assert_eq!(parse_send_argument("recipient --expect-reply"), None);

    assert!(is_engine_id("claude"));
    assert!(is_engine_id("codex"));
    assert!(is_engine_id("acp:0"));
    assert!(is_engine_id(&format!("acp:{}", "a".repeat(64))));
    assert!(!is_engine_id("Claude"));
    assert!(!is_engine_id("acp:-grok"));
    assert!(!is_engine_id("acp:grok-"));
    assert!(!is_engine_id(&format!("acp:{}", "a".repeat(65))));
}

#[test]
fn image_summaries_and_terminal_notifications_match_legacy_safe_chrome() {
    assert_eq!(image_attachment_summary(0), "[image 1]");
    assert_eq!(image_attachment_summary(1), "[image 1]");
    assert_eq!(image_attachment_summary(3), "[image 1] · +2 more");

    let unsafe_notification = TerminalNotification {
        title: "Approve\u{1b}]52;bad".into(),
        body: "tool\u{7}".into(),
    };
    assert_eq!(
        terminal_notification_sequence(&unsafe_notification, false),
        "\u{7}\u{1b}]9;Approve ]52;bad: tool\u{1b}\\"
    );
    assert_eq!(
        terminal_notification_sequence(&unsafe_notification, true),
        ""
    );
    assert_eq!(notification_part(&"x".repeat(241)).chars().count(), 240);
}

#[test]
fn rich_transcript_renders_markdown_command_folds_diffs_tools_and_plans() {
    let cells = vec![
        TranscriptCell::Message {
            role: "assistant",
            markdown: "# Result\n**bold** and `code`\n- item\n```rust\nlet ok = true;\n```\n| A | B |\n|---|---|\n| 1 | 2 |".into(),
        },
        TranscriptCell::Tool {
            call_id: "one".into(),
            tool: "Bash".into(),
            summary: "secret command one".into(),
            input_preview: "secret input one".into(),
            result: Some(ToolResultCell {
                ok: true,
                summary: "done".into(),
            }),
            command_activity: true,
        },
        TranscriptCell::Tool {
            call_id: "two".into(),
            tool: "commandExecution".into(),
            summary: "secret command two".into(),
            input_preview: "secret input two".into(),
            result: Some(ToolResultCell {
                ok: false,
                summary: "exit 1".into(),
            }),
            command_activity: true,
        },
        TranscriptCell::Tool {
            call_id: "read".into(),
            tool: "Read".into(),
            summary: "README.md".into(),
            input_preview: "README.md".into(),
            result: Some(ToolResultCell {
                ok: true,
                summary: "done".into(),
            }),
            command_activity: false,
        },
        TranscriptCell::Diff {
            call_id: "read".into(),
            path: "src/main.rs".into(),
            before: Some("old".into()),
            after: Some("new".into()),
            withheld: false,
            truncated: false,
        },
        TranscriptCell::Plan {
            entries: vec![PlanEntry {
                content: "ship parity".into(),
                priority: PlanEntryPriority::High,
                status: PlanEntryStatus::Pending,
            }],
        },
    ];
    let plain = transcript_plain_lines(&cells).join("\n");
    assert!(plain.contains("Result"));
    assert!(plain.contains("bold and code"));
    assert!(plain.contains("• item"));
    assert!(plain.contains("  let ok = true;"));
    assert!(plain.contains("A │ B"));
    assert!(plain.contains("✗ Ran 2 commands · 1 failed"));
    assert!(!plain.contains("secret command"));
    assert!(!plain.contains("secret input"));
    assert!(plain.contains("• Read · README.md"));
    assert!(plain.contains("• Edited src/main.rs"));
    assert!(!plain.lines().any(|line| line.trim() == "old"));
    assert!(!plain.lines().any(|line| line.trim() == "new"));
    assert!(plain.contains("○ ship parity [high]"));
}

#[test]
fn credential_shaped_diffs_are_withheld_in_cards_and_details() {
    let mut row = session("claude#diff", AgentStatus::Idle);
    reduce_event(
        &mut row,
        &EngineEvent {
            at: None,
            event: EngineEventKind::ToolCall {
                call_id: "edit".into(),
                tool: "Edit".into(),
                summary: "credentials.json".into(),
                input: json!({"path":"credentials.json"}),
                agent_key: None,
            },
        },
    );
    reduce_event(
        &mut row,
        &EngineEvent {
            at: None,
            event: EngineEventKind::DiffProduced {
                call_id: "edit".into(),
                path: "credentials.json".into(),
                before: "fixture-before".into(),
                after: "fixture-after".into(),
                agent_key: None,
            },
        },
    );
    assert!(matches!(
        row.transcript.last(),
        Some(TranscriptCell::Diff {
            before: None,
            after: None,
            withheld: true,
            ..
        })
    ));

    let mut app = app(TuiMode::Fullscreen);
    write_lock(&app.controller.state)
        .sessions
        .insert(row.state.key.clone(), row);
    app.controller.set_active(Some("claude#diff".into()));
    app.show_tool_details("edit");
    let OverlayState::Details { lines, .. } = app.overlay.as_ref().expect("details") else {
        panic!("expected details");
    };
    assert!(lines.iter().any(|line| line == "content withheld"));
    assert!(!lines.iter().any(|line| line.contains("fixture-before")));
    assert!(!lines.iter().any(|line| line.contains("fixture-after")));
    assert!(rendered(36, 12, &mut app).contains("content withheld"));
}

#[test]
fn a_huge_diff_keeps_a_capped_side_and_details_say_so() {
    let mut row = session("claude#diff", AgentStatus::Idle);
    // Two-byte characters, so the cut has to find a character boundary.
    let after = "\u{e9}".repeat(512 * 1024);
    for event in [
        EngineEventKind::ToolCall {
            call_id: "write".into(),
            tool: "Write".into(),
            summary: "big.txt".into(),
            input: json!({"path":"big.txt"}),
            agent_key: None,
        },
        EngineEventKind::DiffProduced {
            call_id: "write".into(),
            path: "big.txt".into(),
            before: "small".into(),
            after: after.clone(),
            agent_key: None,
        },
    ] {
        reduce_event(&mut row, &EngineEvent { at: None, event });
    }
    let Some(TranscriptCell::Diff {
        before: Some(before),
        after: Some(kept),
        truncated: true,
        ..
    }) = row.transcript.last()
    else {
        panic!("expected a truncated diff");
    };
    assert_eq!(before, "small");
    assert_eq!(kept.len(), reducer::DIFF_SIDE_LIMIT_BYTES);
    assert!(after.starts_with(kept.as_str()));

    let mut app = app(TuiMode::Fullscreen);
    write_lock(&app.controller.state)
        .sessions
        .insert(row.state.key.clone(), row);
    app.controller.set_active(Some("claude#diff".into()));
    app.show_tool_details("write");
    let OverlayState::Details { lines, .. } = app.overlay.as_ref().expect("details") else {
        panic!("expected details");
    };
    assert!(
        lines
            .iter()
            .any(|line| line == "truncated: each side keeps its first 64 KiB")
    );
}

#[test]
fn a_transcript_releases_its_oldest_diff_text_past_its_budget() {
    let diff = |serial: usize| TranscriptCell::Diff {
        call_id: format!("edit-{serial}"),
        path: format!("file-{serial}.txt"),
        before: Some("b".repeat(reducer::DIFF_SIDE_LIMIT_BYTES)),
        after: Some("a".repeat(reducer::DIFF_SIDE_LIMIT_BYTES)),
        withheld: false,
        truncated: false,
    };
    let mut transcript = Transcript::new(Vec::new());
    for serial in 0..100 {
        transcript.push(diff(serial));
    }
    let holding = |cell: &TranscriptCell| {
        matches!(
            cell,
            TranscriptCell::Diff {
                before: Some(_),
                after: Some(_),
                truncated: false,
                ..
            }
        )
    };
    let released = |cell: &TranscriptCell| {
        matches!(
            cell,
            TranscriptCell::Diff {
                before: None,
                after: None,
                truncated: true,
                ..
            }
        )
    };
    // Every cell stays; the oldest gave up their text, the newest kept it.
    assert_eq!(transcript.len(), 100);
    let kept = transcript.iter().filter(|cell| holding(cell)).count();
    assert!(kept > 0 && kept < 100, "{kept}");
    assert!(transcript[..100 - kept].iter().all(released));
    assert!(transcript[100 - kept..].iter().all(holding));
    assert!(kept * 2 * reducer::DIFF_SIDE_LIMIT_BYTES <= 4 * 1024 * 1024);

    // Text that leaves with its cells is counted out: after a clear, and
    // after the cell limit drops diffs, a full budget of new diffs keeps
    // all of its text.
    transcript.clear();
    for serial in 100..103 {
        transcript.push(diff(serial));
    }
    for _ in 0..TRANSCRIPT_LIMIT {
        transcript.push(TranscriptCell::Notice {
            message: "filler".into(),
            tone: CellTone::Info,
        });
    }
    let budget = 4 * 1024 * 1024 / (2 * reducer::DIFF_SIDE_LIMIT_BYTES);
    for serial in 0..budget {
        transcript.push(diff(200 + serial));
    }
    assert_eq!(
        transcript.iter().filter(|cell| holding(cell)).count(),
        budget
    );
}

#[test]
fn transcript_read_preserves_exact_multiline_tail_order_and_sanitization() {
    let controller = controller();
    let mut row = session("claude#one", AgentStatus::Idle);
    row.transcript = Transcript::new(vec![
        TranscriptCell::Message {
            role: "user",
            markdown: "old one\nold two".into(),
        },
        TranscriptCell::Tool {
            call_id: "ignored-tool".into(),
            tool: "Read".into(),
            summary: "ignored summary".into(),
            input_preview: "ignored input".into(),
            result: None,
            command_activity: false,
        },
        TranscriptCell::Notice {
            message: "notice one\nnotice two\u{1b}[31m".into(),
            tone: CellTone::Info,
        },
        TranscriptCell::Plan {
            entries: Vec::new(),
        },
        TranscriptCell::Message {
            role: "assistant",
            markdown: "answer one\nanswer two\u{1b}[31m".into(),
        },
    ]);
    write_lock(&controller.state)
        .sessions
        .insert("claude#one".into(), row);
    assert_eq!(
        controller.read_agent("claude#one", 4).expect("read"),
        [
            "notice: notice one",
            "notice: notice two",
            "assistant: answer one",
            "assistant: answer two",
        ]
    );
    assert_eq!(
        controller.read_agent("claude#one", 3).expect("read"),
        [
            "notice: notice two",
            "assistant: answer one",
            "assistant: answer two",
        ]
    );
    assert_eq!(
        controller
            .read_agent("claude#one", usize::MAX)
            .expect("read"),
        [
            "user: old one",
            "user: old two",
            "notice: notice one",
            "notice: notice two",
            "assistant: answer one",
            "assistant: answer two",
        ]
    );
}

#[tokio::test]
async fn queued_prompts_are_capped_as_definitely_unsent() {
    let controller = controller();
    write_lock(&controller.state).sessions.insert(
        "claude#one".into(),
        session("claude#one", AgentStatus::Working),
    );
    for index in 0..QUEUED_PROMPT_LIMIT {
        let delivery = controller
            .send_agent("claude#one", format!("q{index}"))
            .await
            .expect("queue");
        assert_eq!(delivery.kind, AgentDeliveryKind::AdapterAccepted);
    }
    let delivery = controller
        .send_agent("claude#one", "overflow".into())
        .await
        .expect("cap");
    assert_eq!(delivery.kind, AgentDeliveryKind::DefinitelyUnsent);
    assert_eq!(
        read_lock(&controller.state)
            .sessions
            .get("claude#one")
            .expect("session")
            .queued
            .len(),
        QUEUED_PROMPT_LIMIT
    );
}

#[test]
fn a_prompt_is_queued_only_while_the_turn_still_runs() {
    let controller = controller();
    let queued = |controller: &TuiController| {
        read_lock(&controller.state)
            .sessions
            .get("claude#one")
            .map(|row| row.queued.len())
    };
    // send_agent read "working", but the turn ended and the queue drained
    // before the push: the prompt must go now, not wait behind no turn.
    write_lock(&controller.state).sessions.insert(
        "claude#one".into(),
        session("claude#one", AgentStatus::Idle),
    );
    assert_eq!(
        controller
            .queue_behind_turn("claude#one", "late")
            .expect("route"),
        None
    );
    assert_eq!(queued(&controller), Some(0));
    for (status, code) in [
        (AgentStatus::Blocked, AgentErrorCode::Blocked),
        (AgentStatus::Failed, AgentErrorCode::Failed),
    ] {
        write_lock(&controller.state)
            .sessions
            .get_mut("claude#one")
            .expect("session")
            .state
            .status = status;
        let error = controller
            .queue_behind_turn("claude#one", "late")
            .expect_err("refused");
        assert_eq!(error.code, code);
        assert_eq!(queued(&controller), Some(0));
    }
    write_lock(&controller.state)
        .sessions
        .get_mut("claude#one")
        .expect("session")
        .state
        .status = AgentStatus::Working;
    let delivery = controller
        .queue_behind_turn("claude#one", "behind")
        .expect("route")
        .expect("queued");
    assert_eq!(delivery, AgentDelivery::accepted(true));
    assert_eq!(queued(&controller), Some(1));
    write_lock(&controller.state).sessions.remove("claude#one");
    assert_eq!(
        controller
            .queue_behind_turn("claude#one", "gone")
            .expect_err("removed")
            .code,
        AgentErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_refused_draft_projection_queues_nothing() {
    let mut controller = controller();
    let root = tempfile::tempdir_in("/private/tmp").expect("temp");
    std::fs::set_permissions(
        root.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("private temp");
    let prepared = match crate::collaboration_runtime::prepare(&root.path().join("c"))
        .await
        .expect("prepare")
    {
        crate::collaboration_runtime::CollaborationPreparation::Ready(prepared) => prepared,
        crate::collaboration_runtime::CollaborationPreparation::SafeUnavailable { .. } => return,
    };
    let collaboration = prepared
        .attach(Arc::new(controller.runtime.clone()))
        .await
        .expect("attach");
    // The projection already holds a later draft than this controller will
    // number next, so its next projection is refused as stale.
    collaboration
        .set_draft_state(CollaborationDraftStateUpdate {
            source_session_key: "claude#one".into(),
            revision: 100,
            empty: true,
        })
        .expect("later draft");
    controller.collaboration = Some(collaboration.clone());
    write_lock(&controller.state).sessions.insert(
        "claude#one".into(),
        session("claude#one", AgentStatus::Working),
    );
    let error = controller
        .send_agent("claude#one", "queued".into())
        .await
        .expect_err("refused projection");
    assert_eq!(error.code, AgentErrorCode::Failed);
    assert!(
        read_lock(&controller.state)
            .sessions
            .get("claude#one")
            .expect("session")
            .queued
            .is_empty()
    );
    collaboration.quiesce_and_close().await.expect("quiesce");
    controller
        .runtime
        .shutdown()
        .await
        .expect("runtime shutdown");
    collaboration.drain().await.expect("drain");
}

#[test]
fn transcript_reads_stop_at_the_response_budget_but_keep_the_newest_line() {
    let controller = controller();
    let mut row = session("claude#one", AgentStatus::Idle);
    let line = "y".repeat(64 * 1024);
    row.transcript = Transcript::new(vec![TranscriptCell::Message {
        role: "assistant",
        markdown: vec![line.as_str(); 40].join("\n"),
    }]);
    write_lock(&controller.state)
        .sessions
        .insert("claude#one".into(), row);
    let tail = controller
        .read_agent("claude#one", usize::MAX)
        .expect("read");
    assert!(!tail.is_empty() && tail.len() < 40);
    assert!(crate::agent_protocol::encoded_len(&tail) <= AGENT_RESULT_BUDGET_BYTES);

    let mut row = session("claude#two", AgentStatus::Idle);
    row.transcript = Transcript::new(vec![TranscriptCell::Message {
        role: "assistant",
        markdown: "z".repeat(AGENT_RESULT_BUDGET_BYTES + 1),
    }]);
    write_lock(&controller.state)
        .sessions
        .insert("claude#two".into(), row);
    assert_eq!(
        controller.read_agent("claude#two", 5).expect("read").len(),
        1
    );
}

#[test]
fn second_shutdown_signal_escalates_immediately_with_its_exit_code() {
    let mut signal_exit = None;

    assert_eq!(
        accept_shutdown_signal(&mut signal_exit, 130),
        ShutdownSignalAction::BeginDrain
    );
    assert_eq!(signal_exit, Some(130));
    assert_eq!(
        accept_shutdown_signal(&mut signal_exit, 143),
        ShutdownSignalAction::ForceExit(143)
    );
    assert_eq!(signal_exit, Some(143));
}

#[test]
fn collaboration_parser_matches_frozen_alias_defaults_and_limits() {
    assert_eq!(
        parse_collaboration_request(
            "openai Is this safe?",
            CollaborationEngine::Claude,
            CollaborationMode::Consult,
        ),
        Ok(ParsedCollaborationRequest {
            target: CollaborationEngine::Codex,
            objective: "Is this safe?".into(),
        })
    );
    assert_eq!(
        parse_collaboration_request(
            "x.ai",
            CollaborationEngine::Claude,
            CollaborationMode::Handoff,
        ),
        Ok(ParsedCollaborationRequest {
            target: CollaborationEngine::AcpGrok,
            objective:
                "Continue the project from its current state and complete the remaining work."
                    .into(),
        })
    );
    assert_eq!(
        parse_collaboration_request(
            "anthropic check",
            CollaborationEngine::Claude,
            CollaborationMode::Consult,
        ),
        Err("Choose a different model so this is an independent cross-model pass.".into())
    );
    assert_eq!(
        parse_collaboration_request(
            "codex",
            CollaborationEngine::Claude,
            CollaborationMode::Consult,
        ),
        Err("Add the question or handoff objective after the target model.".into())
    );
    assert!(
        parse_collaboration_request(
            &format!("codex {}", "a".repeat(4_001)),
            CollaborationEngine::Claude,
            CollaborationMode::Consult,
        )
        .is_err()
    );
    assert!(
        parse_collaboration_request(
            &format!("codex {}", "😀".repeat(2_001)),
            CollaborationEngine::Claude,
            CollaborationMode::Consult,
        )
        .is_err(),
        "the legacy limit counts JavaScript UTF-16 code units"
    );
}

#[test]
fn advisor_parser_uses_the_other_engine_and_preserves_explicit_targets() {
    assert_eq!(
        parse_advisor_argument("", CollaborationEngine::Claude, "Review the latest turn",),
        Ok(ParsedCollaborationRequest {
            target: CollaborationEngine::Codex,
            objective: "Review the latest turn".into(),
        })
    );
    assert_eq!(
        parse_advisor_argument(
            "grok focus on races",
            CollaborationEngine::Codex,
            "fallback",
        ),
        Ok(ParsedCollaborationRequest {
            target: CollaborationEngine::AcpGrok,
            objective: "focus on races".into(),
        })
    );
    assert_eq!(
        parse_advisor_argument("focus on races", CollaborationEngine::Codex, "fallback",),
        Ok(ParsedCollaborationRequest {
            target: CollaborationEngine::Claude,
            objective: "focus on races".into(),
        })
    );
}

#[test]
fn collaboration_selector_matches_number_exact_prefix_and_ambiguity() {
    let rows = ["abc-one".to_owned(), "abc-two".to_owned()];
    let refs = rows.iter().collect::<Vec<_>>();
    assert_eq!(
        select_collaboration_row(&refs, "2", String::as_str),
        Ok(&rows[1])
    );
    assert_eq!(
        select_collaboration_row(&refs, "abc-one", String::as_str),
        Ok(&rows[0])
    );
    assert_eq!(
        select_collaboration_row(&refs, "abc", String::as_str),
        Err("That collaboration ID prefix is ambiguous.".into())
    );
    assert_eq!(
        select_collaboration_row(&refs, "", String::as_str),
        Err("More than one request is pending; supply its number or ID.".into())
    );
}

#[tokio::test]
async fn collaboration_commands_keep_disabled_and_unavailable_wording() {
    let mut unavailable = app(TuiMode::Inline);
    assert!(
        unavailable
            .handle_app_command("/consult codex check this")
            .await
    );
    assert_eq!(
        unavailable.notice.as_deref(),
        Some(
            "Durable collaboration is unavailable in this iface runtime. Your source session was not changed."
        )
    );
    unavailable.handle_app_command("/collab").await;
    assert_eq!(
        unavailable.notice.as_deref(),
        Some("This runtime did not start the host-owned collaboration coordinator.")
    );

    let mut disabled = app(TuiMode::Inline);
    disabled.config.set_collaboration(false);
    disabled.handle_app_command("/handoff grok continue").await;
    assert_eq!(
        disabled.notice.as_deref(),
        Some(
            "Collaboration is disabled in your iface config (collaboration: false). Your source session was not changed."
        )
    );
    disabled.handle_app_command("/collab").await;
    assert_eq!(
        disabled.notice.as_deref(),
        Some("Collaboration is disabled in your iface config (collaboration: false).")
    );
}

#[test]
fn spawn_model_effort_are_explicit_and_prompt_flags_are_preserved_after_separator() {
    let parsed =
        parse_spawn_argument("codex worker --model chosen --effort high -- inspect --model text")
            .unwrap();
    assert_eq!(parsed.model, Some("chosen"));
    assert_eq!(parsed.effort, Some("high"));
    assert_eq!(parsed.prompt, "inspect --model text");
    for bad in [
        "codex worker --model",
        "codex worker --model --effort high task",
        "codex worker --model a --model b task",
        "codex worker --unknown x task",
    ] {
        assert!(parse_spawn_argument(bad).is_none());
    }
}

#[tokio::test]
async fn revision_frames_are_spaced_and_always_show_the_newest_state() {
    let (sender, mut revision) = watch::channel(0_u64);
    let producer = tokio::spawn(async move {
        for value in 1..=60 {
            sender.send_replace(value);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        sender
    });
    let mut cap = RevisionFrameCap::new(Instant::now());
    let mut frames: Vec<(Instant, u64)> = Vec::new();
    loop {
        tokio::time::timeout(Duration::from_secs(5), cap.next_frame(&mut revision))
            .await
            .expect("a revision frame is due");
        let shown = *revision.borrow_and_update();
        let now = Instant::now();
        cap.drew(now);
        frames.push((now, shown));
        if shown == 60 {
            break;
        }
    }
    let _sender = producer.await.expect("producer");
    // Back-to-back changes: never two frames within the interval, and the
    // last frame shows the last change.
    for pair in frames.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= REVISION_FRAME_INTERVAL);
    }
    assert_eq!(frames.last().map(|frame| frame.1), Some(60));
}

#[test]
fn indexed_tool_lookups_match_a_scan_through_drains_duplicates_and_clears() {
    fn tool(call_id: &str, serial: usize) -> TranscriptCell {
        TranscriptCell::Tool {
            call_id: call_id.into(),
            tool: "Bash".into(),
            summary: serial.to_string(),
            input_preview: String::new(),
            result: None,
            command_activity: false,
        }
    }
    fn summary(cell: Option<&TranscriptCell>) -> Option<String> {
        match cell {
            Some(TranscriptCell::Tool { summary, .. }) => Some(summary.clone()),
            _ => None,
        }
    }
    fn assert_matches_scan(transcript: &mut Transcript) {
        for call_id in ["dup", "solo", "late"] {
            let scanned = summary(transcript.iter().find(|cell| {
                matches!(cell, TranscriptCell::Tool { call_id: candidate, .. } if candidate == call_id)
            }));
            assert_eq!(
                summary(transcript.tool_mut(call_id).map(|cell| &*cell)),
                scanned,
                "{call_id}"
            );
        }
    }
    let notice = || TranscriptCell::Notice {
        message: "filler".into(),
        tone: CellTone::Info,
    };
    let mut transcript = Transcript::new(vec![tool("dup", 0), tool("dup", 1), tool("solo", 2)]);
    assert_matches_scan(&mut transcript);
    // Fill to the limit, then drop the first duplicate, then the second.
    while transcript.len() < TRANSCRIPT_LIMIT {
        transcript.push(notice());
    }
    assert_matches_scan(&mut transcript);
    transcript.push(notice());
    assert_matches_scan(&mut transcript);
    transcript.push(tool("late", 3));
    assert_matches_scan(&mut transcript);
    transcript.push(tool("dup", 4));
    assert_matches_scan(&mut transcript);
    transcript.push(notice());
    assert_matches_scan(&mut transcript);
    transcript.clear();
    assert_matches_scan(&mut transcript);
    transcript.push(tool("solo", 5));
    assert_matches_scan(&mut transcript);
}

fn screen(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let area = terminal.backend().buffer().area;
    (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn relayout(app: &mut TuiApp, width: u16) {
    let plan = app.controller.with_active_transcript(|transcript| {
        plan_transcript_layout(app.transcript_layout.as_ref(), transcript, width)
    });
    apply_transcript_plan(&mut app.transcript_layout, plan, width);
}

/// The incremental layout equals styling and wrapping every cell afresh.
fn assert_layout_is_exact(app: &mut TuiApp, width: u16) {
    relayout(app, width);
    let cells = app.controller.transcript_snapshot();
    let expected = transcript_styled_lines(&cells);
    let layout = app.transcript_layout.as_ref().expect("layout");
    assert_eq!(layout.lines, expected);
    assert_eq!(layout.row_starts, row_starts(&expected, width));
    assert_eq!(
        layout.units.iter().map(|unit| unit.lines).sum::<usize>(),
        layout.lines.len()
    );
}

fn active_row(app: &TuiApp, key: &str) -> ControlledSession {
    read_lock(&app.controller.state)
        .sessions
        .get(key)
        .cloned()
        .expect("session")
}

#[test]
fn following_output_shows_the_last_word_wrapped_row() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#wrap";
    let mut row = session(key, AgentStatus::Working);
    push_message(&mut row, "assistant", "aaaa bbbb cccc zzzz");
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));
    // Inner width 7 between the side borders; three visible rows.
    let area = Rect::new(0, 0, 9, 3);
    let mut terminal = Terminal::new(TestBackend::new(9, 3)).expect("terminal");
    terminal
        .draw(|frame| render_transcript(frame, area, &mut app))
        .expect("draw");
    let rows = screen(&terminal);
    assert!(rows[2].contains("zzzz"), "{rows:?}");
    assert!(rows[0].contains("bbbb"), "{rows:?}");
}

#[test]
fn details_overlay_end_reaches_the_last_word_wrapped_line() {
    let lines = vec!["aaaa bbbb cccc zzzz".to_owned(), "last".to_owned()];
    // Inner area 7 wide, 2 tall inside the full border.
    let area = Rect::new(0, 0, 9, 4);
    let mut terminal = Terminal::new(TestBackend::new(9, 4)).expect("terminal");
    let mut scroll = u16::MAX;
    terminal
        .draw(|frame| render_details(frame, area, "t", &lines, &mut scroll, &mut None))
        .expect("draw");
    let rows = screen(&terminal);
    assert!(rows[1].contains("zzzz"), "{rows:?}");
    assert!(rows[2].contains("last"), "{rows:?}");
    // End leaves the real last-page offset, so the next Up moves at once.
    assert_eq!(scroll, 3);
}

#[test]
fn details_overlay_measures_once_per_width_and_matches_a_full_paragraph() {
    let lines = (0..60)
        .map(|index| format!("line {index} {}", "word ".repeat(index % 7)))
        .collect::<Vec<_>>();
    let mut layout = None;
    for (width, scroll) in [(30, 0), (30, 5), (30, u16::MAX), (18, 3), (18, 40), (30, 7)] {
        let area = Rect::new(0, 0, width, 12);
        let draw = |layout: &mut Option<DetailsLayout>| {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("terminal");
            terminal
                .draw(|frame| {
                    let mut scroll = scroll;
                    render_details(frame, area, "t", &lines, &mut scroll, layout);
                })
                .expect("draw");
            screen(&terminal)
        };
        let cached = draw(&mut layout);
        assert_eq!(
            layout.as_ref().map(|measured| measured.width),
            Some(width - 2)
        );
        assert_eq!(cached, draw(&mut None), "width {width} scroll {scroll}");

        let full = lines
            .iter()
            .map(|line| Line::raw(line.as_str()))
            .collect::<Vec<_>>();
        let total = *row_starts(&full, width - 2).last().expect("total");
        let top = usize::from(scroll).min(total.saturating_sub(10));
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(full)
                        .block(Block::default().borders(Borders::ALL).title(" t "))
                        .wrap(Wrap { trim: false })
                        .scroll((u16::try_from(top).expect("top"), 0)),
                    area,
                );
            })
            .expect("draw");
        assert_eq!(cached, screen(&terminal), "width {width} scroll {scroll}");
    }
}

#[test]
fn the_windowed_transcript_matches_a_full_paragraph_at_every_scroll() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#window";
    let mut row = session(key, AgentStatus::Working);
    for text in [
        "aaaa bbbb cccc zzzz and more words wrapping at the boundary",
        "四四四四四 wide 字字字字 characters mixed with narrow ones",
        "",
        "averyveryveryverylongunbrokenwordthatcannotwrapatanyspaceatall",
        "- a list item\n- another item with `code` and **bold** text",
    ] {
        push_message(&mut row, "assistant", text);
    }
    push_notice(&mut row, "a notice between messages", CellTone::Warning);
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));
    for (width, height) in [(9_u16, 4_u16), (14, 5), (25, 7), (42, 3)] {
        let inner = width - 2;
        let lines = transcript_styled_lines(&app.controller.transcript_snapshot());
        let total = row_starts(&lines, inner)
            .last()
            .copied()
            .unwrap_or_default();
        let area = Rect::new(0, 0, width, height);
        for scroll in 0..=total {
            hold_transcript_at(&mut app, area, scroll);
            let windowed = draw_transcript(&mut app, area);
            let maximum = total.saturating_sub(usize::from(height));
            let top = scroll.min(maximum);
            let mut full = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            full.draw(|frame| {
                frame.render_widget(
                    Paragraph::new(lines.clone())
                        .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                        .wrap(Wrap { trim: false })
                        .scroll((u16::try_from(top).expect("small fixture"), 0)),
                    area,
                );
            })
            .expect("draw");
            assert_eq!(windowed, screen(&full), "width {width} scroll {scroll}");
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One seeded edit script covering every mutation site.
fn incremental_layout_matches_a_full_restyle_after_arbitrary_edits() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#edits";
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), session(key, AgentStatus::Working));
    app.controller.set_active(Some(key.into()));
    let width = 23;
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let edit = |app: &TuiApp, change: &dyn Fn(&mut ControlledSession)| {
        change(
            write_lock(&app.controller.state)
                .sessions
                .get_mut(key)
                .expect("session"),
        );
    };
    for step in 0..400_u64 {
        let choice = next(9);
        let call = next(12);
        match choice {
            0 | 1 => edit(&app, &|row| {
                push_message(row, "assistant", "several words that wrap");
            }),
            2 => edit(&app, &|row| {
                if let Some(TranscriptCell::Message { markdown, .. }) = row.transcript.last_mut() {
                    markdown.push_str(" streamed token");
                }
            }),
            3 | 4 => edit(&app, &|row| {
                push_cell(
                    row,
                    TranscriptCell::Tool {
                        call_id: format!("call-{call}-{step}"),
                        tool: "Bash".into(),
                        summary: "ls".into(),
                        input_preview: "{}".into(),
                        result: None,
                        command_activity: call % 3 != 0,
                    },
                );
            }),
            5 => edit(&app, &|row| {
                // A result for an arbitrary earlier tool cell, maybe inside a run.
                let target = row.transcript.iter().find_map(|cell| match cell {
                    TranscriptCell::Tool {
                        call_id,
                        result: None,
                        ..
                    } => Some(call_id.clone()),
                    _ => None,
                });
                if let Some(target) = target
                    && let Some(TranscriptCell::Tool { result, .. }) = find_tool_mut(row, &target)
                {
                    *result = Some(ToolResultCell {
                        ok: call % 2 == 0,
                        summary: "done".into(),
                    });
                }
            }),
            6 => edit(&app, &|row| {
                push_cell(
                    row,
                    TranscriptCell::Subagent {
                        agent_key: format!("agent-{call}"),
                        label: "Helper".into(),
                        description: "looks things up".into(),
                        model: None,
                        result: None,
                    },
                );
            }),
            7 => edit(&app, &|row| {
                if let Some(TranscriptCell::Subagent { result, .. }) = row
                    .transcript
                    .rfind_mut(|cell| matches!(cell, TranscriptCell::Subagent { .. }))
                {
                    *result = Some(ToolResultCell {
                        ok: true,
                        summary: "found".into(),
                    });
                }
            }),
            _ if step % 97 == 0 => edit(&app, &|row| row.transcript.clear()),
            _ => edit(&app, &|row| push_notice(row, "notice", CellTone::Info)),
        }
        // Several edits can land between two frames.
        if next(3) == 0 {
            assert_layout_is_exact(&mut app, width);
        }
    }
    assert_layout_is_exact(&mut app, width);
    // A width change restyles everything; switching sessions and back too.
    assert_layout_is_exact(&mut app, 40);
    app.controller.set_active(None);
    relayout(&mut app, 40);
    assert!(
        app.transcript_layout
            .as_ref()
            .expect("layout")
            .source
            .is_none()
    );
    app.controller.set_active(Some(key.into()));
    assert_layout_is_exact(&mut app, 40);
    assert!(active_row(&app, key).transcript.len() > 1);
}

#[test]
fn incremental_layout_follows_the_front_drain_at_the_transcript_limit() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#limit";
    let mut row = session(key, AgentStatus::Working);
    for index in 0..TRANSCRIPT_LIMIT {
        push_cell(
            &mut row,
            TranscriptCell::Tool {
                call_id: format!("c{index}"),
                tool: "Bash".into(),
                summary: "ls".into(),
                input_preview: "{}".into(),
                result: None,
                // Runs at the front are cut in half by the drain.
                command_activity: index % 4 != 3,
            },
        );
    }
    write_lock(&app.controller.state)
        .sessions
        .insert(key.into(), row);
    app.controller.set_active(Some(key.into()));
    assert_layout_is_exact(&mut app, 30);
    for step in 0..6 {
        write_lock(&app.controller.state)
            .sessions
            .get_mut(key)
            .map(|row| push_message(row, "assistant", &format!("after the limit {step}")))
            .expect("session");
        assert_layout_is_exact(&mut app, 30);
    }
    assert_eq!(active_row(&app, key).transcript.len(), TRANSCRIPT_LIMIT);
}

fn draw_transcript(app: &mut TuiApp, area: Rect) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal");
    terminal
        .draw(|frame| render_transcript(frame, area, app))
        .expect("draw");
    screen(&terminal)
}

/// Holds wrapped row `top` at the top of `area`, as the arrow keys would
/// leave it; the maximum and anything past it follow output.
fn hold_transcript_at(app: &mut TuiApp, area: Rect, top: usize) {
    draw_transcript(app, area);
    let layout = app.transcript_layout.as_ref().expect("layout");
    app.scroll = if top >= layout.maximum {
        TranscriptScroll::Follow
    } else {
        layout.hold_at(top).expect("row in the layout")
    };
}

/// The text between the transcript's side borders.
fn transcript_text(screen: &[String]) -> Vec<String> {
    screen
        .iter()
        .map(|line| {
            let cells = line.chars().collect::<Vec<_>>();
            cells[1..cells.len() - 1]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn activate(app: &TuiApp, row: ControlledSession) {
    let key = row.state.key.clone();
    write_lock(&app.controller.state)
        .sessions
        .insert(key.clone(), row);
    app.controller.set_active(Some(key));
}

fn push_to(app: &TuiApp, key: &str, change: impl FnOnce(&mut ControlledSession)) {
    change(
        write_lock(&app.controller.state)
            .sessions
            .get_mut(key)
            .expect("session"),
    );
}

#[test]
fn transcript_scrolling_reaches_rows_past_the_old_u16_offset() {
    const LINES: usize = 1_000;
    const CELLS: usize = 70;
    // Each prompt lays out as its blank opening row, then one row per line.
    let expected_row = |row: usize| match (row / (LINES + 1), row % (LINES + 1)) {
        (_, 0) => String::new(),
        (cell, 1) => format!("› r{cell}-0"),
        (cell, line) => format!("  r{cell}-{}", line - 1),
    };
    let mut app = app(TuiMode::Fullscreen);
    let mut row = session("claude#long", AgentStatus::Idle);
    for cell in 0..CELLS {
        let text = (0..LINES)
            .map(|line| format!("r{cell}-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        push_message(&mut row, "user", &text);
    }
    activate(&app, row);
    let area = Rect::new(0, 0, 24, 6);
    let height = usize::from(area.height);
    let maximum = CELLS * (LINES + 1) - height;
    let expected = |top: usize| (top..top + height).map(expected_row).collect::<Vec<_>>();
    assert_eq!(
        transcript_text(&draw_transcript(&mut app, area)),
        expected(maximum)
    );
    // The old offset counted u16 rows up from the newest, so no row above
    // `old_limit` could reach the top of the view.
    let old_limit = maximum - usize::from(u16::MAX);
    for top in [old_limit + 1, old_limit, old_limit - 1, 0] {
        hold_transcript_at(&mut app, area, top);
        assert_eq!(
            transcript_text(&draw_transcript(&mut app, area)),
            expected(top),
            "top {top}"
        );
    }
    // PageUp from the newest rows reaches the oldest and stops there, so
    // the first Down after over-scrolling moves the view at once.
    app.scroll = TranscriptScroll::Follow;
    draw_transcript(&mut app, area);
    for _ in 0..maximum / 8 + 3 {
        app.scroll_transcript(-8);
    }
    assert_eq!(
        transcript_text(&draw_transcript(&mut app, area)),
        expected(0)
    );
    app.scroll_transcript(1);
    assert_eq!(
        transcript_text(&draw_transcript(&mut app, area)),
        expected(1)
    );
}

#[tokio::test]
async fn a_held_transcript_stays_put_as_output_arrives_and_end_follows_again() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#held";
    let mut row = session(key, AgentStatus::Working);
    for index in 0..20 {
        push_message(&mut row, "assistant", &format!("message {index}"));
    }
    activate(&app, row);
    let area = Rect::new(0, 0, 30, 6);
    let press = |code| key_event(code, KeyModifiers::NONE);
    let newest = draw_transcript(&mut app, area);
    for _ in 0..3 {
        app.terminal_input(press(KeyCode::Up)).await;
    }
    let held = draw_transcript(&mut app, area);
    assert_ne!(held, newest);

    // Output streaming in below leaves the rows in view where they are.
    push_to(&app, key, |row| {
        for index in 20..40 {
            push_message(row, "assistant", &format!("message {index}"));
        }
    });
    assert_eq!(draw_transcript(&mut app, area), held);

    // End follows the newest output again and moves the composer cursor.
    app.input.move_start();
    app.terminal_input(press(KeyCode::End)).await;
    assert_eq!(app.scroll, TranscriptScroll::Follow);
    assert_eq!(app.input.cursor(), app.input.text().len());
    let following = transcript_text(&draw_transcript(&mut app, area));
    assert_eq!(following.last().map(String::as_str), Some("message 39"));

    // Down back onto the newest rows follows too; Up with nothing above
    // the view stays following.
    app.terminal_input(press(KeyCode::Up)).await;
    assert_ne!(app.scroll, TranscriptScroll::Follow);
    app.terminal_input(press(KeyCode::Down)).await;
    assert_eq!(app.scroll, TranscriptScroll::Follow);
    let short = "claude#short";
    let mut row = session(short, AgentStatus::Idle);
    push_message(&mut row, "assistant", "fits");
    activate(&app, row);
    draw_transcript(&mut app, area);
    app.terminal_input(press(KeyCode::Up)).await;
    assert_eq!(app.scroll, TranscriptScroll::Follow);

    // A view held on one session follows the next one it switches to.
    app.controller.set_active(Some(key.into()));
    draw_transcript(&mut app, area);
    app.terminal_input(press(KeyCode::PageUp)).await;
    assert_ne!(app.scroll, TranscriptScroll::Follow);
    app.controller.set_active(Some(short.into()));
    draw_transcript(&mut app, area);
    assert_eq!(app.scroll, TranscriptScroll::Follow);

    // Inline mode leaves the arrows and End to the composer alone.
    let mut inline = self::app(TuiMode::Inline);
    let mut row = session(key, AgentStatus::Working);
    for index in 0..20 {
        push_message(&mut row, "assistant", &format!("message {index}"));
    }
    activate(&inline, row);
    draw_transcript(&mut inline, area);
    inline.terminal_input(press(KeyCode::PageUp)).await;
    assert_eq!(inline.scroll, TranscriptScroll::Follow);
}

#[test]
fn a_held_transcript_keeps_its_rows_while_old_cells_drop_at_the_limit() {
    let mut app = app(TuiMode::Fullscreen);
    let key = "claude#full";
    let mut row = session(key, AgentStatus::Working);
    for index in 0..TRANSCRIPT_LIMIT {
        push_notice(&mut row, &format!("notice {index}"), CellTone::Info);
    }
    activate(&app, row);
    let area = Rect::new(0, 0, 30, 6);
    hold_transcript_at(&mut app, area, 1_001);
    let held = draw_transcript(&mut app, area);
    let push = |app: &TuiApp, from: usize, count: usize| {
        push_to(app, key, |row| {
            for index in from..from + count {
                push_notice(row, &format!("notice {index}"), CellTone::Info);
            }
        });
    };
    // A hundred cells drop from the front; the rows in view stay.
    push(&app, TRANSCRIPT_LIMIT, 100);
    assert_eq!(draw_transcript(&mut app, area), held);
    // Once the held cells themselves are gone, the oldest row is on top.
    push(&app, TRANSCRIPT_LIMIT + 100, 1_000);
    let dropped = draw_transcript(&mut app, area);
    assert_ne!(app.scroll, TranscriptScroll::Follow);
    hold_transcript_at(&mut app, area, 0);
    assert_eq!(draw_transcript(&mut app, area), dropped);
}

#[tokio::test]
async fn ctrl_y_sends_the_transcript_to_scrollback_and_keeps_the_notice_short() {
    for mode in [TuiMode::Fullscreen, TuiMode::Inline] {
        let mut app = app(mode);
        let mut row = session("claude#dump", AgentStatus::Idle);
        for index in 0..500 {
            push_message(&mut row, "assistant", &format!("transcript line {index}"));
        }
        activate(&app, row);
        app.terminal_input(key_event(KeyCode::Char('y'), KeyModifiers::CONTROL))
            .await;
        if mode == TuiMode::Fullscreen {
            let lines = transcript_plain_lines(&app.controller.transcript_snapshot());
            assert_eq!(lines.len(), 1_000);
            assert_eq!(app.scrollback_dump, Some(lines.join("\n")));
            assert_eq!(
                app.notice.as_deref(),
                Some("Dumped 1000 lines to scrollback.")
            );
        } else {
            // 0.1.169 bound Ctrl+Y in fullscreen only.
            assert_eq!(app.scrollback_dump, None);
            assert_eq!(app.notice, None);
        }
    }
}

#[test]
fn continue_opens_the_most_recent_session_in_the_launch_directory() {
    let controller = controller();
    let row = |key: &str, cwd: &str, last_activity: &str| {
        let mut row = session(key, AgentStatus::NotLive);
        row.state.cwd = cwd.into();
        row.last_activity = last_activity.into();
        (key.to_owned(), row)
    };
    write_lock(&controller.state).sessions.extend([
        // The lowest key, and the newest by text, but 07:30Z as an instant.
        row("claude#aaa", "/repo", "2026-09-27T09:30:00+02:00"),
        row("claude#mmm", "/repo", "2026-09-27T08:00:00.000Z"),
        row("codex#zzz", "/repo", "2026-09-26T23:59:59.000Z"),
        // Newer, but in another directory.
        row("claude#bbb", "/elsewhere", "2026-09-28T00:00:00.000Z"),
    ]);
    assert_eq!(controller.continue_target().as_deref(), Some("claude#mmm"));

    // Activity that does not parse compares as text, and an exact tie keeps
    // the session listed first.
    write_lock(&controller.state).sessions.extend([
        row("claude#two", "/repo", "later"),
        row("claude#one", "/repo", "later"),
    ]);
    assert_eq!(controller.continue_target().as_deref(), Some("claude#one"));
    write_lock(&controller.state).sessions.clear();
    assert_eq!(controller.continue_target(), None);
}
