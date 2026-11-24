use std::{
    ffi::OsString,
    io::{IsTerminal, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use engine_acp::{
    AcpAdapterConfig, AcpProfile as EngineAcpProfile, BuiltinAcpProvider,
    resolve_builtin_executable, validate_profile,
};
use engine_codex::{CODEX_EXECUTABLE_ENV, CodexAdapterConfig};
use interface_contracts::engine::CollaborationProviderBindings;
use interface_host::{
    OrchestrationHostControl, acquire_cli_instance_lock,
    bootstrap::{
        LoginShellPathProbe, executable_file, provider_executable_on_path, safe_executable_name,
    },
    prepare_one_shot_orchestration_host, prepare_orchestration_host,
};
use interface_registry::SessionRegistry;
use interface_runtime::{
    EngineHistoryStore, HistoryLimits, NativeAdapterConfiguration, RuntimeServices, SessionRuntime,
};
use interface_workspace::{default_customization_user_root, read_customization_inventory};

use crate::{
    agent, agent_client,
    agent_protocol::AGENT_MAX_PROMPT_BYTES,
    argv::{CLI_USAGE, CliArgv, CliCommand, OutputFormat},
    background::{
        BackgroundProcesses, BackgroundStore, SystemBackgroundProcesses, ensure_private_root,
        render_list,
    },
    collaboration_runtime::{
        CliCollaborationRuntime, CollaborationPreparation, PreparedCliCollaboration,
    },
    config::CliConfigStore,
    diagnostics::{DiagnosticLevel, DiagnosticsSink},
    doctor,
    errors::{CLI_EXIT, CommandResult},
    exec, orchestration,
    paths::{CliPaths, resolve_cli_paths},
    sanitize::for_display,
};

pub struct AppEnvironment {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub paths: CliPaths,
    pub user_data_root: PathBuf,
    pub stdin: String,
    pub executable: PathBuf,
    pub original_args: Vec<String>,
    pub runtime: Option<SessionRuntime>,
    pub processes: Arc<dyn BackgroundProcesses>,
    pub customization_user_root: Option<PathBuf>,
}

/// What `iface --version` and the TUI title report and the Codex and ACP
/// adapters send in their handshakes: the release-stamped product version
/// (see `interface_common::product_version!`). Set by `main.rs` from the
/// binary target so the library stays cached across releases.
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

static OVERRIDDEN_PRODUCT_VERSION: OnceLock<&'static str> = OnceLock::new();

pub fn set_product_version(version: &'static str) {
    let _ = OVERRIDDEN_PRODUCT_VERSION.set(version);
}

#[must_use]
pub fn product_version() -> &'static str {
    OVERRIDDEN_PRODUCT_VERSION
        .get()
        .copied()
        .unwrap_or(PRODUCT_VERSION)
}

impl AppEnvironment {
    /// Resolves the legacy CLI and Interface application paths without opening
    /// a provider, store, socket, or credential file.
    ///
    /// # Errors
    ///
    /// Returns an error when the process cwd, home, or executable path cannot
    /// be resolved.
    pub fn system(original_args: Vec<String>) -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not configured".to_owned())?;
        let paths = resolve_cli_paths(std::env::var_os("IFACE_DATA_DIR"), &home);
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        Ok(Self {
            cwd,
            user_data_root: interface_user_data_root(&home),
            home,
            paths,
            stdin: String::new(),
            executable,
            original_args,
            runtime: None,
            processes: Arc::new(SystemBackgroundProcesses),
            customization_user_root: None,
        })
    }
}

pub type AppResult = CommandResult;

pub async fn run(parsed: CliArgv, environment: &mut AppEnvironment) -> AppResult {
    run_with_progress(parsed, environment, None).await
}

