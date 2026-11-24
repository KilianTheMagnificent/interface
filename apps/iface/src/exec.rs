use std::{path::Path, time::Duration};

use interface_contracts::engine::{
    EngineEvent, EngineEventKind, EngineId, LaunchOptions, ResumeOptions, SessionStatus,
    SessionSummary,
};
use interface_engine::DeliveryOutcome;
use interface_runtime::{RuntimeError, RuntimeEventFrame, SessionOpenOutcome, SessionRuntime};

use crate::{
    argv::{CliArgv, LaunchScope, OutputFormat},
    errors::{CLI_EXIT, CommandResult},
    sanitize::for_display,
};

const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub type ExecResult = CommandResult;

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExecOutcome {
    Accepted,
    DefinitelyUnsent(String),
    Unknown(String),
    TimedOut,
    /// `--timeout` elapsed while waiting for a session that was already
    /// contacted. The provider is left running, so the turn's outcome is
    /// unknown rather than unsent.
    WaitTimedOut(String),
}

impl ExecOutcome {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Accepted => "adapter-accepted",
            Self::DefinitelyUnsent(_) | Self::TimedOut => "definitely-unsent",
            Self::Unknown(_) | Self::WaitTimedOut(_) => "outcome-unknown",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Accepted => "",
            Self::DefinitelyUnsent(message)
            | Self::Unknown(message)
            | Self::WaitTimedOut(message) => message,
            Self::TimedOut => "exec timed out.",
        }
    }
}

/// Runs one provider turn through the shared Rust runtime. `None` is a proven
/// local refusal: no provider process is launched and the prompt is not sent.
pub async fn run(
    parsed: &CliArgv,
    cwd: &Path,
    prompt: String,
    runtime: Option<&SessionRuntime>,
) -> ExecResult {
    if prompt.trim().is_empty() {
        return ExecResult {
            stdout: String::new(),
            stderr: "iface: exec requires a prompt.\n".into(),
            exit_code: CLI_EXIT.validation,
        };
    }
    if parsed.timeout_ms == Some(0) {
        return ExecResult {
            stdout: String::new(),
            stderr: "iface: --timeout must be a positive duration.\n".into(),
            exit_code: CLI_EXIT.validation,
        };
    }
    let Some(runtime) = runtime else {
        let outcome =
            ExecOutcome::DefinitelyUnsent("No live engine host; the prompt was not sent.".into());
        return render(parsed, &[], outcome);
    };
    // `--timeout` bounds how long `iface` waits, never how long the provider
    // runs. Before dispatch it is a proven refusal; after dispatch it stops the
    // wait and reports "outcome-unknown" with the session left alive, because
    // cancelling a contacted provider would turn it into the false claim
    // "definitely-unsent".
    let (events, outcome) = run_runtime(parsed, cwd, &prompt, runtime).await;
    render(parsed, &events, outcome)
}