pub async fn run_with_progress(
    parsed: CliArgv,
    environment: &mut AppEnvironment,
    progress: Option<interface_collaboration::away::Progress>,
) -> AppResult {
    if parsed.version {
        return AppResult::success(format!("{}\n", product_version()));
    }
    if parsed.help {
        return AppResult::success(format!("{CLI_USAGE}\n"));
    }
    let default_interactive_cwd;
    let requested_cwd = if parsed.command.is_none() && parsed.cwd.is_none() {
        default_interactive_cwd =
            match interface_settings::resolve_default_directory(&environment.home) {
                Ok(path) => path,
                Err(error) => return AppResult::error(error.to_string(), CLI_EXIT.generic),
            };
        &default_interactive_cwd
    } else {
        parsed.cwd.as_deref().unwrap_or(&environment.cwd)
    };
    let cwd = match canonical_directory(requested_cwd) {
        Ok(cwd) => cwd,
        Err(error) => return AppResult::error(error, CLI_EXIT.generic),
    };
    match parsed.command {
        Some(CliCommand::Workers) => crate::workers::run(&parsed.worker_args).await,
        Some(CliCommand::Theme) => crate::theme::run(&parsed.theme_args, &cwd),
        Some(CliCommand::Mcp) => {
            crate::mcp::run(&parsed.mcp_args, &environment.user_data_root).await
        }
        Some(CliCommand::Away) => {
            crate::away::run(&parsed.away_args, &environment.home, progress).await
        }
        Some(CliCommand::Doctor) => doctor_command(&parsed, &cwd, environment).await,
        Some(CliCommand::Ps | CliCommand::Logs | CliCommand::Kill) => {
            background_command(&parsed, environment)
        }
        Some(CliCommand::Exec) => exec_command(parsed, &cwd, environment).await,
        Some(CliCommand::Orchestrate) => {
            let shared = match open_one_shot_orchestration(environment).await {
                Ok(shared) => shared,
                Err(error) => return AppResult::error(error, CLI_EXIT.generic),
            };
            let result = orchestration::run(
                &parsed.orchestration_args,
                &shared.control,
                shared.live_provider_host,
                &cwd,
            )
            .await;
            finish_one_shot(result, shared.close().await)
        }
        Some(CliCommand::Trigger) => {
            trigger_command(parsed.name.as_deref().unwrap_or_default(), environment).await
        }
        Some(CliCommand::Agent) if agent::is_help_request(&parsed.agent_args) => {
            AppResult::success(format!("{}\n", agent::AGENT_USAGE))
        }
        Some(CliCommand::Agent) => {
            agent_client::run_agent_command(
                &parsed.agent_args,
                &environment.paths.agent_socket,
                &environment.stdin,
            )
            .await
        }
        None => interactive_command(&parsed, &cwd, environment).await,
    }
}

/// `iface trigger <name or id>` (T3 section 7): asks the open Interface to
/// run a thread's scheduled prompt now. Sent is success; a firing waiting
/// for a busy thread is success too (it goes when the turn ends); refused
/// is definitely unsent (exit 3) and unknown is unknown (exit 4).
async fn trigger_command(target: &str, environment: &AppEnvironment) -> AppResult {
    match interface_host::trigger_thread_schedule(&environment.user_data_root, target).await {
        Ok(answer) => trigger_result(&answer),
        Err(error) => AppResult::error(for_display(&error), CLI_EXIT.unsent),
    }
}

pub(crate) fn trigger_result(answer: &serde_json::Value) -> AppResult {
    let text = |key: &str| answer.get(key).and_then(serde_json::Value::as_str);
    let label = text("name").or_else(|| text("id")).unwrap_or("schedule");
    let detail = text("detail")
        .map(|detail| format!(" {detail}"))
        .unwrap_or_default();
    let (message, code) = match text("outcome") {
        Some("sent") => (format!("Sent \"{label}\" to its thread."), CLI_EXIT.success),
        Some("waiting") => (
            format!("\"{label}\" waits for its thread to finish its turn, then goes."),
            CLI_EXIT.success,
        ),
        Some("unknown") => (
            format!(
                "\"{label}\" may or may not have reached its thread; it is not sent again.{detail}"
            ),
            CLI_EXIT.unknown,
        ),
        _ => (
            format!("\"{label}\" was not sent.{detail}"),
            CLI_EXIT.unsent,
        ),
    };
    let message = for_display(&message);
    if code == CLI_EXIT.success {
        AppResult::success(format!("{message}\n"))
    } else {
        AppResult::error(message, code)
    }
}

struct OneShotOrchestration {
    control: OrchestrationHostControl,
    runtime: Option<SessionRuntime>,
    live_provider_host: bool,
}

impl OneShotOrchestration {
    async fn close(self) -> Result<(), String> {
        let control = self.control.close().await;
        let runtime = if let Some(runtime) = self.runtime {
            runtime.shutdown().await.map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        control.and(runtime)
    }
}

async fn open_one_shot_orchestration(
    environment: &AppEnvironment,
) -> Result<OneShotOrchestration, String> {
    let prepared = prepare_one_shot_orchestration_host(&environment.user_data_root).await?;
    let owns_stores = prepared.owns_stores();
    let runtime_epoch = prepared.runtime_epoch().to_owned();
    if owns_stores {
        let runtime = build_runtime(
            &environment.paths,
            &environment.home,
            Some((&environment.user_data_root, &runtime_epoch)),
            None,
            &CliExecutableLookup::for_runtime(),
        )?;
        let control = OrchestrationHostControl::start(prepared, &runtime).await?;
        Ok(OneShotOrchestration {
            control,
            runtime: Some(runtime),
            live_provider_host: false,
        })
    } else {
        Ok(OneShotOrchestration {
            control: OrchestrationHostControl::connect_client(prepared)?,
            runtime: None,
            live_provider_host: true,
        })
    }
}

fn finish_one_shot(mut result: CommandResult, close: Result<(), String>) -> AppResult {
    if let Err(error) = close {
        result
            .stderr
            .push_str("iface: orchestration shutdown failed: ");
        result.stderr.push_str(&for_display(&error));
        result.stderr.push('\n');
        if result.exit_code == CLI_EXIT.success {
            result.exit_code = CLI_EXIT.generic;
        }
    }
    result
}

async fn doctor_command(parsed: &CliArgv, cwd: &Path, environment: &AppEnvironment) -> AppResult {
    let output = parsed.output.unwrap_or(OutputFormat::Text);
    if parsed.doctor_report == crate::argv::DoctorReport::Providers {
        return provider_canary_command(output, &environment.home).await;
    }
    let user_root = environment
        .customization_user_root
        .clone()
        .unwrap_or_else(default_customization_user_root);
    let inventory = read_customization_inventory(cwd, Some(&user_root));
    let telemetry = doctor::read_context_telemetry(&environment.paths.logs);
    let mut snapshot = doctor::snapshot_from_inventory(cwd, inventory, telemetry);
    doctor::apply_engine_rows(&mut snapshot, doctor_sign_in_rows(&environment.home).await);
    let mut text = doctor::render(&snapshot, output);
    // The text report also checks each provider CLI; the JSON keeps its
    // legacy shape (`doctor providers --output json` has the canary's).
    if output == OutputFormat::Text && !cfg!(test) {
        let reports = provider_canary_reports(&environment.home).await;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&interface_terminal::provider_canary::render_text(&reports));
    }
    AppResult::success(text)
}

/// `iface doctor providers` (review F2, 2026-10-15): each installed provider
/// CLI against what Interface depends on, by `--version`, `--help`, listing
/// and schema output and the installed files only. Exits 1 when anything
/// Interface depends on no longer matches.
async fn provider_canary_command(output: OutputFormat, home: &Path) -> AppResult {
    let reports = provider_canary_reports(home).await;
    let drifted = reports.iter().any(|report| !report.findings.is_empty());
    let text = if output == OutputFormat::Json {
        let rows = reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "label": report.label,
                    "installed": report.installed(),
                    "executable": report.executable,
                    "version": report.version,
                    "verified": report.verified,
                    "checks": report.checks,
                    "findings": report.findings,
                    "note": report.note,
                })
            })
            .collect::<Vec<_>>();
        format!(
            "{}\n",
            serde_json::json!({"schemaVersion": 1, "providers": rows})
        )
    } else {
        interface_terminal::provider_canary::render_text(&reports)
    };
    if drifted {
        AppResult {
            exit_code: CLI_EXIT.generic,
            ..AppResult::success(text)
        }
    } else {
        AppResult::success(text)
    }
}

async fn provider_canary_reports(
    home: &Path,
) -> Vec<interface_terminal::provider_canary::CanaryReport> {
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let path = std::env::var_os("PATH");
        interface_terminal::provider_canary::run_all(&home, path.as_deref(), |name| {
            std::env::var_os(name)
        })
    })
    .await
    .unwrap_or_default()
}

/// Whether Claude and Codex are installed and signed in, by each CLI's own
/// status command (`claude auth status`, `codex login status`); never by
/// reading their credentials. Tests leave the rows unprobed.
async fn doctor_sign_in_rows(home: &Path) -> Vec<doctor::DoctorEngineRow> {
    if cfg!(test) {
        return Vec::new();
    }
    let lookup = CliExecutableLookup::for_runtime();
    let claude = resolve_claude_executable(home, claude_override(), &lookup);
    let codex = resolve_codex_executable(home, codex_override(), &lookup);
    let (claude_auth, codex_auth) = tokio::join!(
        async {
            match &claude {
                Some(path) => {
                    engine_claude::auth::auth_status(path, engine_claude::auth::AUTH_STATUS_TIMEOUT)
                        .await
                        .map(|status| status.auth)
                }
                None => None,
            }
        },
        async {
            match &codex {
                Some(path) => {
                    engine_codex::auth::auth_status(path, engine_codex::auth::AUTH_STATUS_TIMEOUT)
                        .await
                }
                None => None,
            }
        },
    );
    ["claude", "codex"]
        .into_iter()
        .zip([
            (claude.is_some(), claude_auth),
            (codex.is_some(), codex_auth),
        ])
        .map(|(engine, (installed, auth))| {
            doctor::engine_auth_row(
                engine,
                installed,
                auth,
                doctor::sign_in_command(engine).unwrap_or_default(),
            )
        })
        .collect()
}