async fn run_runtime(
    parsed: &CliArgv,
    cwd: &Path,
    prompt: &str,
    runtime: &SessionRuntime,
) -> (Vec<EngineEvent>, ExecOutcome) {
    let started = std::time::Instant::now();
    let requested_engine = match EngineId::new(parsed.engine.as_deref().unwrap_or("claude")) {
        Ok(engine) => engine,
        Err(error) => {
            return (Vec::new(), ExecOutcome::DefinitelyUnsent(error.to_string()));
        }
    };
    let scope = parsed.scope.unwrap_or(LaunchScope::Isolated);
    let setting_sources = scope
        .setting_sources()
        .map(|sources| sources.into_iter().map(str::to_owned).collect::<Vec<_>>());

    let (open, resume_send) = if let Some(prefix) = parsed.resume.as_deref() {
        let session = match resolve_resume(runtime, prefix).await {
            Ok(session) => session,
            Err(outcome) => return (Vec::new(), outcome),
        };
        (
            match resume_request(&session) {
                Ok((resume_cwd, options)) => {
                    runtime
                        .resume_session(
                            &session.engine,
                            session.session_id.clone(),
                            resume_cwd,
                            options,
                        )
                        .await
                }
                Err(message) => {
                    return (Vec::new(), ExecOutcome::DefinitelyUnsent(message));
                }
            },
            true,
        )
    } else {
        (
            runtime
                .start_session(
                    &requested_engine,
                    LaunchOptions {
                        cwd: cwd.to_string_lossy().into_owned(),
                        prompt: prompt.to_owned(),
                        setting_sources,
                        ..LaunchOptions::default()
                    },
                )
                .await,
            false,
        )
    };

    let session = match open {
        SessionOpenOutcome::Ready(session) => session,
        SessionOpenOutcome::DefinitelyUnsent(error)
        | SessionOpenOutcome::DefinitelyRejected(error) => {
            return (Vec::new(), ExecOutcome::DefinitelyUnsent(error.message));
        }
        SessionOpenOutcome::OutcomeUnknown(error)
        | SessionOpenOutcome::ProviderReadyLocalFailure { error, .. } => {
            return (Vec::new(), delivery_unknown(&error.message));
        }
    };
    let key = session.key.clone();
    let taken = tokio::time::timeout(READY_TIMEOUT, runtime.take_session_events(&key)).await;
    let mut stream = match taken {
        Ok(Ok(stream)) => stream,
        failed => {
            // Only a resume has sent nothing yet, so only it may close the
            // session: closing a start would cancel a prompt that may run.
            if resume_send {
                let _ = runtime.close_session(&key).await;
            }
            return (
                Vec::new(),
                stream_failure(resume_send, failed.ok().and_then(Result::err)),
            );
        }
    };

    // The `--timeout` budget covers the whole run, so a resume whose deadline
    // is already spent is refused before the prompt crosses the provider seam.
    // That is a proven refusal; only this branch may close the session.
    if resume_send {
        if remaining_budget(started, parsed.timeout_ms) == Some(Duration::ZERO) {
            let _ = runtime.close_session(&key).await;
            return (Vec::new(), ExecOutcome::TimedOut);
        }
        match runtime.send(&key, prompt.to_owned()).await.outcome {
            DeliveryOutcome::Confirmed(()) => {}
            DeliveryOutcome::DefinitelyUnsent(error)
            | DeliveryOutcome::DefinitelyRejected(error) => {
                let _ = runtime.close_session(&key).await;
                return (Vec::new(), ExecOutcome::DefinitelyUnsent(error.message));
            }
            DeliveryOutcome::OutcomeUnknown(error) => {
                let _ = runtime.close_session(&key).await;
                return (Vec::new(), delivery_unknown(&error.message));
            }
        }
    }

    let (events, outcome) = wait_for_turn(&mut stream, &key, started, parsed.timeout_ms).await;
    // A session that ran out of wait budget is deliberately left open: closing
    // it would cancel a provider turn that is still running.
    if !matches!(outcome, ExecOutcome::WaitTimedOut(_)) {
        let _ = runtime.close_session(&key).await;
    }
    (events, outcome)
}

/// Waits for the turn to go quiescent, bounded by whatever is left of
/// `--timeout`. Expiry abandons the wait only; the caller keeps the session.
async fn wait_for_turn(
    stream: &mut interface_engine::BoundedEventStream<RuntimeEventFrame>,
    key: &str,
    started: std::time::Instant,
    timeout_ms: Option<u64>,
) -> (Vec<EngineEvent>, ExecOutcome) {
    let mut events = Vec::new();
    let mut saw_activity = false;
    let outcome = loop {
        let frame = match remaining_budget(started, timeout_ms) {
            None => stream.recv().await,
            Some(budget) if budget.is_zero() => break wait_timed_out(key, timeout_ms),
            Some(budget) => match tokio::time::timeout(budget, stream.recv()).await {
                Ok(frame) => frame,
                Err(_) => break wait_timed_out(key, timeout_ms),
            },
        };
        match frame {
            Ok(Some(RuntimeEventFrame::Event { value })) => {
                saw_activity |= is_activity(&value.event);
                let done = is_quiescent(&value.event, saw_activity);
                events.push(value.event);
                if done {
                    break ExecOutcome::Accepted;
                }
            }
            Ok(Some(RuntimeEventFrame::Terminal { message, .. })) => {
                break message.map_or(ExecOutcome::Accepted, ExecOutcome::Unknown);
            }
            Ok(None) => break ExecOutcome::Accepted,
            Err(error) => break ExecOutcome::Unknown(error.to_string()),
        }
    };
    (events, outcome)
}