#[allow(clippy::too_many_lines)] // Startup order is the CLI ownership transaction.
async fn interactive_command(
    parsed: &CliArgv,
    cwd: &Path,
    environment: &mut AppEnvironment,
) -> AppResult {
    let diagnostics = match DiagnosticsSink::install(&environment.paths.logs) {
        Ok(diagnostics) => diagnostics,
        Err(error) => return AppResult::error(error, CLI_EXIT.generic),
    };
    let mut signals = match crate::tui::ShutdownSignals::new() {
        Ok(signals) => signals,
        Err(error) => {
            diagnostics.record(DiagnosticLevel::Error, &error);
            diagnostics.restore().await;
            return AppResult::error(error, CLI_EXIT.generic);
        }
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return AppResult::error(
            "an interactive terminal is required (try --help or --version).",
            CLI_EXIT.generic,
        );
    }
    let executable_lookup = environment
        .runtime
        .is_none()
        .then(CliExecutableLookup::started_early);
    if let Err(error) = ensure_private_root(&environment.paths.data_dir) {
        return AppResult::error(error, CLI_EXIT.generic);
    }
    if let Some(code) = signals.pending() {
        return AppResult::exit(code);
    }
    let instance_lock = match acquire_cli_instance_lock(&environment.paths.instance_lock) {
        Ok(lock) => lock,
        Err(error) => return AppResult::error(error, CLI_EXIT.generic),
    };
    if let Some(code) = signals.pending() {
        return AppResult::exit(code);
    }
    if environment.runtime.is_none() {
        if let Err(error) = ensure_interactive_control_root(&environment.user_data_root) {
            return AppResult::error(error, CLI_EXIT.generic);
        }
    }
    let prepared_orchestration = match prepare_orchestration_host(&environment.user_data_root).await
    {
        Ok(prepared) => prepared,
        Err(error) => return AppResult::error(error, CLI_EXIT.generic),
    };
    let mut collaboration_diagnostic = None;
    let mut prepared_collaboration: Option<PreparedCliCollaboration> = None;
    if environment.runtime.is_none() {
        let collaboration_enabled = match CliConfigStore::new(environment.paths.config.clone()) {
            Ok(mut store) => store.load().collaboration,
            Err(error) => return AppResult::error(error, CLI_EXIT.generic),
        };
        if collaboration_enabled {
            match crate::collaboration_runtime::prepare(&environment.paths.collaboration).await {
                Ok(CollaborationPreparation::Ready(prepared)) => {
                    prepared_collaboration = Some(*prepared);
                }
                Ok(CollaborationPreparation::SafeUnavailable { diagnostic }) => {
                    diagnostics.record(
                        DiagnosticLevel::Warn,
                        format!(
                            "collaboration is unavailable; starting providers without cross-model tools: {diagnostic}"
                        ),
                    );
                    collaboration_diagnostic = Some(diagnostic);
                }
                Err(error) => return AppResult::error(error, CLI_EXIT.generic),
            }
        }
        let control_root = prepared_orchestration.owns_stores().then_some((
            environment.user_data_root.as_path(),
            prepared_orchestration.runtime_epoch(),
        ));
        match build_runtime(
            &environment.paths,
            &environment.home,
            control_root,
            prepared_collaboration
                .as_ref()
                .map(PreparedCliCollaboration::provider_bindings),
            &executable_lookup.unwrap_or_else(CliExecutableLookup::for_runtime),
        ) {
            Ok(runtime) => environment.runtime = Some(runtime),
            Err(error) => {
                if let Some(prepared) = prepared_collaboration.take() {
                    prepared.rollback().await;
                }
                return AppResult::error(error, CLI_EXIT.generic);
            }
        }
    }
    let mut collaboration: Option<CliCollaborationRuntime> = None;
    if let Some(prepared) = prepared_collaboration {
        let Some(runtime) = environment.runtime.as_ref() else {
            return AppResult::error(
                "The interactive runtime was not initialized.",
                CLI_EXIT.generic,
            );
        };
        match prepared.attach(Arc::new(runtime.clone())).await {
            Ok(active) => collaboration = Some(active),
            Err(error) => {
                let _ = runtime.shutdown().await;
                drop(environment.runtime.take());
                return AppResult::error(error, CLI_EXIT.generic);
            }
        }
    }
    if let Some(code) = signals.pending() {
        shutdown_interactive_runtime(environment.runtime.as_ref(), collaboration.as_ref()).await;
        drop(environment.runtime.take());
        return AppResult::exit(code);
    }
    let Some(runtime) = environment.runtime.take() else {
        return AppResult::error(
            "The interactive runtime was not initialized.",
            CLI_EXIT.generic,
        );
    };
    let cleanup_runtime = runtime.clone();
    let cleanup_collaboration = collaboration.clone();
    let run_result = crate::tui::run(
        runtime,
        collaboration,
        collaboration_diagnostic,
        diagnostics.clone(),
        prepared_orchestration,
        &environment.paths,
        parsed,
        cwd,
        signals,
    )
    .await;
    shutdown_interactive_runtime(Some(&cleanup_runtime), cleanup_collaboration.as_ref()).await;
    let release_result = instance_lock.release();
    if let Err(error) = &run_result {
        diagnostics.record(DiagnosticLevel::Error, error);
    }
    diagnostics.restore().await;
    match (run_result, release_result) {
        (Ok(exit_code), Ok(())) => AppResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
        },
        (Err(error), _) | (Ok(_), Err(error)) => AppResult::error(error, CLI_EXIT.generic),
    }
}

async fn shutdown_interactive_runtime(
    runtime: Option<&SessionRuntime>,
    collaboration: Option<&CliCollaborationRuntime>,
) {
    if let Some(collaboration) = collaboration {
        let _ = collaboration.quiesce_and_close().await;
    }
    if let Some(runtime) = runtime {
        let _ = runtime.shutdown().await;
    }
    if let Some(collaboration) = collaboration {
        let _ = collaboration.drain().await;
    }
}

fn ensure_interactive_control_root(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    ensure_private_root(path)
}

fn background_command(parsed: &CliArgv, environment: &AppEnvironment) -> AppResult {
    let store = match BackgroundStore::open(&environment.paths.data_dir, "logs", "bg-sessions") {
        Ok(store) => store,
        Err(error) => return AppResult::error(error, CLI_EXIT.generic),
    };
    match parsed.command {
        Some(CliCommand::Ps) => match store.list(environment.processes.as_ref()) {
            Ok(sessions) => AppResult::success(render_list(
                &sessions,
                parsed.output == Some(OutputFormat::Json),
            )),
            Err(error) => AppResult::error(error, CLI_EXIT.generic),
        },
        Some(CliCommand::Logs) => match store.read_log(parsed.name.as_deref().unwrap_or("")) {
            Ok(log) => AppResult::success(if log.ends_with('\n') {
                log
            } else {
                format!("{log}\n")
            }),
            Err(error) => AppResult::error(error, CLI_EXIT.generic),
        },
        Some(CliCommand::Kill) => {
            match store.kill(
                parsed.name.as_deref().unwrap_or(""),
                environment.processes.as_ref(),
            ) {
                Ok(session) => AppResult::success(format!(
                    "{{\"schemaVersion\":1,\"session\":{}}}\n",
                    serde_json::to_string(&session).expect("background session serializes")
                )),
                Err(error) => AppResult::error(error, CLI_EXIT.generic),
            }
        }
        _ => AppResult::error("unknown background command.", CLI_EXIT.validation),
    }
}

async fn exec_command(parsed: CliArgv, cwd: &Path, environment: &mut AppEnvironment) -> AppResult {
    let prompt = if parsed.prompt.as_deref() == Some("-") {
        environment.stdin.clone()
    } else {
        parsed.prompt.clone().unwrap_or_default()
    };
    if prompt.trim().is_empty() {
        return AppResult::error("exec requires a prompt.", CLI_EXIT.validation);
    }
    if let Some(name) = parsed.background_name.as_deref() {
        let store = match BackgroundStore::open(&environment.paths.data_dir, "logs", "bg-sessions")
        {
            Ok(store) => store,
            Err(error) => return AppResult::error(error, CLI_EXIT.generic),
        };
        let child_args = without_background_flag(&environment.original_args, name);
        let command = std::iter::once(environment.executable.to_string_lossy().into_owned())
            .chain(child_args)
            .collect::<Vec<_>>();
        return match store.start(name, command, environment.processes.as_ref()) {
            Ok(session) => AppResult::success(format!(
                "{{\"schemaVersion\":1,\"session\":{}}}\n",
                serde_json::to_string(&session).expect("background session serializes")
            )),
            Err(error) => AppResult::error(error, CLI_EXIT.generic),
        };
    }
    // An injected runtime is the caller-owned test seam. Production exec owns
    // the same single-writer lease as the TUI because it mutates the shared
    // registry and durable transcript stores.
    let owns_runtime = environment.runtime.is_none();
    let executable_lookup = owns_runtime.then(CliExecutableLookup::started_early);
    let instance_lock = if owns_runtime {
        if let Err(error) = ensure_private_root(&environment.paths.data_dir) {
            return AppResult::error(error, CLI_EXIT.generic);
        }
        match acquire_cli_instance_lock(&environment.paths.instance_lock) {
            Ok(lock) => Some(lock),
            Err(error) => return AppResult::error(error, CLI_EXIT.generic),
        }
    } else {
        None
    };
    if let Some(executable_lookup) = &executable_lookup {
        match build_runtime(
            &environment.paths,
            &environment.home,
            None,
            None,
            executable_lookup,
        ) {
            Ok(runtime) => environment.runtime = Some(runtime),
            Err(error) => {
                if let Some(lock) = instance_lock {
                    let _ = lock.release();
                }
                return AppResult::error(error, CLI_EXIT.unsent);
            }
        }
    }
    let result = exec::run(&parsed, cwd, prompt, environment.runtime.as_ref()).await;
    let shutdown_error = if owns_runtime {
        match environment.runtime.take() {
            Some(runtime) => runtime
                .shutdown()
                .await
                .err()
                .map(|error| error.to_string()),
            None => None,
        }
    } else {
        None
    };
    let release_error = instance_lock.and_then(|lock| lock.release().err());
    with_teardown_warnings(
        result,
        [
            shutdown_error.map(|error| ("runtime shutdown", error)),
            release_error.map(|error| ("instance lock release", error)),
        ],
    )
}