/// How a failure to take the session's event stream is reported; `error` is
/// `None` when the take timed out. A resume takes the stream before it sends,
/// so nothing was sent. A start hands its prompt to the provider at launch,
/// so once the start was ready the outcome is unknown: exit 4, never 3.
fn stream_failure(resume_send: bool, error: Option<RuntimeError>) -> ExecOutcome {
    match (resume_send, error) {
        (true, Some(error)) => ExecOutcome::DefinitelyUnsent(error.message),
        (true, None) => ExecOutcome::TimedOut,
        (false, Some(error)) => delivery_unknown(&error.message),
        (false, None) => ExecOutcome::Unknown(
            "Timed out waiting for the session's events after the prompt was handed to the provider. Delivery unknown — Resend / Discard."
                .into(),
        ),
    }
}

/// The one stored session whose id starts with `prefix`, first by engine then
/// id. No match, an ambiguous prefix or an unreadable listing is refused
/// before any provider contact.
async fn resolve_resume(
    runtime: &SessionRuntime,
    prefix: &str,
) -> Result<SessionSummary, ExecOutcome> {
    let sessions = match runtime.list_sessions().await {
        Ok(sessions) => sessions,
        Err(error) => {
            return Err(ExecOutcome::DefinitelyUnsent(error.message));
        }
    };
    let mut matches = sessions
        .into_iter()
        .filter(|session| session.session_id.starts_with(prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.engine
            .as_str()
            .cmp(right.engine.as_str())
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    let Some(session) = matches.first().cloned() else {
        return Err(ExecOutcome::DefinitelyUnsent(format!(
            "No session matches id prefix \"{prefix}\"."
        )));
    };
    if matches.len() > 1 {
        let candidates = matches
            .iter()
            .map(|item| format!("{}#{}", item.engine, item.session_id))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ExecOutcome::DefinitelyUnsent(format!(
            "Session id prefix \"{prefix}\" is ambiguous:\n{candidates}"
        )));
    }
    Ok(session)
}

/// A prompt the provider may already hold: never resent automatically.
fn delivery_unknown(message: &str) -> ExecOutcome {
    ExecOutcome::Unknown(format!("{message} Delivery unknown — Resend / Discard."))
}

fn resume_request(session: &SessionSummary) -> Result<(String, ResumeOptions), String> {
    let key = format!("{}#{}", session.engine, session.session_id);
    let recorded = session.cwd.trim();
    if recorded.is_empty() {
        return Err(format!(
            "Cannot resume {key}: no working directory was recorded for it."
        ));
    }
    let cwd = if Path::new(recorded).is_dir() {
        recorded.to_owned()
    } else if let Some(home) = session
        .worktree_home
        .as_deref()
        .map(str::trim)
        .filter(|home| !home.is_empty() && Path::new(home).is_dir())
    {
        home.to_owned()
    } else {
        return Err(format!(
            "That folder is gone. Interface recorded this session as running in {recorded}, and that folder is no longer there — so there is nowhere to restart the agent. Nothing was changed, and the transcript is untouched: move the folder back, or start a new session where the work lives now."
        ));
    };
    Ok((
        cwd,
        ResumeOptions {
            setting_sources: session.setting_sources.clone(),
            disallowed_tools: session.disallowed_tools.clone(),
            model: remembered_setting(session.model.as_deref()),
            effort: remembered_setting(session.effort.as_ref().and_then(|value| value.as_deref())),
            ..ResumeOptions::default()
        },
    ))
}

fn remembered_setting(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "default")
        .map(str::to_owned)
}

/// How much of `--timeout` is left for the whole exec run. `None` means the
/// caller asked for no deadline; `Some(Duration::ZERO)` means it is spent.
fn remaining_budget(started: std::time::Instant, timeout_ms: Option<u64>) -> Option<Duration> {
    timeout_ms.map(|timeout_ms| Duration::from_millis(timeout_ms).saturating_sub(started.elapsed()))
}

fn wait_timed_out(key: &str, timeout_ms: Option<u64>) -> ExecOutcome {
    let budget = timeout_ms.unwrap_or_default();
    ExecOutcome::WaitTimedOut(format!(
        "exec stopped waiting after {budget} ms. The prompt reached session {key}, which was not cancelled and may still be running — reattach with --resume to read the result."
    ))
}

fn is_activity(event: &EngineEvent) -> bool {
    match &event.event {
        EngineEventKind::StatusChanged { status } => *status == SessionStatus::Working,
        EngineEventKind::MessageDelta { .. }
        | EngineEventKind::ToolCall { .. }
        | EngineEventKind::ThinkingSignal { .. }
        | EngineEventKind::UserMessage { .. } => true,
        _ => false,
    }
}