/// An exec's stdout and exit code say what became of its prompt, so a
/// teardown failure after the turn is appended to stderr as a warning and
/// replaces neither: a bare generic 1 would hide a finished or
/// outcome-unknown turn and invite a resend that repeats it.
fn with_teardown_warnings(
    mut result: AppResult,
    failures: impl IntoIterator<Item = Option<(&'static str, String)>>,
) -> AppResult {
    for (step, error) in failures.into_iter().flatten() {
        result.stderr.push_str("iface: warning: ");
        result.stderr.push_str(step);
        result.stderr.push_str(" failed after exec: ");
        result.stderr.push_str(&for_display(&error));
        result.stderr.push('\n');
    }
    result
}

pub(crate) fn build_runtime(
    paths: &CliPaths,
    home: &Path,
    control_root: Option<(&Path, &str)>,
    collaboration: Option<&CollaborationProviderBindings>,
    executable_lookup: &CliExecutableLookup,
) -> Result<SessionRuntime, String> {
    ensure_private_root(&paths.data_dir)?;
    let sessions = SessionRegistry::open(&paths.data_dir).map_err(|error| error.to_string())?;
    // Refuses a data root that is not owner-private before any store opens.
    interface_storage::PrivateDirectory::open(&paths.data_dir)
        .map_err(|error| error.to_string())?;
    // Frozen 0.1.169 owns `history` as an append-only prompt-history file. The
    // engine history is a process-local projection that writes nothing; a
    // durable journal would need a name of its own, never `history`.
    let history = EngineHistoryStore::open(&paths.data_dir, HistoryLimits::default())
        .map_err(|error| error.to_string())?;
    let mut services = RuntimeServices::new(sessions, history);
    if let Some((control_root, runtime_epoch)) = control_root {
        let orchestration = interface_orchestration::OrchestrationStore::open(
            control_root,
            format!("iface-cli-{}", uuid::Uuid::new_v4()),
            runtime_epoch,
            interface_contracts::canonical_timestamp(chrono::Utc::now()),
        )
        .map_err(|error| error.to_string())?;
        services = services.with_orchestration(orchestration);
    }
    let mut builder = SessionRuntime::builder(services);
    let codex = resolve_codex_executable(home, codex_override(), executable_lookup)
        .unwrap_or_else(|| home.join(".local/bin/codex"));
    let claude_executable = resolve_claude_executable(home, claude_override(), executable_lookup)
        .unwrap_or_else(|| home.join(".local/bin/claude"));
    let active_acp_profiles = load_acp_profiles(&paths.acp_profiles);
    let acp = acp_adapter_configuration(home, &active_acp_profiles, executable_lookup)?;
    builder
        .register_native_adapters_with_collaboration(
            NativeAdapterConfiguration {
                codex: Some(CodexAdapterConfig::new(codex)),
                claude_executable: Some(claude_executable),
                acp,
            }
            .with_client_version(product_version()),
            collaboration,
        )
        .map_err(|error| error.to_string())?;
    builder.build().map_err(|error| error.to_string())
}

fn load_acp_profiles(path: &Path) -> Vec<interface_contracts::ipc::AcpProfile> {
    let Ok(mut store) = interface_settings::AcpProfileStore::open(path) else {
        return Vec::new();
    };
    store.load().unwrap_or_default()
}

fn acp_adapter_configuration(
    home: &Path,
    active_profiles: &[interface_contracts::ipc::AcpProfile],
    executable_lookup: &CliExecutableLookup,
) -> Result<Vec<AcpAdapterConfig>, String> {
    let grok = AcpAdapterConfig::builtin(
        BuiltinAcpProvider::Grok,
        resolve_builtin_acp_executable(BuiltinAcpProvider::Grok, home, executable_lookup),
    )
    .map_err(|error| error.to_string())?;
    let mut qwen = AcpAdapterConfig::builtin(
        BuiltinAcpProvider::Qwen,
        resolve_builtin_acp_executable(BuiltinAcpProvider::Qwen, home, executable_lookup),
    )
    .map_err(|error| error.to_string())?;
    // An optional helper: never spawn a login shell just to look for it.
    // Qwen falls back to its known install locations at launch.
    qwen.login_shell_path = executable_lookup.find_inherited("qwen-mlx-server");
    let mut adapters = vec![grok, qwen];
    for profile in active_profiles {
        let validated = validate_profile(EngineAcpProfile {
            slug: profile.slug.clone(),
            display_name: profile.display_name.clone(),
            executable_path: PathBuf::from(&profile.executable_path),
            args: profile.args.clone(),
        })
        .map_err(|error| error.to_string())?;
        adapters.push(AcpAdapterConfig::custom(validated));
    }
    Ok(adapters)
}

fn resolve_builtin_acp_executable(
    provider: BuiltinAcpProvider,
    home: &Path,
    executable_lookup: &CliExecutableLookup,
) -> PathBuf {
    let explicit = std::env::var_os(provider.executable_environment()).map(PathBuf::from);
    resolve_builtin_acp_executable_with(provider, home, explicit, executable_lookup)
}

fn resolve_builtin_acp_executable_with(
    provider: BuiltinAcpProvider,
    home: &Path,
    explicit: Option<PathBuf>,
    executable_lookup: &CliExecutableLookup,
) -> PathBuf {
    let binary = match provider {
        BuiltinAcpProvider::Grok => "grok",
        BuiltinAcpProvider::Qwen => "qwen",
    };
    let found = lookup_unless_overridden(explicit.as_deref(), binary, executable_lookup);
    resolve_builtin_executable(provider, home, explicit, found, executable_file).map_or_else(
        || match provider {
            BuiltinAcpProvider::Grok => home.join(".grok/bin/grok"),
            BuiltinAcpProvider::Qwen => home.join(".qwen/bin/qwen"),
        },
        |resolved| resolved.path,
    )
}

// Codex and Claude resolve through their engine crates, as the desktop host
// does, and keep the path found rather than its target: each CLI update
// repoints a link such as `~/.local/bin/claude` and prunes the version it
// pointed at, so a canonical path goes stale while the TUI stays open.
fn resolve_codex_executable(
    home: &Path,
    explicit: Option<PathBuf>,
    executable_lookup: &CliExecutableLookup,
) -> Option<PathBuf> {
    let found = lookup_unless_overridden(explicit.as_deref(), "codex", executable_lookup);
    engine_codex::resolve_codex_executable(home, explicit, found, executable_file)
        .map(|resolved| resolved.path)
}

fn resolve_claude_executable(
    home: &Path,
    explicit: Option<PathBuf>,
    executable_lookup: &CliExecutableLookup,
) -> Option<PathBuf> {
    let found = lookup_unless_overridden(explicit.as_deref(), "claude", executable_lookup);
    // As in the desktop host, Claude admits a candidate without an execute bit.
    engine_claude::resolve_claude_executable(home, explicit, found, Path::is_file)
        .map(|resolved| resolved.path)
}

fn codex_override() -> Option<PathBuf> {
    std::env::var_os(CODEX_EXECUTABLE_ENV).map(PathBuf::from)
}

fn claude_override() -> Option<PathBuf> {
    std::env::var_os(engine_claude::CLAUDE_EXECUTABLE_ENV).map(PathBuf::from)
}

/// A usable override wins before any `PATH` candidate, so the lookup (and
/// with it the login shell) is skipped.
fn lookup_unless_overridden(
    explicit: Option<&Path>,
    binary: &str,
    executable_lookup: &CliExecutableLookup,
) -> Option<PathBuf> {
    if explicit.is_some_and(usable_override) {
        None
    } else {
        executable_lookup.find(binary)
    }
}

/// Provider commands `build_runtime` looks up on `PATH` unless their
/// override names a usable executable.
const LOOKED_UP_PROVIDERS: [(&str, &str); 4] = [
    (engine_claude::CLAUDE_EXECUTABLE_ENV, "claude"),
    (CODEX_EXECUTABLE_ENV, "codex"),
    (BuiltinAcpProvider::Grok.executable_environment(), "grok"),
    (BuiltinAcpProvider::Qwen.executable_environment(), "qwen"),
];

fn usable_override(path: &Path) -> bool {
    path.is_absolute() && executable_file(path)
}

/// Finds provider commands on the inherited `PATH`, then on the login
/// shell's, through the desktop host's bounded lookup.
pub(crate) struct CliExecutableLookup {
    inherited_path: Option<OsString>,
    login_path: OnceLock<Option<OsString>>,
    /// A probe already running; the first miss joins it.
    probe: Mutex<Option<LoginShellPathProbe>>,
}

impl CliExecutableLookup {
    /// Runs the login-shell lookup only on the first miss.
    #[cfg(not(test))]
    pub(crate) fn for_runtime() -> Self {
        Self {
            inherited_path: std::env::var_os("PATH"),
            login_path: OnceLock::new(),
            probe: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_runtime() -> Self {
        let login_path = OnceLock::new();
        login_path
            .set(None)
            .expect("test executable lookup must be uninitialized");
        Self {
            inherited_path: None,
            login_path,
            probe: Mutex::new(None),
        }
    }

    /// Starts the login-shell probe now when `build_runtime` will need it —
    /// a provider neither overridden nor on the inherited `PATH` — so the
    /// shell's startup overlaps the instance lock and store opens instead of
    /// following them.
    pub(crate) fn started_early() -> Self {
        let lookup = Self::for_runtime();
        if lookup.login_path.get().is_none()
            && lookup.login_shell_needed(|name| {
                std::env::var_os(name).is_some_and(|path| usable_override(Path::new(&path)))
            })
        {
            *lookup.probe.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(LoginShellPathProbe::start());
        }
        lookup
    }

    /// Whether `build_runtime` will consult the login shell: some provider
    /// is neither `overridden` nor on the inherited `PATH`.
    fn login_shell_needed(&self, overridden: impl Fn(&str) -> bool) -> bool {
        LOOKED_UP_PROVIDERS.iter().any(|(override_name, binary)| {
            !overridden(override_name) && self.find_inherited(binary).is_none()
        })
    }

    fn find_inherited(&self, binary: &str) -> Option<PathBuf> {
        provider_executable_on_path(binary, self.inherited_path.as_deref())
    }

    fn find(&self, binary: &str) -> Option<PathBuf> {
        if !safe_executable_name(binary) {
            return None;
        }
        self.find_inherited(binary).or_else(|| {
            let login_path = self.login_path.get_or_init(|| {
                let probe = self
                    .probe
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                probe.unwrap_or_else(LoginShellPathProbe::start).finish()
            });
            provider_executable_on_path(binary, login_path.as_deref())
        })
    }
}

fn without_background_flag<'a>(
    args: &'a [String],
    name: &'a str,
) -> impl Iterator<Item = String> + 'a {
    let mut skip_name = false;
    args.iter().filter_map(move |argument| {
        if skip_name {
            skip_name = false;
            if argument == name {
                return None;
            }
        }
        if argument == "--bg" {
            skip_name = true;
            return None;
        }
        Some(argument.clone())
    })
}

pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let metadata = std::fs::metadata(&absolute).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err(format!("Not a directory: {}", absolute.display()));
    }
    absolute.canonicalize().map_err(|error| error.to_string())
}

#[must_use]
pub fn interface_user_data_root(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library")
            .join("Application Support")
            .join("Interface")
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA").map_or_else(
            || home.join("AppData").join("Roaming").join("Interface"),
            |root| PathBuf::from(root).join("Interface"),
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(
            || home.join(".config").join("Interface"),
            |root| PathBuf::from(root).join("Interface"),
        )
    }
}

/// Reads stdin only for the exact command grammars that consume it.
///
/// # Errors
///
/// Returns a fully rendered command error when stdin cannot be read under the
/// frozen command-specific byte and UTF-8 rules.
pub fn read_stdin_if_needed(parsed: &CliArgv) -> Result<String, AppResult> {
    let mut stdin = std::io::stdin().lock();
    read_stdin_from(parsed, &mut stdin)
}

fn read_stdin_from(parsed: &CliArgv, reader: &mut impl Read) -> Result<String, AppResult> {
    if parsed.prompt.as_deref() != Some("-")
        && !(parsed.command == Some(CliCommand::Agent)
            && agent_client::agent_command_uses_stdin(&parsed.agent_args))
    {
        return Ok(String::new());
    }
    if parsed.command == Some(CliCommand::Agent) {
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(u64::try_from(AGENT_MAX_PROMPT_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(|error| AppResult::error(error.to_string(), CLI_EXIT.generic))?;
        if bytes.len() > AGENT_MAX_PROMPT_BYTES {
            return Err(agent_stdin_error(
                &parsed.agent_args,
                agent_client::AgentStdinReadError::TooLarge,
            ));
        }
        return String::from_utf8(bytes).map_err(|_| {
            agent_stdin_error(
                &parsed.agent_args,
                agent_client::AgentStdinReadError::InvalidUtf8,
            )
        });
    }

    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| AppResult::error(error.to_string(), CLI_EXIT.generic))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn agent_stdin_error(args: &[String], error: agent_client::AgentStdinReadError) -> AppResult {
    agent_client::render_stdin_read_error(args, error)
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