fn is_quiescent(event: &EngineEvent, saw_activity: bool) -> bool {
    match &event.event {
        EngineEventKind::SessionClosed => true,
        EngineEventKind::Error { retryable, .. } => !retryable,
        EngineEventKind::StatusChanged { status } => {
            *status == SessionStatus::Failed || (*status == SessionStatus::Idle && saw_activity)
        }
        _ => false,
    }
}

fn render(parsed: &CliArgv, events: &[EngineEvent], outcome: ExecOutcome) -> ExecResult {
    if outcome == ExecOutcome::TimedOut {
        return ExecResult {
            stdout: String::new(),
            stderr: "iface: exec timed out.\n".into(),
            exit_code: CLI_EXIT.timeout,
        };
    }
    let format = parsed.output.unwrap_or(OutputFormat::Text);
    let stdout = match format {
        OutputFormat::Text if outcome == ExecOutcome::Accepted => {
            format!("{}\n", assistant_text(events))
        }
        OutputFormat::Text => String::new(),
        OutputFormat::Json => format!("{}\n", envelope(parsed, events, &outcome)),
        OutputFormat::JsonLines => {
            let mut lines = events
                .iter()
                .enumerate()
                .map(|(sequence, event)| {
                    format!(
                        "{{\"schemaVersion\":1,\"sequence\":{sequence},\"event\":{}}}",
                        serde_json::to_string(event).expect("engine event serializes")
                    )
                })
                .collect::<Vec<_>>();
            lines.push(format!(
                "{{\"schemaVersion\":1,\"sequence\":{},\"terminal\":{}}}",
                lines.len(),
                envelope(parsed, events, &outcome)
            ));
            format!("{}\n", lines.join("\n"))
        }
    };
    let (exit_code, stderr) = match outcome {
        ExecOutcome::Accepted => (CLI_EXIT.success, String::new()),
        ExecOutcome::DefinitelyUnsent(message) => (
            CLI_EXIT.unsent,
            format!("iface: {}\n", for_display(&message)),
        ),
        ExecOutcome::Unknown(message) => (
            CLI_EXIT.unknown,
            format!("iface: {}\n", for_display(&message)),
        ),
        ExecOutcome::WaitTimedOut(message) => (
            CLI_EXIT.timeout,
            format!("iface: {}\n", for_display(&message)),
        ),
        ExecOutcome::TimedOut => unreachable!("handled above"),
    };
    ExecResult {
        stdout,
        stderr,
        exit_code,
    }
}

fn envelope(parsed: &CliArgv, events: &[EngineEvent], outcome: &ExecOutcome) -> String {
    let text = if outcome == &ExecOutcome::Accepted {
        assistant_text(events)
    } else {
        for_display(outcome.message())
    };
    let mut value = format!(
        "{{\"schemaVersion\":1,\"engine\":{},\"scope\":{},\"result\":{},\"text\":{}",
        serde_json::to_string(parsed.engine.as_deref().unwrap_or("claude"))
            .expect("engine serializes"),
        serde_json::to_string(parsed.scope.unwrap_or(LaunchScope::Isolated).as_str())
            .expect("scope serializes"),
        serde_json::to_string(outcome.kind()).expect("outcome serializes"),
        serde_json::to_string(&text).expect("text serializes")
    );
    if let Some(resume) = parsed.resume.as_deref() {
        value.push_str(",\"sessionKey\":");
        value.push_str(&serde_json::to_string(resume).expect("resume prefix serializes"));
    }
    value.push('}');
    value
}

fn assistant_text(events: &[EngineEvent]) -> String {
    let text = events
        .iter()
        .filter_map(|event| match &event.event {
            EngineEventKind::MessageDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    for_display(&text)
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::{EngineId, SessionOrigin, SessionSummary};

    use super::*;

    fn summary(cwd: String) -> SessionSummary {
        SessionSummary {
            engine: EngineId::new("claude").expect("engine"),
            session_id: "session-1".into(),
            title: "Session".into(),
            cwd,
            status: SessionStatus::Idle,
            cloud: false,
            model: Some("  claude-opus-5  ".into()),
            effort: Some(Some(" high ".into())),
            last_activity: "2026-08-27T00:00:00.000Z".into(),
            first_seen: "2026-08-27T00:00:00.000Z".into(),
            archived: false,
            size: None,
            title_from_summary: None,
            origin: Some(SessionOrigin::Store),
            project_root: None,
            worktree_home: None,
            worktree_path: None,
            setting_sources: Some(vec!["project".into()]),
            disallowed_tools: Some(vec!["Bash".into()]),
            kind: None,
            permission_mode: None,
        }
    }

    #[test]
    fn golden_text_json_and_jsonl_rendering_match_legacy() {
        let events = vec![EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "hel\u{1b}[31mlo".into(),
                agent_key: None,
            },
        }];
        let mut parsed = CliArgv {
            command: Some(crate::argv::CliCommand::Exec),
            ..CliArgv::default()
        };
        let text = render(&parsed, &events, ExecOutcome::Accepted);
        assert_eq!(text.stdout, "hello\n");
        parsed.output = Some(OutputFormat::Json);
        let json = render(&parsed, &events, ExecOutcome::Accepted);
        assert_eq!(json.exit_code, 0);
        assert_eq!(
            json.stdout,
            "{\"schemaVersion\":1,\"engine\":\"claude\",\"scope\":\"isolated\",\"result\":\"adapter-accepted\",\"text\":\"hello\"}\n"
        );
        parsed.output = Some(OutputFormat::JsonLines);
        let jsonl = render(&parsed, &events, ExecOutcome::Accepted);
        assert_eq!(jsonl.stdout.lines().count(), 2);
        assert!(
            jsonl
                .stdout
                .lines()
                .last()
                .expect("terminal")
                .contains("\"terminal\"")
        );
    }

    #[test]
    fn json_envelopes_echo_the_callers_raw_resume_prefix() {
        let mut parsed = CliArgv {
            command: Some(crate::argv::CliCommand::Exec),
            output: Some(OutputFormat::Json),
            resume: Some("abc".into()),
            ..CliArgv::default()
        };
        let json = render(&parsed, &[], ExecOutcome::Accepted);
        assert!(json.stdout.contains("\"sessionKey\":\"abc\""));
        assert!(!json.stdout.contains("codex#abcdef"));

        parsed.output = Some(OutputFormat::JsonLines);
        let jsonl = render(
            &parsed,
            &[],
            ExecOutcome::DefinitelyUnsent("not sent".into()),
        );
        assert!(
            jsonl
                .stdout
                .lines()
                .last()
                .expect("terminal")
                .contains("\"sessionKey\":\"abc\"")
        );
    }

    #[tokio::test]
    async fn missing_runtime_is_proven_unsent_and_never_claims_dispatch() {
        let parsed = CliArgv {
            command: Some(crate::argv::CliCommand::Exec),
            output: Some(OutputFormat::Json),
            ..CliArgv::default()
        };
        let result = run(&parsed, Path::new("/tmp"), "ping".into(), None).await;
        assert_eq!(result.exit_code, 3);
        assert!(result.stdout.contains("\"result\":\"definitely-unsent\""));
        assert_eq!(
            result.stderr,
            "iface: No live engine host; the prompt was not sent.\n"
        );
    }

    #[test]
    fn resume_reuses_durable_cwd_model_effort_scope_and_tool_policy() {
        let temp = tempfile::tempdir().expect("temp");
        let stored = summary(temp.path().to_string_lossy().into_owned());

        let (cwd, options) = resume_request(&stored).expect("resume request");

        assert_eq!(cwd, temp.path().to_string_lossy());
        assert_eq!(options.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(options.effort.as_deref(), Some("high"));
        assert_eq!(options.setting_sources, Some(vec!["project".into()]));
        assert_eq!(options.disallowed_tools, Some(vec!["Bash".into()]));
    }

    #[test]
    fn resume_repairs_a_removed_worktree_to_its_durable_home() {
        let temp = tempfile::tempdir().expect("temp");
        let mut stored = summary(temp.path().join("removed").to_string_lossy().into_owned());
        stored.worktree_home = Some(temp.path().to_string_lossy().into_owned());

        let (cwd, _) = resume_request(&stored).expect("fallback home");

        assert_eq!(cwd, temp.path().to_string_lossy());
    }

    #[test]
    fn timeout_budget_spans_the_whole_run_and_never_claims_unsent_after_contact() {
        let already_expired = std::time::Instant::now()
            .checked_sub(Duration::from_millis(2))
            .expect("representable instant");
        assert_eq!(
            remaining_budget(already_expired, Some(1)),
            Some(Duration::ZERO)
        );
        assert_eq!(remaining_budget(already_expired, None), None);
        assert!(
            remaining_budget(std::time::Instant::now(), Some(60_000))
                .expect("a budget")
                .as_millis()
                > 0
        );

        // Expiring after the prompt has crossed the adapter seam is reported as
        // outcome-unknown, never as a proven refusal, and the session is left
        // alive for the caller to resume.
        let outcome = wait_timed_out("claude#session-1", Some(1_500));
        assert_eq!(outcome.kind(), "outcome-unknown");
        assert!(outcome.message().contains("1500 ms"));
        assert!(outcome.message().contains("claude#session-1"));
        assert!(outcome.message().contains("may still be running"));

        let parsed = CliArgv {
            command: Some(crate::argv::CliCommand::Exec),
            output: Some(OutputFormat::Json),
            ..CliArgv::default()
        };
        let rendered = render(&parsed, &[], outcome);
        assert_eq!(rendered.exit_code, CLI_EXIT.timeout);
        assert!(rendered.stdout.contains("\"result\":\"outcome-unknown\""));
        assert!(rendered.stderr.starts_with("iface: exec stopped waiting"));
    }

    #[test]
    fn a_stream_failure_after_a_prompt_carrying_start_is_unknown_never_unsent() {
        let parsed = CliArgv {
            command: Some(crate::argv::CliCommand::Exec),
            output: Some(OutputFormat::Json),
            ..CliArgv::default()
        };
        let detached = || {
            RuntimeError::definitely_unsent(
                interface_runtime::RuntimeErrorKind::SessionNotActive,
                "session claude#session-1 has no live attachment",
                false,
            )
        };

        // A start already handed its prompt to the provider.
        for outcome in [
            stream_failure(false, Some(detached())),
            stream_failure(false, None),
        ] {
            assert_eq!(outcome.kind(), "outcome-unknown");
            assert!(
                outcome
                    .message()
                    .ends_with("Delivery unknown — Resend / Discard."),
                "{outcome:?}"
            );
            let rendered = render(&parsed, &[], outcome);
            assert_eq!(rendered.exit_code, CLI_EXIT.unknown);
            assert!(rendered.stdout.contains("\"result\":\"outcome-unknown\""));
        }
        assert!(
            stream_failure(false, Some(detached()))
                .message()
                .starts_with("session claude#session-1 has no live attachment")
        );

        // A resume takes the stream before it sends anything.
        let unsent = stream_failure(true, Some(detached()));
        assert_eq!(
            unsent,
            ExecOutcome::DefinitelyUnsent("session claude#session-1 has no live attachment".into())
        );
        assert_eq!(render(&parsed, &[], unsent).exit_code, CLI_EXIT.unsent);
        assert_eq!(stream_failure(true, None), ExecOutcome::TimedOut);
        assert_eq!(
            render(&parsed, &[], ExecOutcome::TimedOut).exit_code,
            CLI_EXIT.timeout
        );
    }

    fn status_frame(status: SessionStatus) -> RuntimeEventFrame {
        RuntimeEventFrame::Event {
            value: interface_runtime::RuntimeEvent {
                session_key: "claude#session-1".into(),
                sequence: 0,
                received_at: "2026-08-27T00:00:00.000Z".into(),
                event: EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged { status },
                },
            },
        }
    }

    #[tokio::test]
    async fn timeout_bounds_the_event_wait_and_leaves_the_contacted_session_alive() {
        let (sink, mut stream) = interface_engine::bounded_event_stream::<RuntimeEventFrame>(4);
        let (events, outcome) = wait_for_turn(
            &mut stream,
            "claude#session-1",
            std::time::Instant::now(),
            Some(20),
        )
        .await;
        assert!(events.is_empty());
        assert!(matches!(outcome, ExecOutcome::WaitTimedOut(_)));
        assert_eq!(outcome.kind(), "outcome-unknown");
        // The wait is abandoned, never the session: nothing closed the stream.
        assert!(!sink.is_closed());

        let (sink, mut stream) = interface_engine::bounded_event_stream::<RuntimeEventFrame>(4);
        for status in [SessionStatus::Working, SessionStatus::Idle] {
            sink.emit(status_frame(status)).expect("emit");
        }
        let (events, outcome) = wait_for_turn(
            &mut stream,
            "claude#session-1",
            std::time::Instant::now(),
            Some(60_000),
        )
        .await;
        assert_eq!(outcome, ExecOutcome::Accepted);
        assert_eq!(events.len(), 2);
    }
}
