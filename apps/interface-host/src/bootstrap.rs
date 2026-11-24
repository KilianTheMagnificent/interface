use std::{
    error::Error,
    ffi::{OsStr, OsString},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use chrono::Utc;
use engine_acp::{
    AcpAdapterConfig, AcpProfile as EngineAcpProfile, BuiltinAcpProvider,
    opencode_usage::OpenCodeGoUsageReader, resolve_builtin_executable, validate_profile,
};
use engine_claude::{CLAUDE_EXECUTABLE_ENV, ClaudeAdapter};
use engine_codex::{CODEX_EXECUTABLE_ENV, CodexAdapterConfig, resolve_codex_executable};
use interface_contracts::terminal::{TERMINAL_LAUNCH_REQUEST_VERSION, TerminalLaunchRequestV1};
use interface_contracts::{
    canonical_timestamp, engine::CollaborationProviderBindings, ipc::AcpProfile,
    terminal::TerminalLaunchProfileId,
};
use interface_desktop_services::{DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts};
use interface_desktop_services::{
    SavedTerminalProviderSessionPersistence, TerminalReopenAuthorityState,
};
use interface_orchestration::OrchestrationStore;
use interface_registry::SessionRegistry;
use interface_runtime::{
    EngineHistoryStore, HistoryLimits, NativeAdapterConfiguration, RuntimeServices, SessionRuntime,
};
use interface_storage::{
    PrivateDirectory, TargetSnapshot, WriterLock, atomic_write, snapshot_target,
};
use interface_terminal::{
    CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, CLAUDE_SESSION_HOOK_MODE, CODEX_SESSION_HOOK_MODE,
    TerminalLaunchCatalog, TerminalLaunchProgram,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::capture_generation::RECEIPT_ROOT_LOCK;
use crate::claude_manual_wrapper::{
    CLAUDE_MANUAL_CAPTURE_TOKEN_ENV, CLAUDE_MANUAL_EXECUTABLE_ENV, CLAUDE_MANUAL_HOST_ENV,
    CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, CLAUDE_MANUAL_RECEIPTS_ROOT_ENV, CLAUDE_MANUAL_SETTINGS_ENV,
    CLAUDE_MANUAL_USER_ZDOTDIR_ENV, CLAUDE_MANUAL_WRAPPER_MODE, CODEX_MANUAL_EXECUTABLE_ENV,
    CODEX_MANUAL_WRAPPER_MODE,
};
use crate::pane_mcp::{
    PANE_MCP_SERVER_NAME, PANE_MCP_SOCKET_ENV, PANE_TOOL_NAMES, cheap_agents_mcp_socket_path,
    pane_mcp_socket_path,
};
use crate::provider_capture::{ProviderCaptureLookup, StartupReceiptDisposition};
use crate::{antigravity_usage::AntigravityUsageReader, worktree::SystemWorktreeService};
use interface_collaboration::workers::MCP_SERVER_NAME as CHEAP_AGENTS_MCP_SERVER_NAME;

const ANTIGRAVITY_EXECUTABLE_ENV: &str = "ANTIGRAVITY_EXECUTABLE";
const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
const GROK_HOME_ENV: &str = "GROK_HOME";
const EXECUTABLE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const EXECUTABLE_LOOKUP_TAIL_BYTES: usize = 64 * 1_024;
const LOGIN_PATH_MARKER: &str = "__INTERFACE_LOGIN_PATH__";
/// How long a finished login shell's output may keep arriving before the
/// lookup stops waiting for EOF (a background job may hold stdout open).
const LOGIN_PATH_DRAIN_GRACE: Duration = Duration::from_millis(100);
const CLAUDE_TERMINAL_SESSION_RECEIPTS_DIRECTORY: &str = "claude-terminal-session-receipts";
const CLAUDE_MANUAL_ZSH_DIRECTORY: &str = "manual-claude-zsh";
const CLAUDE_MANUAL_ZSH_LOCK: &str = ".manual-claude-zsh.lock";
const CLAUDE_MANUAL_ZSH_FILE_BYTES: usize = 16 * 1024;

pub struct OpenedHostServices {
    pub data_root: PathBuf,
    pub desktop: Arc<DesktopService>,
    pub runtime: SessionRuntime,
    pub terminal_launch_catalog: TerminalLaunchCatalog,
    pub claude_projects_root: PathBuf,
    pub claude_terminal_session_receipts_root: PathBuf,
    pub antigravity_usage: Option<Arc<AntigravityUsageReader>>,
    pub opencode_usage: Option<Arc<OpenCodeGoUsageReader>>,
    pub worktrees: Arc<SystemWorktreeService>,
    pub orchestration_runtime_epoch: String,
    /// The Claude adapter's voice MCP slot; `main.rs` attaches the service.
    pub voice_mcp: Arc<crate::voice::DeferredVoiceMcp>,
    pub agent_tools: Arc<crate::agent_tools::DeferredAgentTools>,
    /// Every adapter's team tools slot (protocol 57); `main.rs` attaches the
    /// thread host's servers.
    pub team_tools: Arc<crate::team_tools::DeferredTeamTools>,
    /// Where an unreadable session cache was set aside at open, if it was.
    pub registry_quarantined: Option<PathBuf>,
    /// What Models & providers → Refresh rebuilds the catalog and the
    /// engines' executables from.
    pub provider_catalog: ProviderCatalogSource,
    /// Several accounts per provider (protocol 60); `None` when this process
    /// does not own the shared stores or they could not be opened, and then
    /// every provider runs on its Default account only.
    pub provider_accounts: Option<Arc<crate::provider_accounts::ProviderAccounts>>,
    /// Folder trust (protocol 72): the decision every Claude adapter asks
    /// before a launch or resume, and Threads reads.
    pub folder_trust: Arc<crate::folder_trust::HostFolderTrust>,
}

/// The fixed inputs of the startup launch catalog and the executable each
/// engine adapter launches through, kept so a refresh can find the provider
/// CLIs again (a CLI installed while Interface runs) without a restart.
pub struct ProviderCatalogSource {
    runtime_root: PathBuf,
    home: PathBuf,
    receipts_root: PathBuf,
    pane_socket: PathBuf,
    claude: interface_engine::ExecutablePath,
    codex: interface_engine::ExecutablePath,
    grok: interface_engine::ExecutablePath,
    qwen: interface_engine::ExecutablePath,
}

/// What one rebuild found.
pub struct RefreshedProviders {
    pub catalog: TerminalLaunchCatalog,
    /// The engines whose executable moved (`claude`, `codex`, `grok`, `qwen`).
    pub moved: Vec<&'static str>,
}

impl ProviderCatalogSource {
    /// Runs a fresh login shell for its `PATH`, resolves every provider CLI
    /// again, builds a new launch catalog and points each engine adapter at
    /// its CLI's current path. Blocking (the login shell may take up to five
    /// seconds): run it off the async workers. The caller swaps the catalog
    /// into the terminal service.
    ///
    /// # Errors
    ///
    /// The new catalog could not be built; nothing was changed.
    pub fn rebuild(&self) -> Result<RefreshedProviders, Box<dyn Error>> {
        let lookup = LoginShellExecutableLookup::new(Some(LoginShellPathProbe::start()));
        let executables = resolve_provider_executables(&self.home, &lookup);
        let catalog = terminal_launch_catalog(
            &self.runtime_root,
            &self.home,
            &lookup.shell,
            &executables.claude,
            &executables.codex,
            &executables.grok,
            &executables.qwen,
            &executables.antigravity,
            &executables.omp,
            &executables.opencode,
            &executables.muse,
            &executables.kimi,
            &executables.kimi_home,
            &self.receipts_root,
            &self.pane_socket,
        )?;
        let mut moved = Vec::new();
        for (name, slot, path) in [
            ("claude", &self.claude, executables.claude),
            ("codex", &self.codex, executables.codex),
            ("grok", &self.grok, executables.grok),
            ("qwen", &self.qwen, executables.qwen),
        ] {
            if slot.set(path) {
                moved.push(name);
            }
        }
        Ok(RefreshedProviders { catalog, moved })
    }
}

/// Opens the complete application graph against injected state and home roots.
///
/// Desktop stores intentionally open before the runtime. This makes the
/// startup ACP roster authoritative and ensures `ready` cannot precede any
/// writer lease.
///
/// # Errors
///
/// Fails closed for unsafe paths, invalid startup state, or writer contention.
pub fn open_host_services_at(
    runtime_root: impl AsRef<Path>,
    home_directory: impl AsRef<Path>,
) -> Result<OpenedHostServices, Box<dyn Error>> {
    let orchestration_runtime_epoch = Uuid::new_v4().to_string();
    open_host_services_at_with_orchestration_and_collaboration(
        runtime_root,
        home_directory,
        &orchestration_runtime_epoch,
        true,
        None,
        None,
        env!("CARGO_PKG_VERSION"),
    )
}

/// Opens the complete application graph after shared-store election and
/// atomically installs the host-owned collaboration provider bindings.
///
/// The broker must be started before this function so adapters cannot launch
/// without the exact process descriptor and session factories they will use
/// for their whole lifetime.
///
/// # Errors
///
/// Fails closed for unsafe paths, invalid startup state, writer contention,
/// or any provider binding that cannot be installed exactly.
///
/// `login_shell` is the host's early login-shell `PATH` probe; without one,
/// the shell runs on first use as before. `client_version` is what the Codex
/// and ACP adapters report in their handshakes: the binary's release-stamped
/// version, passed in so this library never reads the stamp.
#[allow(clippy::too_many_lines)] // One ordered open of every host service.
pub fn open_host_services_at_with_orchestration_and_collaboration(
    runtime_root: impl AsRef<Path>,
    home_directory: impl AsRef<Path>,
    orchestration_runtime_epoch: &str,
    owns_shared_stores: bool,
    collaboration: Option<&CollaborationProviderBindings>,
    login_shell: Option<LoginShellPathProbe>,
    client_version: &str,
) -> Result<OpenedHostServices, Box<dyn Error>> {
    let runtime_root = runtime_root.as_ref();
    let home_directory = home_directory.as_ref();
    let desktop = Arc::new(DesktopService::open(
        DesktopServiceConfig {
            appearance_file: runtime_root.join("appearance.json"),
            acp_profiles_file: runtime_root.join("acp-profiles.json"),
            terminal_workspaces_file: runtime_root.join("terminal-workspaces.json"),
            home_directory: home_directory.to_path_buf(),
            customization_user_root: Some(home_directory.join(".claude")),
            review_checkpoint_store_path: Some(runtime_root.join("review-checkpoints")),
        },
        DesktopServiceReviewPorts::default(),
    )?);
    let mut customization_roots =
        interface_desktop_services::CustomizationRoots::for_home(home_directory);
    for (name, target) in [
        ("CLAUDE_CONFIG_DIR", &mut customization_roots.claude),
        ("CODEX_HOME", &mut customization_roots.codex),
        ("GROK_HOME", &mut customization_roots.grok),
        ("QWEN_HOME", &mut customization_roots.qwen),
    ] {
        if let Some(root) = std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|root| root.is_absolute())
        {
            *target = root;
        }
    }
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        customization_roots.claude_mcp = customization_roots.claude.join(".claude.json");
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        customization_roots.opencode = config.join("opencode");
    }
    customization_roots.opencode_extra = std::env::var_os("OPENCODE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    customization_roots.opencode_file = std::env::var_os("OPENCODE_CONFIG")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    desktop.configure_customization_roots(customization_roots)?;
    let active_acp_profiles = desktop.active_acp_profiles();
    let folder_trust = crate::folder_trust::HostFolderTrust::new(
        Arc::clone(&desktop),
        home_directory,
        std::env::var_os(CLAUDE_CONFIG_DIR_ENV)
            .filter(|folder| !folder.is_empty())
            .map(PathBuf::from),
    );
    let opened = match open_runtime_at_with_epoch(
        runtime_root,
        &active_acp_profiles,
        orchestration_runtime_epoch,
        home_directory,
        owns_shared_stores,
        collaboration,
        login_shell,
        client_version,
        Some(&folder_trust),
    ) {
        Ok(opened) => opened,
        Err(error) => {
            drop(desktop);
            return Err(error);
        }
    };
    match ProviderCaptureLookup::new(
        &opened.claude_projects_root,
        &opened.claude_terminal_session_receipts_root,
    )
    .and_then(|lookup| {
        lookup.reconcile_startup_receipts(|recovered| {
            reconcile_recovered_claude_terminal_session(&desktop, recovered)
        })
    }) {
        Ok(sweep) if sweep.examined > 0 => tracing::info!(
            examined = sweep.examined,
            retired = sweep.retired,
            active = sweep.active,
            retryable = sweep.retryable,
            recovered_orphans = sweep.recovered_orphans,
            "reconciled crash-surviving Claude terminal receipts"
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!(
            %error,
            "Claude terminal receipt startup reconciliation remained retryable"
        ),
    }
    Ok(OpenedHostServices {
        data_root: runtime_root.to_path_buf(),
        desktop,
        runtime: opened.runtime,
        terminal_launch_catalog: opened.terminal_launch_catalog,
        claude_projects_root: opened.claude_projects_root,
        claude_terminal_session_receipts_root: opened.claude_terminal_session_receipts_root,
        antigravity_usage: opened.antigravity_usage,
        opencode_usage: opened.opencode_usage,
        worktrees: opened.worktrees,
        orchestration_runtime_epoch: orchestration_runtime_epoch.to_owned(),
        voice_mcp: opened.voice_mcp,
        agent_tools: opened.agent_tools,
        team_tools: opened.team_tools,
        registry_quarantined: opened.registry_quarantined,
        provider_catalog: opened.provider_catalog,
        provider_accounts: opened.provider_accounts,
        folder_trust,
    })
}

fn reconcile_recovered_claude_terminal_session(
    desktop: &DesktopService,
    recovered: &crate::provider_capture::RecoveredClaudeTerminalSession,
) -> StartupReceiptDisposition {
    let authority = match desktop.terminal_reopen_authority_state(recovered.terminal_id) {
        TerminalReopenAuthorityState::Authority(authority) => authority,
        TerminalReopenAuthorityState::Unproven
            if recovered.nested_shell && recovered.nested_replacement =>
        {
            match desktop.live_terminal_reopen_authority(recovered.terminal_id) {
                Ok(Some(authority)) => authority,
                Ok(None) | Err(_) => return StartupReceiptDisposition::Retryable,
            }
        }
        TerminalReopenAuthorityState::Unsaved => {
            return StartupReceiptDisposition::DefinitelyUnsaved;
        }
        TerminalReopenAuthorityState::PendingMigration(_)
        | TerminalReopenAuthorityState::Unproven
        | TerminalReopenAuthorityState::Unavailable => {
            return StartupReceiptDisposition::Retryable;
        }
    };
    if authority.root != recovered.root
        || !recovered_cwd_matches_authority(
            &authority.root,
            &authority.terminal.cwd,
            &recovered.cwd,
            recovered.nested_shell,
        )
    {
        return StartupReceiptDisposition::Retryable;
    }
    let shell_authority = authority.terminal.launch_session.is_none()
        && authority
            .terminal
            .launch
            .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell);
    let durable_manual_demotion = recovered.nested_shell
        && !recovered.nested_replacement
        && recovered.prior_launch_session.is_some()
        && shell_authority;
    if durable_manual_demotion {
        return match desktop
            .persist_terminal_provider_reopen_identity(&authority.root, &authority.terminal)
        {
            Ok(SavedTerminalProviderSessionPersistence::Durable) => {
                StartupReceiptDisposition::Durable
            }
            Ok(SavedTerminalProviderSessionPersistence::NotSaved) => {
                StartupReceiptDisposition::DefinitelyUnsaved
            }
            Err(_) => StartupReceiptDisposition::Retryable,
        };
    }
    let replacement_after_durable_shell =
        recovered.nested_shell && recovered.nested_replacement && shell_authority;
    if replacement_after_durable_shell {
        match desktop
            .persist_terminal_provider_reopen_identity(&authority.root, &authority.terminal)
        {
            Ok(SavedTerminalProviderSessionPersistence::Durable) => {}
            Ok(SavedTerminalProviderSessionPersistence::NotSaved) => {
                return StartupReceiptDisposition::DefinitelyUnsaved;
            }
            Err(_) => return StartupReceiptDisposition::Retryable,
        }
    }
    if !replacement_after_durable_shell
        && authority.terminal.launch_session != recovered.prior_launch_session
        && authority.terminal.launch_session.as_ref() != Some(&recovered.launch_session)
    {
        return StartupReceiptDisposition::Retryable;
    }
    let authority_root = authority.root;
    let mut recovered_terminal = authority.terminal;
    recovered_terminal.cwd.clone_from(&recovered.cwd);
    recovered_terminal.launch = Some(TerminalLaunchRequestV1 {
        version: TERMINAL_LAUNCH_REQUEST_VERSION,
        profile_id: recovered.profile_id,
    });
    recovered_terminal.launch_session = Some(recovered.launch_session.clone());
    match desktop.persist_terminal_provider_reopen_identity(&authority_root, &recovered_terminal) {
        Ok(SavedTerminalProviderSessionPersistence::Durable) => StartupReceiptDisposition::Durable,
        Ok(SavedTerminalProviderSessionPersistence::NotSaved) => {
            StartupReceiptDisposition::DefinitelyUnsaved
        }
        Err(_) => StartupReceiptDisposition::Retryable,
    }
}

fn recovered_cwd_matches_authority(
    authority_root: &str,
    saved_cwd: &str,
    recovered_cwd: &str,
    nested_shell: bool,
) -> bool {
    if !nested_shell {
        return saved_cwd == recovered_cwd;
    }
    let authority_root = Path::new(authority_root);
    let recovered_cwd = Path::new(recovered_cwd);
    let Ok(canonical_root) = std::fs::canonicalize(authority_root) else {
        return false;
    };
    let Ok(canonical_cwd) = std::fs::canonicalize(recovered_cwd) else {
        return false;
    };
    canonical_root == authority_root
        && canonical_root.is_dir()
        && canonical_cwd == recovered_cwd
        && canonical_cwd.is_dir()
        && canonical_cwd.starts_with(&canonical_root)
}

struct OpenedRuntimeCore {
    runtime: SessionRuntime,
    terminal_launch_catalog: TerminalLaunchCatalog,
    claude_projects_root: PathBuf,
    claude_terminal_session_receipts_root: PathBuf,
    antigravity_usage: Option<Arc<AntigravityUsageReader>>,
    opencode_usage: Option<Arc<OpenCodeGoUsageReader>>,
    worktrees: Arc<SystemWorktreeService>,
    voice_mcp: Arc<crate::voice::DeferredVoiceMcp>,
    agent_tools: Arc<crate::agent_tools::DeferredAgentTools>,
    team_tools: Arc<crate::team_tools::DeferredTeamTools>,
    registry_quarantined: Option<PathBuf>,
    provider_catalog: ProviderCatalogSource,
    provider_accounts: Option<Arc<crate::provider_accounts::ProviderAccounts>>,
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn open_runtime_at_with_epoch(
    runtime_root: impl AsRef<Path>,
    active_acp_profiles: &[AcpProfile],
    orchestration_runtime_epoch: &str,
    home: &Path,
    owns_shared_stores: bool,
    collaboration: Option<&CollaborationProviderBindings>,
    login_shell: Option<LoginShellPathProbe>,
    client_version: &str,
    folder_trust: Option<&Arc<crate::folder_trust::HostFolderTrust>>,
) -> Result<OpenedRuntimeCore, Box<dyn Error>> {
    let runtime_root = runtime_root.as_ref();
    let directory = PrivateDirectory::open_or_create(runtime_root)?;
    drop(directory);
    let claude_terminal_session_receipts = PrivateDirectory::open_or_create(
        runtime_root.join(CLAUDE_TERMINAL_SESSION_RECEIPTS_DIRECTORY),
    )?;
    // Hooks may start concurrently when a saved workspace restores. Create
    // the persistent root-gate inode once during single-threaded bootstrap so
    // none of those helpers race through WriterLock's first-create path.
    WriterLock::acquire(&claude_terminal_session_receipts, RECEIPT_ROOT_LOCK)?
        .release(&claude_terminal_session_receipts)?;
    let claude_terminal_session_receipts_root =
        claude_terminal_session_receipts.path().to_path_buf();
    drop(claude_terminal_session_receipts);
    let chat_home = match PrivateDirectory::open_or_create(runtime_root.join("chat")) {
        Ok(directory) => Some(directory.path().to_path_buf()),
        Err(error) => {
            tracing::warn!(%error, "chat folder unavailable; Chat is disabled");
            None
        }
    };

    let mut sessions = SessionRegistry::open(runtime_root)?;
    if owns_shared_stores {
        settle_sessions_a_crash_left_busy(&mut sessions);
    }
    let registry_quarantined = sessions.quarantined_artifact().map(Path::to_path_buf);
    let provider_accounts = if owns_shared_stores {
        open_provider_accounts(runtime_root, &sessions, registry_quarantined.is_none())
    } else {
        None
    };
    let history = EngineHistoryStore::open(runtime_root, HistoryLimits::default())?;
    let mut services = RuntimeServices::new(sessions, history);
    if owns_shared_stores {
        match OrchestrationStore::open(
            runtime_root,
            format!("host_{}", Uuid::new_v4()),
            orchestration_runtime_epoch,
            canonical_timestamp(Utc::now()),
        ) {
            Ok(orchestration) => {
                services = services.with_orchestration(orchestration);
            }
            Err(error) => tracing::warn!(
                %error,
                "shared orchestration state could not be opened; ordinary provider sessions remain available"
            ),
        }
    }
    if let (Some(trust), Some(accounts)) = (folder_trust, &provider_accounts) {
        trust.attach_accounts(Arc::clone(accounts));
    }
    let mut builder = SessionRuntime::builder(services);
    if let Some(chat_home) = chat_home {
        // Interface made the Chat folder and owns what is in it.
        if let Some(trust) = folder_trust {
            trust.add_owned_folder(&chat_home);
        }
        builder = builder.chat_home(chat_home);
    }
    let executable_lookup = LoginShellExecutableLookup::new(login_shell);
    let executables = resolve_provider_executables(home, &executable_lookup);
    let claude_projects_root = claude_projects_root(
        home,
        std::env::var_os(CLAUDE_CONFIG_DIR_ENV).map(PathBuf::from),
    );
    let terminal_launch_catalog = terminal_launch_catalog(
        runtime_root,
        home,
        &executable_lookup.shell,
        &executables.claude,
        &executables.codex,
        &executables.grok,
        &executables.qwen,
        &executables.antigravity,
        &executables.omp,
        &executables.opencode,
        &executables.muse,
        &executables.kimi,
        &executables.kimi_home,
        &claude_terminal_session_receipts_root,
        &pane_mcp_socket_path(runtime_root),
    )?;
    let antigravity_usage = executable_file(&executables.antigravity).then(|| {
        Arc::new(AntigravityUsageReader::new(
            executables.antigravity.clone(),
            runtime_root.to_path_buf(),
        ))
    });
    let opencode_usage = opencode_usage_reader(&executables.opencode, runtime_root);
    let acp = acp_adapter_configuration(
        home,
        active_acp_profiles,
        executables.grok,
        executables.qwen,
        executable_lookup.find("qwen-mlx-server"),
    )?;

    // The built-in Grok and Qwen configs come first; a refresh repoints them.
    let grok_executable = acp[0].executable.clone();
    let qwen_executable = acp[1].executable.clone();
    let agent_tools = Arc::new(crate::agent_tools::DeferredAgentTools::default());
    let team_tools = Arc::new(crate::team_tools::DeferredTeamTools::default());
    let team_factory: Arc<dyn interface_contracts::engine::TeamToolsFactory> = team_tools.clone();
    let mut codex = CodexAdapterConfig::new(executables.codex);
    codex.agent_tools = Some(agent_tools.clone());
    codex.team_tools = Some(team_factory.clone());
    let codex_executable = codex.executable_path.clone();
    // Several accounts per provider: each added account's adapter is built
    // from the same configuration as the Default one, plus its environment.
    let codex_template = codex.clone();
    let grok_template = acp[0].clone();
    builder.register_native_adapters_with_team_tools(
        NativeAdapterConfiguration {
            codex: Some(codex),
            claude_executable: None,
            acp,
        }
        .with_client_version(client_version),
        collaboration,
        Some(&team_factory),
    )?;
    let mut claude =
        ClaudeAdapter::new_with_projects_root(executables.claude, claude_projects_root.clone())?;
    if let Some(bindings) = collaboration {
        claude = claude.with_collaboration_factory(Arc::clone(&bindings.session_factory));
    }
    let voice_mcp = Arc::new(crate::voice::DeferredVoiceMcp::default());
    let voice_provider: Arc<dyn engine_claude::VoiceMcpProvider> = voice_mcp.clone();
    claude = claude
        .with_voice_mcp(voice_provider)
        .with_agent_tools(agent_tools.clone())
        .with_team_tools(team_factory);
    if let Some(trust) = folder_trust {
        claude = claude.with_folder_trust(trust.for_adapter(None));
    }
    let provider_catalog = ProviderCatalogSource {
        runtime_root: runtime_root.to_path_buf(),
        home: home.to_path_buf(),
        receipts_root: claude_terminal_session_receipts_root.clone(),
        pane_socket: pane_mcp_socket_path(runtime_root),
        claude: claude.executable_path(),
        codex: codex_executable,
        grok: grok_executable,
        qwen: qwen_executable,
    };
    let claude_template = AccountTemplates {
        claude_executable: claude.executable_path(),
        claude_voice: voice_mcp.clone(),
        agent_tools: agent_tools.clone(),
        team_tools: team_tools.clone(),
        collaboration: collaboration.cloned(),
        folder_trust: folder_trust.cloned(),
    };
    builder.register_adapter(claude)?;
    if let Some(accounts) = &provider_accounts {
        install_account_routers(
            &mut builder,
            accounts,
            &claude_template,
            codex_template,
            grok_template,
        )?;
    }
    let runtime = builder.build()?;
    Ok(OpenedRuntimeCore {
        provider_catalog,
        runtime,
        terminal_launch_catalog,
        claude_projects_root,
        claude_terminal_session_receipts_root,
        antigravity_usage,
        opencode_usage,
        worktrees: open_worktree_ledger(runtime_root),
        voice_mcp,
        agent_tools,
        team_tools,
        registry_quarantined,
        provider_accounts,
    })
}

/// Opens the account stores, and drops bindings whose session left the
/// registry when the registry read cleanly. A store that cannot open leaves
/// every provider on its Default account.
pub(crate) fn open_provider_accounts(
    runtime_root: &Path,
    sessions: &SessionRegistry,
    registry_clean: bool,
) -> Option<Arc<crate::provider_accounts::ProviderAccounts>> {
    match crate::provider_accounts::ProviderAccounts::open(runtime_root) {
        Ok(accounts) => {
            let known = sessions
                .summaries()
                .map(|summary| {
                    interface_contracts::engine::session_key(&summary.engine, &summary.session_id)
                })
                .collect::<std::collections::BTreeSet<_>>();
            if registry_clean && !known.is_empty() {
                accounts.prune_sessions(&known);
            }
            Some(accounts)
        }
        Err(error) => {
            tracing::warn!(%error, "provider accounts unavailable; every provider uses its Default account");
            None
        }
    }
}

/// What an added account's adapter is built from besides its environment.
struct AccountTemplates {
    claude_executable: interface_engine::ExecutablePath,
    claude_voice: Arc<crate::voice::DeferredVoiceMcp>,
    agent_tools: Arc<crate::agent_tools::DeferredAgentTools>,
    team_tools: Arc<crate::team_tools::DeferredTeamTools>,
    collaboration: Option<CollaborationProviderBindings>,
    folder_trust: Option<Arc<crate::folder_trust::HostFolderTrust>>,
}

/// Stands an [`AccountRoutedAdapter`] in front of the Claude, Codex and
/// Grok adapters. Each added account's adapter is the Default one's
/// configuration with the account's directory variable and store roots; the
/// Default adapter itself is unchanged.
fn install_account_routers(
    builder: &mut interface_runtime::SessionRuntimeBuilder,
    accounts: &Arc<crate::provider_accounts::ProviderAccounts>,
    templates: &AccountTemplates,
    codex: CodexAdapterConfig,
    grok: engine_acp::AcpAdapterConfig,
) -> Result<(), Box<dyn Error>> {
    use interface_contracts::provider_accounts::AccountProvider;
    use interface_runtime::accounts::{
        AccountAdapterFactory, AccountProfile, AccountRoutedAdapter,
    };
    use interface_settings::provider_accounts::account_environment;

    let team_factory: Arc<dyn interface_contracts::engine::TeamToolsFactory> =
        templates.team_tools.clone();
    let claude: AccountAdapterFactory = {
        let executable = templates.claude_executable.clone();
        let voice: Arc<dyn engine_claude::VoiceMcpProvider> = templates.claude_voice.clone();
        let agent_tools = templates.agent_tools.clone();
        let team_factory = Arc::clone(&team_factory);
        let collaboration = templates.collaboration.clone();
        let folder_trust = templates.folder_trust.clone();
        Arc::new(move |profile: &AccountProfile| {
            let mut adapter = ClaudeAdapter::new_with_projects_root(
                executable.get(),
                profile.directory.join("projects"),
            )?
            .sharing_executable(executable.clone())
            .with_environment(account_environment(
                AccountProvider::Claude,
                &profile.directory,
            ));
            if let Some(bindings) = &collaboration {
                adapter = adapter.with_collaboration_factory(Arc::clone(&bindings.session_factory));
            }
            // This account's own record of Claude Code's trust answers.
            if let Some(trust) = &folder_trust {
                adapter =
                    adapter.with_folder_trust(trust.for_adapter(Some(profile.directory.clone())));
            }
            let adapter = adapter
                .with_voice_mcp(Arc::clone(&voice))
                .with_agent_tools(agent_tools.clone())
                .with_team_tools(Arc::clone(&team_factory));
            Ok(Arc::new(adapter) as Arc<dyn interface_engine::EngineAdapter>)
        })
    };
    let codex_factory: AccountAdapterFactory = {
        let collaboration = templates.collaboration.clone();
        Arc::new(move |profile: &AccountProfile| {
            let mut config = codex.clone();
            config.environment = account_environment(AccountProvider::Codex, &profile.directory);
            let mut adapter = engine_codex::CodexAdapter::new(config)?;
            if let Some(bindings) = &collaboration {
                adapter = adapter.with_collaboration_bindings(bindings)?;
            }
            Ok(Arc::new(adapter) as Arc<dyn interface_engine::EngineAdapter>)
        })
    };
    let grok_factory: AccountAdapterFactory = {
        let collaboration = templates.collaboration.clone();
        let team_factory = Arc::clone(&team_factory);
        Arc::new(move |profile: &AccountProfile| {
            let mut config = grok.clone();
            config.environment = account_environment(AccountProvider::Grok, &profile.directory);
            config.grok_sessions_root = profile.directory.join("sessions");
            let mut adapter = engine_acp::AcpAdapter::new(config)?;
            if let Some(bindings) = &collaboration {
                adapter =
                    adapter.with_collaboration_factory(Arc::clone(&bindings.session_factory))?;
            }
            Ok(Arc::new(adapter.with_team_tools(Arc::clone(&team_factory)))
                as Arc<dyn interface_engine::EngineAdapter>)
        })
    };
    for (provider, factory) in [
        (AccountProvider::Claude, claude),
        (AccountProvider::Codex, codex_factory),
        (AccountProvider::Grok, grok_factory),
    ] {
        let engine = interface_contracts::engine::EngineId::new(provider.engine())?;
        let Some(default) = builder.adapter(&engine) else {
            continue;
        };
        let directory: Arc<dyn interface_runtime::accounts::AccountDirectory> = accounts.clone();
        let router = Arc::new(AccountRoutedAdapter::new(default, factory, directory));
        accounts.install_router(provider, Arc::clone(&router));
        builder.replace_adapter(router)?;
    }
    Ok(())
}

/// Opens the one-writer managed-worktree ledger, degrading to an in-memory
/// inventory when its metadata cannot be loaded.
fn open_worktree_ledger(runtime_root: &Path) -> Arc<SystemWorktreeService> {
    Arc::new(match SystemWorktreeService::open(runtime_root) {
        Ok(worktrees) => worktrees,
        Err(error) => {
            tracing::warn!(
                %error,
                "managed worktree metadata could not be loaded; recovered inventory remains unavailable"
            );
            SystemWorktreeService::degraded()
        }
    })
}

fn opencode_usage_reader(
    executable: &Path,
    runtime_root: &Path,
) -> Option<Arc<OpenCodeGoUsageReader>> {
    executable_file(executable).then(|| {
        Arc::new(OpenCodeGoUsageReader::new(
            executable.to_path_buf(),
            runtime_root.to_path_buf(),
        ))
    })
}

struct ProviderExecutables {
    claude: PathBuf,
    codex: PathBuf,
    grok: PathBuf,
    qwen: PathBuf,
    antigravity: PathBuf,
    omp: PathBuf,
    opencode: PathBuf,
    muse: PathBuf,
    kimi: PathBuf,
    /// Where Kimi Code keeps its plugins, beside the executable it found.
    kimi_home: PathBuf,
}

fn resolve_provider_executables(
    home: &Path,
    executable_lookup: &LoginShellExecutableLookup,
) -> ProviderExecutables {
    let codex_override = std::env::var_os(CODEX_EXECUTABLE_ENV).map(PathBuf::from);
    let codex = resolve_codex_executable(
        home,
        codex_override,
        executable_lookup.find("codex"),
        Path::is_file,
    )
    .map_or_else(|| home.join(".local/bin/codex"), |resolved| resolved.path);
    let claude = resolve_claude_executable(home, executable_lookup);
    let grok = resolve_builtin_acp_executable(BuiltinAcpProvider::Grok, home, executable_lookup);
    let qwen = resolve_builtin_acp_executable(BuiltinAcpProvider::Qwen, home, executable_lookup);
    let antigravity = resolve_antigravity_executable(
        home,
        std::env::var_os(ANTIGRAVITY_EXECUTABLE_ENV).map(PathBuf::from),
        executable_lookup,
    );

    ProviderExecutables {
        claude,
        codex,
        grok,
        qwen,
        antigravity,
        omp: resolve_omp_executable(home, executable_lookup),
        opencode: resolve_opencode_executable(home, executable_lookup),
        muse: resolve_muse_executable(home, executable_lookup),
        kimi: resolve_kimi_executable(home, executable_lookup),
        kimi_home: kimi_home(home, std::env::var_os("KIMI_CODE_HOME")),
    }
}

fn acp_adapter_configuration(
    home: &Path,
    active_profiles: &[AcpProfile],
    grok_executable: PathBuf,
    qwen_executable: PathBuf,
    qwen_mlx_server: Option<PathBuf>,
) -> Result<Vec<AcpAdapterConfig>, Box<dyn Error>> {
    let mut grok = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, grok_executable)?;
    grok.grok_sessions_root =
        grok_sessions_root(home, std::env::var_os(GROK_HOME_ENV).map(PathBuf::from));
    let mut qwen = AcpAdapterConfig::builtin(BuiltinAcpProvider::Qwen, qwen_executable)?;
    qwen.login_shell_path = qwen_mlx_server;
    let mut adapters = vec![grok, qwen];
    for profile in active_profiles {
        let validated = validate_profile(EngineAcpProfile {
            slug: profile.slug.clone(),
            display_name: profile.display_name.clone(),
            executable_path: PathBuf::from(&profile.executable_path),
            args: profile.args.clone(),
        })?;
        adapters.push(AcpAdapterConfig::custom(validated));
    }
    Ok(adapters)
}

fn claude_projects_root(home: &Path, configured: Option<PathBuf>) -> PathBuf {
    configured
        .filter(|root| !root.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects")
}

fn grok_sessions_root(home: &Path, configured: Option<PathBuf>) -> PathBuf {
    configured
        .filter(|root| !root.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".grok"))
        .join("sessions")
}

fn resolve_builtin_acp_executable(
    provider: BuiltinAcpProvider,
    home: &Path,
    executable_lookup: &LoginShellExecutableLookup,
) -> PathBuf {
    let explicit = std::env::var_os(provider.executable_environment()).map(PathBuf::from);
    let command = match provider {
        BuiltinAcpProvider::Grok => "grok",
        BuiltinAcpProvider::Qwen => "qwen",
    };
    resolve_builtin_executable(
        provider,
        home,
        explicit,
        executable_lookup.find(command),
        Path::is_file,
    )
    .map_or_else(
        || match provider {
            BuiltinAcpProvider::Grok => home.join(".grok/bin/grok"),
            BuiltinAcpProvider::Qwen => home.join(".qwen/bin/qwen"),
        },
        |resolved| resolved.path,
    )
}

/// The first absolute candidate `accept` admits: the explicit override, the
/// login-shell `PATH` hit for `name`, then the fixed install locations in
/// order; `fallback` when none is admitted.
fn first_executable(
    explicit: Option<PathBuf>,
    executable_lookup: &LoginShellExecutableLookup,
    name: &str,
    fixed: impl IntoIterator<Item = PathBuf>,
    fallback: PathBuf,
    accept: fn(&Path) -> bool,
) -> PathBuf {
    explicit
        .into_iter()
        .chain(executable_lookup.find(name))
        .chain(fixed)
        .find(|candidate| candidate.is_absolute() && accept(candidate))
        .unwrap_or(fallback)
}

fn resolve_omp_executable(home: &Path, executable_lookup: &LoginShellExecutableLookup) -> PathBuf {
    first_executable(
        None,
        executable_lookup,
        "omp",
        [
            home.join(".local/bin/omp"),
            home.join(".bun/bin/omp"),
            home.join(".npm-global/bin/omp"),
            PathBuf::from("/opt/homebrew/bin/omp"),
            PathBuf::from("/usr/local/bin/omp"),
        ],
        home.join(".local/bin/omp"),
        executable_file,
    )
}

fn resolve_opencode_executable(
    home: &Path,
    executable_lookup: &LoginShellExecutableLookup,
) -> PathBuf {
    first_executable(
        None,
        executable_lookup,
        "opencode",
        [
            home.join(".opencode/bin/opencode"),
            home.join(".local/bin/opencode"),
            home.join(".bun/bin/opencode"),
            home.join(".npm-global/bin/opencode"),
            PathBuf::from("/opt/homebrew/bin/opencode"),
            PathBuf::from("/usr/local/bin/opencode"),
        ],
        home.join(".local/bin/opencode"),
        executable_file,
    )
}

/// Meta's installer writes the `muse` launcher (which execs its native
/// `muse-bin-<version>`) into `~/.local/bin` unless `MUSE_INSTALL_DIR` says
/// otherwise; the login PATH covers a moved install.
fn resolve_muse_executable(home: &Path, executable_lookup: &LoginShellExecutableLookup) -> PathBuf {
    first_executable(
        None,
        executable_lookup,
        "muse",
        [
            home.join(".local/bin/muse"),
            PathBuf::from("/opt/homebrew/bin/muse"),
            PathBuf::from("/usr/local/bin/muse"),
        ],
        home.join(".local/bin/muse"),
        executable_file,
    )
}

/// Kimi Code's installer writes its native `kimi` into `~/.kimi-code/bin`
/// unless `KIMI_INSTALL_DIR` says otherwise; Homebrew and npm installs and a
/// moved install are found on the login PATH.
fn resolve_kimi_executable(home: &Path, executable_lookup: &LoginShellExecutableLookup) -> PathBuf {
    first_executable(
        None,
        executable_lookup,
        "kimi",
        [
            home.join(".kimi-code/bin/kimi"),
            PathBuf::from("/opt/homebrew/bin/kimi"),
            PathBuf::from("/usr/local/bin/kimi"),
        ],
        home.join(".kimi-code/bin/kimi"),
        executable_file,
    )
}

/// Kimi Code's data directory: `KIMI_CODE_HOME` when absolute, else
/// `~/.kimi-code`, as Kimi itself resolves it.
fn kimi_home(home: &Path, configured: Option<OsString>) -> PathBuf {
    configured
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .unwrap_or_else(|| home.join(".kimi-code"))
}

fn resolve_antigravity_executable(
    home: &Path,
    explicit: Option<PathBuf>,
    executable_lookup: &LoginShellExecutableLookup,
) -> PathBuf {
    // The stable `gemini` profile id is retained on the wire for saved
    // workspaces, but its curated terminal and supplemental usage source are
    // both Antigravity. The retired standalone `gemini` CLI is never selected.
    first_executable(
        explicit,
        executable_lookup,
        "agy",
        [
            home.join(".local/bin/agy"),
            home.join(".npm-global/bin/agy"),
            PathBuf::from("/opt/homebrew/bin/agy"),
            PathBuf::from("/usr/local/bin/agy"),
        ],
        home.join(".local/bin/agy"),
        executable_file,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One catalog transaction assembles explicit provider paths and their scoped integrations.
fn terminal_launch_catalog(
    runtime_root: &Path,
    home: &Path,
    login_shell: &Path,
    claude: &Path,
    codex: &Path,
    grok: &Path,
    qwen: &Path,
    antigravity: &Path,
    omp: &Path,
    opencode: &Path,
    muse: &Path,
    kimi: &Path,
    kimi_home: &Path,
    claude_terminal_session_receipts_root: &Path,
    pane_mcp_socket: &Path,
) -> Result<TerminalLaunchCatalog, Box<dyn Error>> {
    // The owner's "Pane tools without asking" switch, read again at every
    // build: at start, and when Settings changes it (a provider refresh).
    let pane_tools = PaneToolApproval::from_switch(
        interface_settings::threads::pane_tools_without_asking_at(runtime_root),
    );
    let claude_arguments = claude_terminal_capture_arguments(
        claude_terminal_session_receipts_root,
        pane_mcp_socket,
        pane_tools,
    )?;
    let codex_arguments = codex_terminal_capture_arguments(
        claude_terminal_session_receipts_root,
        Some(pane_mcp_socket),
        pane_tools,
    )?;
    // `--no-leader` keeps Grok's agent, and so the MCP servers it spawns, in
    // the pane's own process; under the shared leader daemon `nc` would not
    // descend from the pane and the pane server could not name its caller.
    let grok_arguments = vec![OsString::from("--no-leader")];
    let native_host = std::fs::canonicalize(std::env::current_exe()?)?;
    // Plugin hooks are the only Muse hook source a fresh in-process
    // session (`/clear`) reloads; `--config` hooks are run-scoped and the
    // new session drops them, so its stops would never be observed. When
    // the plugin is installed and this host has routed itself, Muse needs
    // no injected arguments at all (its own settings load normally).
    // Otherwise the `--config` merge stays as the first-session fallback.
    let muse_plugin_ready =
        match crate::native_checkpoint_setup::install_muse_plugin(runtime_root, muse) {
            Ok(true) => {
                crate::native_checkpoint_setup::write_muse_hook_host(runtime_root, &native_host)
                    .map_err(|error| {
                        tracing::warn!(%error, "Muse checkpoint plugin routing unavailable");
                        error
                    })
                    .is_ok()
            }
            Ok(false) => false,
            Err(error) => {
                tracing::warn!(%error, "Muse checkpoint plugin unavailable");
                false
            }
        };
    let muse_arguments = if muse_plugin_ready {
        Some(Vec::new())
    } else {
        crate::native_checkpoint_setup::muse_arguments(
            home,
            &native_host,
            claude_terminal_session_receipts_root,
        )
        .map_err(|error| {
            tracing::warn!(%error,"Muse checkpoint hooks unavailable");
            error
        })
        .ok()
    };
    let agy_capture = crate::native_checkpoint_setup::install_agy(runtime_root, antigravity)
        .map_err(|error| tracing::warn!(%error,"Antigravity checkpoint hooks unavailable"))
        .is_ok();
    // Kimi Code's plugin hooks inherit the pane's environment, so its
    // observer routes by this host's private variables, like Antigravity's.
    let kimi_capture =
        crate::native_checkpoint_setup::install_kimi_plugin(runtime_root, kimi_home, kimi)
            .map_err(|error| tracing::warn!(%error, "Kimi Code checkpoint plugin unavailable"))
            .is_ok();
    // A private, process-scoped TUI overlay adds the transport while OpenCode
    // continues loading its global and project settings. Preserve an explicit
    // owner-selected overlay rather than replacing it.
    let opencode_tui = if std::env::var_os("OPENCODE_TUI_CONFIG").is_none() {
        Some(install_opencode_activity_transport(runtime_root)?)
    } else {
        tracing::warn!(
            "OpenCode activity bridge unavailable: OPENCODE_TUI_CONFIG already selects an overlay"
        );
        None
    };
    let candidates = [
        (TerminalLaunchProfileId::Claude, claude, claude_arguments),
        (TerminalLaunchProfileId::Codex, codex, codex_arguments),
        (TerminalLaunchProfileId::Grok, grok, grok_arguments),
        (TerminalLaunchProfileId::Qwen, qwen, Vec::new()),
        (TerminalLaunchProfileId::Gemini, antigravity, Vec::new()),
        (TerminalLaunchProfileId::Omp, omp, Vec::new()),
        (
            TerminalLaunchProfileId::Opencode,
            opencode,
            vec![OsString::from("--auto")],
        ),
        // Muse Code keeps its own permission profile and model settings.
        (
            TerminalLaunchProfileId::Muse,
            muse,
            muse_arguments.clone().unwrap_or_default(),
        ),
        // Kimi Code starts in its own configured permission mode.
        (TerminalLaunchProfileId::Kimi, kimi, Vec::new()),
    ];
    let programs = candidates
        .into_iter()
        .filter_map(|(profile_id, executable, arguments)| {
            let mut environment = terminal_launch_environment(home, executable);
            if (profile_id == TerminalLaunchProfileId::Gemini && agy_capture)
                || (profile_id == TerminalLaunchProfileId::Kimi && kimi_capture)
            {
                environment.push((
                    interface_terminal::native_checkpoint::EXE_ENV.into(),
                    native_host.as_os_str().to_owned(),
                ));
                environment.push((
                    interface_terminal::native_checkpoint::ROOT_ENV.into(),
                    claude_terminal_session_receipts_root.as_os_str().to_owned(),
                ));
            }
            if profile_id == TerminalLaunchProfileId::Opencode
                && let Some(config) = &opencode_tui
            {
                environment.push((
                    OsString::from("INTERFACE_OPENCODE_RECEIPTS_ROOT"),
                    claude_terminal_session_receipts_root.as_os_str().to_owned(),
                ));
                environment.push((
                    OsString::from("OPENCODE_TUI_CONFIG"),
                    config.clone().into_os_string(),
                ));
            }
            let program =
                TerminalLaunchProgram::new(profile_id, executable, arguments, environment).ok()?;
            if matches!(
                profile_id,
                TerminalLaunchProfileId::Claude
                    | TerminalLaunchProfileId::Codex
                    | TerminalLaunchProfileId::Opencode
            ) || (profile_id == TerminalLaunchProfileId::Gemini && agy_capture)
                || (profile_id == TerminalLaunchProfileId::Muse && muse_arguments.is_some())
                || (profile_id == TerminalLaunchProfileId::Kimi && kimi_capture)
            {
                program.with_provider_session_capture().ok()
            } else {
                Some(program)
            }
        });
    // `codex resume <id>` exits on a deleted or archived thread, so the reopen
    // first checks the rollout still exists under Codex's sessions store.
    let codex_sessions_root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions");
    // `grok --session-id <id>` exits on an id Grok already holds, so a Grok
    // reopen first checks its sessions store to ask for `--resume` instead.
    // Same root as the ACP adapter (`GROK_HOME`, else `~/.grok/sessions`).
    let catalog = TerminalLaunchCatalog::new(programs)?
        .with_codex_sessions_root(codex_sessions_root)
        .with_grok_sessions_root(grok_sessions_root(
            home,
            std::env::var_os(GROK_HOME_ENV).map(PathBuf::from),
        ))
        .with_antigravity_root(home.join(".gemini/antigravity-cli"))
        // Every pane, shells included, so the static Grok/Antigravity entry
        // (and a manual `grok` in a shell pane) finds this host's socket, and
        // Antigravity (`agy`) reports live OSC 7501 turn status without waiting
        // for a capability query reply.
        .with_pane_environment(vec![
            (
                OsString::from(PANE_MCP_SOCKET_ENV),
                pane_mcp_socket.as_os_str().to_owned(),
            ),
            (OsString::from("AGY_CLI_OSC7501"), OsString::from("on")),
        ])?;
    if !zsh_login_shell(login_shell) {
        return Ok(catalog);
    }
    match manual_claude_shell_environment(
        runtime_root,
        home,
        claude,
        codex,
        claude_terminal_session_receipts_root,
        pane_tools,
    )
    .and_then(|environment| {
        catalog
            .clone()
            .with_shell_provider_session_capture(environment)
            .map_err(Into::into)
    }) {
        Ok(catalog) => Ok(catalog),
        Err(error) => {
            tracing::warn!(
                %error,
                "manual Claude capture is unavailable; login shells remain available"
            );
            Ok(catalog)
        }
    }
}

fn zsh_login_shell(shell: &Path) -> bool {
    shell.is_absolute()
        && std::fs::canonicalize(shell)
            .ok()
            .is_some_and(|shell| shell.is_file() && shell.file_name() == Some(OsStr::new("zsh")))
}

/// Inline, process-local settings for the one hook Interface needs in an
/// integrated Claude terminal, plus the pane MCP server. Claude merges hook
/// entries across settings levels, so this adds capture without reading or
/// replacing user settings. A nonempty `args` array selects Claude's
/// direct-exec hook form; no path or token ever passes through a shell.
///
/// `--mcp-config` is variadic in Claude's grammar, so it goes first and the
/// single-valued `--settings` closes it; the pass-off prompt the terminal
/// service appends later then lands as a positional, not as another config.
fn claude_terminal_capture_arguments(
    receipts_root: &Path,
    pane_mcp_socket: &Path,
    pane_tools: PaneToolApproval,
) -> Result<Vec<OsString>, Box<dyn Error>> {
    Ok(vec![
        OsString::from("--mcp-config"),
        claude_pane_mcp_config(pane_mcp_socket)?,
        OsString::from("--settings"),
        claude_terminal_capture_settings(receipts_root, pane_tools)?,
    ])
}

/// Which pane tools a Claude or Codex pane may call without asking the
/// owner. `read_pane` only reads, so it is always pre-approved;
/// `send_to_pane` types into another pane and `pass_off` starts one, so they
/// ask unless the owner switched "Pane tools without asking" on (Settings ›
/// Threads, protocol 71).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PaneToolApproval {
    /// The default: only `read_pane` runs without asking.
    #[default]
    ReadOnly,
    /// Every pane tool runs without asking.
    All,
}

impl PaneToolApproval {
    /// The environment variable the host gives a shell pane, so a Codex the
    /// owner starts there (the manual wrapper) follows the same switch.
    /// Absent, or any value but `1`, means [`Self::ReadOnly`].
    pub(crate) const ENV: &'static str = "INTERFACE_MANUAL_PANE_TOOLS_WITHOUT_ASKING";

    #[must_use]
    pub(crate) const fn from_switch(without_asking: bool) -> Self {
        if without_asking {
            Self::All
        } else {
            Self::ReadOnly
        }
    }

    /// The manual wrapper's reading of [`Self::ENV`].
    #[must_use]
    pub(crate) fn from_environment(value: Option<&OsStr>) -> Self {
        Self::from_switch(value == Some(OsStr::new("1")))
    }

    /// Whether `tool` (one of `PANE_TOOL_NAMES`) runs without asking.
    #[must_use]
    pub(crate) fn pre_approves(self, tool: &str) -> bool {
        tool == "read_pane" || self == Self::All
    }
}

/// `path` as UTF-8 when it is absolute and non-empty: the only form the
/// generated hook and MCP arguments embed.
fn safe_absolute_utf8<'a>(path: &'a Path, what: &str) -> Result<&'a str, Box<dyn Error>> {
    path.to_str()
        .filter(|text| path.is_absolute() && !text.is_empty())
        .ok_or_else(|| format!("the {what} path is not a safe absolute UTF-8 path").into())
}

/// The inline `--mcp-config` naming this host's pane socket through `nc`.
pub(crate) fn claude_pane_mcp_config(pane_mcp_socket: &Path) -> Result<OsString, Box<dyn Error>> {
    let socket = safe_absolute_utf8(pane_mcp_socket, "pane MCP socket")?;
    Ok(OsString::from(serde_json::to_string(&serde_json::json!({
        "mcpServers": {
            PANE_MCP_SERVER_NAME: {
                "type": "stdio",
                "command": crate::mcp_transport::NC,
                "args": ["-U", socket],
                "env": {}
            },
            CHEAP_AGENTS_MCP_SERVER_NAME: {
                "type": "stdio",
                "command": crate::mcp_transport::NC,
                "args": ["-U", cheap_agents_mcp_socket_path(pane_mcp_socket)],
                "env": {}
            }
        }
    }))?))
}

fn claude_terminal_capture_settings(
    receipts_root: &Path,
    pane_tools: PaneToolApproval,
) -> Result<OsString, Box<dyn Error>> {
    claude_terminal_capture_settings_for(
        &std::fs::canonicalize(std::env::current_exe()?)?,
        receipts_root,
        pane_tools,
    )
}

pub(crate) fn claude_terminal_capture_settings_for(
    executable: &Path,
    receipts_root: &Path,
    pane_tools: PaneToolApproval,
) -> Result<OsString, Box<dyn Error>> {
    let receipts_root = safe_absolute_utf8(receipts_root, "Claude terminal receipt")?;
    let command = executable
        .to_str()
        .ok_or("the Interface host executable path is not valid UTF-8")?;
    let command_hook = serde_json::json!({
        "type": "command",
        "command": command,
        "args": [
            CLAUDE_SESSION_HOOK_MODE,
            CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER,
            receipts_root
        ],
        "timeout": 5
    });
    // Report workers' notices reach the pane at turn boundaries.
    let notice_hook = serde_json::json!({"type":"command", "command":command,
        "args":[crate::delegation_hook::NOTICE_MODE,"claude"], "timeout":5});
    // The pane tools are pre-allowed here rather than through `--allowedTools`,
    // whose variadic form would swallow a trailing pass-off prompt. Only the
    // ones the owner's switch lets through; Claude asks for the others.
    let allowed_pane_tools = PANE_TOOL_NAMES
        .iter()
        .filter(|tool| pane_tools.pre_approves(tool))
        .map(|tool| format!("mcp__{PANE_MCP_SERVER_NAME}__{tool}"))
        .chain(
            interface_collaboration::workers::TOOL_NAMES
                .iter()
                .map(|tool| format!("mcp__{CHEAP_AGENTS_MCP_SERVER_NAME}__{tool}")),
        )
        .collect::<Vec<_>>();
    // The turn-boundary hooks reuse the same command: the helper answers them
    // with an activity title frame instead of a receipt.
    let settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [{ "hooks": [command_hook.clone(), notice_hook.clone()] }],
            "Stop": [{ "hooks": [command_hook.clone()] }],
            "PreToolUse": [{"hooks":[command_hook.clone()]}],
            "PostToolUse": [{"hooks":[notice_hook]}],
            // Answers with no decision, so Claude still shows its dialog.
            "PermissionRequest": [{ "hooks": [command_hook.clone()] }],
            "SessionStart": [
                { "matcher": "startup", "hooks": [command_hook.clone()] },
                { "matcher": "resume", "hooks": [command_hook.clone()] },
                { "matcher": "clear", "hooks": [command_hook.clone()] },
                { "matcher": "compact", "hooks": [command_hook.clone()] },
                { "matcher": "fork", "hooks": [command_hook.clone()] }
            ]
        },
        "permissions": { "allow": allowed_pane_tools }
    });
    Ok(OsString::from(serde_json::to_string(&settings)?))
}

/// Inline, process-local Codex hook that reports the thread id the pane
/// actually opened. Codex reads inline `[hooks]` tables from `-c` overrides
/// alongside the user's own hooks, and gates every new hook behind one
/// "Hooks need review" prompt keyed by a hash of the hook's definition. The
/// command is therefore identical for every pane: the capture token rides
/// `CODEX_SESSION_CAPTURE_TOKEN_ENV` instead of argv. Codex reads hook trust
/// from the session-flags layer as well, so the matching `hooks.state` entry
/// travels inline too: the prompt never shows, the user's config is not
/// edited, and the production, Dev and worktree hosts (three different
/// commands) stop overwriting one shared trust slot. Declining that prompt
/// once was enough to lose every Codex pane at the next restart.
/// `features.hooks` is switched on for the pane because Codex silently skips
/// hooks without it.
/// The process-local title override asks Codex to announce its thread name
/// over OSC: initially the first prompt, then the chat name when available.
/// Force `tui.notification_method=osc9` so turn-complete alerts are OSC 9
/// messages rather than bare BEL — Interface's `TERM_PROGRAM` is not a
/// Ghostty/iTerm/etc. name, so Codex's `auto` path would otherwise ring BEL
/// and the activity monitor would misread every finished turn as waiting.
pub(crate) fn codex_terminal_capture_arguments(
    receipts_root: &Path,
    pane_mcp_socket: Option<&Path>,
    pane_tools: PaneToolApproval,
) -> Result<Vec<OsString>, Box<dyn Error>> {
    codex_terminal_capture_arguments_for(
        &std::fs::canonicalize(std::env::current_exe()?)?,
        receipts_root,
        pane_mcp_socket,
        pane_tools,
    )
}

#[allow(clippy::too_many_lines)] // Keep generated hook definitions and their exact trust receipts together.
pub(crate) fn codex_terminal_capture_arguments_for(
    executable: &Path,
    receipts_root: &Path,
    pane_mcp_socket: Option<&Path>,
    pane_tools: PaneToolApproval,
) -> Result<Vec<OsString>, Box<dyn Error>> {
    let receipts_root = safe_absolute_utf8(receipts_root, "Codex terminal receipt")?;
    let pane_mcp_socket = pane_mcp_socket
        .map(|socket| safe_absolute_utf8(socket, "pane MCP socket"))
        .transpose()?;
    let executable = executable
        .to_str()
        .ok_or("the Interface host executable path is not valid UTF-8")?;
    let command = format!(
        "{} {CODEX_SESSION_HOOK_MODE} {}",
        shell_single_quoted(executable)?,
        shell_single_quoted(receipts_root)?
    );
    // Each entry becomes one `-c` pair, in this order: Codex hashes trust
    // positionally.
    let mut overrides = vec![
        interface_terminal::CODEX_TERMINAL_TITLE_CONFIG.to_owned(),
        interface_terminal::CODEX_FAST_MODE_KEY_CONFIG.to_owned(),
        "tui.notification_method=osc9".to_owned(),
        "features.hooks=true".to_owned(),
        format!(
            "hooks.SessionStart=[{{hooks=[{{type=\"command\",command=\"{}\"}}]}}]",
            toml_basic_string_escaped(&command)
        ),
    ];
    let mut trust = vec![format!(
        "\"{CODEX_SESSION_HOOK_STATE_KEY}\"={{trusted_hash=\"{}\"}}",
        codex_hook_trusted_hash(&command)
    )];
    // The pane MCP server, inline like the hook: no config file is written.
    if let Some(socket) = pane_mcp_socket {
        // The worker-notice hook; its exact command text is in the trust hash.
        let hook_command = format!(
            "{} {} codex",
            shell_single_quoted(executable)?,
            crate::delegation_hook::NOTICE_MODE
        );
        for (event, identity_name) in [
            ("UserPromptSubmit", "user_prompt_submit"),
            ("PostToolUse", "post_tool_use"),
        ] {
            overrides.push(format!(
                "hooks.{event}=[{{hooks=[{{type=\"command\",command=\"{}\",timeout=5}}]}}]",
                toml_basic_string_escaped(&hook_command)
            ));
            let identity = serde_json::json!({"event_name":identity_name,"hooks":[{"async":false,"command":hook_command,"timeout":5,"type":"command"}]});
            let hash = format!(
                "sha256:{:x}",
                Sha256::digest(identity.to_string().as_bytes())
            );
            trust.push(format!(
                "\"/<session-flags>/config.toml:{identity_name}:0:0\"={{trusted_hash=\"{hash}\"}}"
            ));
        }
        // Codex's per-tool `approval_mode` is `auto`, `prompt`, `writes` or
        // `approve` (Codex CLI 0.171); `prompt` makes it ask every time, so a
        // tool the switch does not let through never inherits a looser
        // server or user default.
        let pane_tools = PANE_TOOL_NAMES
            .iter()
            .map(|name| {
                let mode = if pane_tools.pre_approves(name) {
                    "approve"
                } else {
                    "prompt"
                };
                format!("{name}={{approval_mode=\"{mode}\"}}")
            })
            .collect::<Vec<_>>()
            .join(",");
        let report_tools = interface_collaboration::workers::TOOL_NAMES
            .iter()
            .map(|name| format!("{name}={{approval_mode=\"approve\"}}"))
            .collect::<Vec<_>>()
            .join(",");
        let cheap_agents_socket = cheap_agents_mcp_socket_path(Path::new(socket));
        let cheap_agents_socket = cheap_agents_socket
            .to_str()
            .ok_or("the cheap_agents MCP socket path is not valid UTF-8")?;
        overrides.extend([
            format!("mcp_servers.{PANE_MCP_SERVER_NAME}.tools={{{pane_tools}}}"),
            format!("mcp_servers.{CHEAP_AGENTS_MCP_SERVER_NAME}.tools={{{report_tools}}}"),
            format!(
                "mcp_servers.{PANE_MCP_SERVER_NAME}.command=\"{}\"",
                crate::mcp_transport::NC
            ),
            format!(
                "mcp_servers.{PANE_MCP_SERVER_NAME}.args=[\"-U\",\"{}\"]",
                toml_basic_string_escaped(socket)
            ),
            format!(
                "mcp_servers.{CHEAP_AGENTS_MCP_SERVER_NAME}.command=\"{}\"",
                crate::mcp_transport::NC
            ),
            format!(
                "mcp_servers.{CHEAP_AGENTS_MCP_SERVER_NAME}.args=[\"-U\",\"{}\"]",
                toml_basic_string_escaped(cheap_agents_socket)
            ),
        ]);
    }
    // Process-local trust names the exact reviewed command definitions only.
    overrides.push(format!("hooks.state={{{}}}", trust.join(",")));
    Ok(overrides
        .into_iter()
        .flat_map(|value| [OsString::from("-c"), OsString::from(value)])
        .collect())
}

/// Codex's persisted trust key for the first `SessionStart` hook that
/// session flags declare.
const CODEX_SESSION_HOOK_STATE_KEY: &str = "/<session-flags>/config.toml:session_start:0:0";

/// The trust hash Codex computes for a command hook: SHA-256 of the
/// canonical (key-sorted, compact) JSON of the normalized hook identity,
/// where a `SessionStart` command hook normalizes to a 600 s timeout and
/// `async = false`. Keys are written sorted so the encoding does not depend
/// on `serde_json`'s map ordering.
/// ponytail: mirrors Codex 0.153's `hook_hash`; if a later Codex changes the
/// normalization, the hook merely reads as "changed" and the one-time review
/// prompt returns, which is today's behavior.
fn codex_hook_trusted_hash(command: &str) -> String {
    let identity = serde_json::json!({
        "event_name": "session_start",
        "hooks": [{
            "async": false,
            "command": command,
            "timeout": 600,
            "type": "command",
        }],
    });
    format!(
        "sha256:{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    )
}

fn shell_single_quoted(value: &str) -> Result<String, Box<dyn Error>> {
    if value.chars().any(char::is_control) {
        return Err("a Codex hook path contains control characters".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn toml_basic_string_escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn manual_claude_shell_environment(
    runtime_root: &Path,
    home: &Path,
    claude: &Path,
    codex: &Path,
    receipts_root: &Path,
    pane_tools: PaneToolApproval,
) -> Result<Vec<(OsString, OsString)>, Box<dyn Error>> {
    let overlay = install_manual_claude_zsh_overlay(runtime_root)?;
    let host = std::fs::canonicalize(std::env::current_exe()?)?;
    let user_zdotdir = manual_claude_user_zdotdir(
        home,
        &overlay,
        std::env::var_os("ZDOTDIR").map(PathBuf::from),
    );
    let settings = claude_terminal_capture_settings(receipts_root, pane_tools)?;
    let mut environment = vec![
        (OsString::from("ZDOTDIR"), overlay.clone().into_os_string()),
        (
            OsString::from(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV),
            overlay.into_os_string(),
        ),
        (
            OsString::from(CLAUDE_MANUAL_USER_ZDOTDIR_ENV),
            user_zdotdir.into_os_string(),
        ),
        (
            OsString::from(CLAUDE_MANUAL_HOST_ENV),
            host.into_os_string(),
        ),
        (
            OsString::from(CLAUDE_MANUAL_EXECUTABLE_ENV),
            claude.as_os_str().to_owned(),
        ),
        (
            OsString::from(CODEX_MANUAL_EXECUTABLE_ENV),
            codex.as_os_str().to_owned(),
        ),
        (
            OsString::from(CLAUDE_MANUAL_CAPTURE_TOKEN_ENV),
            OsString::from(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER),
        ),
        (
            OsString::from(CLAUDE_MANUAL_RECEIPTS_ROOT_ENV),
            receipts_root.as_os_str().to_owned(),
        ),
        (OsString::from(CLAUDE_MANUAL_SETTINGS_ENV), settings),
    ];
    // Only the owner's on is exported; absent means a manual Codex asks.
    if pane_tools == PaneToolApproval::All {
        environment.push((OsString::from(PaneToolApproval::ENV), OsString::from("1")));
    }
    Ok(environment)
}

fn manual_claude_user_zdotdir(home: &Path, overlay: &Path, inherited: Option<PathBuf>) -> PathBuf {
    inherited
        .filter(|path| path.is_absolute() && !same_directory(path, overlay))
        .unwrap_or_else(|| home.to_path_buf())
}

fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

const ZSHENV: &str = r#"# Generated by Interface. Sources the user's original zsh startup file.
typeset -g _interface_manual_claude_overlay="${INTERFACE_MANUAL_CLAUDE_OVERLAY_ZDOTDIR:-}"
typeset -g _interface_manual_claude_user="${INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR:-$HOME}"
if [[ -n "$_interface_manual_claude_user" && "$_interface_manual_claude_user" != "$_interface_manual_claude_overlay" ]]; then
  export ZDOTDIR="$_interface_manual_claude_user"
  [[ -r "$_interface_manual_claude_user/.zshenv" ]] && builtin source "$_interface_manual_claude_user/.zshenv"
  if [[ -z "${ZDOTDIR:-}" ]]; then
    _interface_manual_claude_user="$HOME"
    export INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR="$HOME"
  elif [[ "$ZDOTDIR" != "$_interface_manual_claude_overlay" ]]; then
    _interface_manual_claude_user="$ZDOTDIR"
    export INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR="$ZDOTDIR"
  fi
fi
export ZDOTDIR="$_interface_manual_claude_overlay"
# If the user disabled RCS here, zsh will not read the later overlay files.
if [[ -o INTERACTIVE && ! -o RCS && ! -v functions[claude] && ! -v aliases[claude] ]]; then
  function claude {
    "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-claude-wrapper "$@"
  }
fi
if [[ ! -o RCS ]]; then
  if [[ -o INTERACTIVE && ! -v functions[codex] && ! -v aliases[codex] ]]; then
    function codex {
      if [[ -x "${INTERFACE_MANUAL_CODEX_EXECUTABLE:-}" ]]; then
        "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-codex-wrapper "$@"
      else
        command codex -c 'tui.terminal_title=["spinner","app-name","fast-mode","model","status","thread"]' -c 'tui.keymap.global.toggle_fast_mode=["shift-f12"]' -c 'tui.notification_method=osc9' "$@"
      fi
    }
  fi
  export ZDOTDIR="${_interface_manual_claude_user:-$HOME}"
fi
"#;
const ZPROFILE: &str = r#"# Generated by Interface. Sources the user's original zprofile.
export ZDOTDIR="$_interface_manual_claude_user"
[[ -r "$_interface_manual_claude_user/.zprofile" ]] && builtin source "$_interface_manual_claude_user/.zprofile"
if [[ -z "${ZDOTDIR:-}" ]]; then
  _interface_manual_claude_user="$HOME"
  export INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR="$HOME"
elif [[ "$ZDOTDIR" != "$_interface_manual_claude_overlay" ]]; then
  _interface_manual_claude_user="$ZDOTDIR"
  export INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR="$ZDOTDIR"
fi
export ZDOTDIR="$_interface_manual_claude_overlay"
# A zprofile may also be the last startup file after disabling RCS.
if [[ -o INTERACTIVE && ! -o RCS && ! -v functions[claude] && ! -v aliases[claude] ]]; then
  function claude {
    "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-claude-wrapper "$@"
  }
fi
if [[ ! -o RCS ]]; then
  if [[ -o INTERACTIVE && ! -v functions[codex] && ! -v aliases[codex] ]]; then
    function codex {
      if [[ -x "${INTERFACE_MANUAL_CODEX_EXECUTABLE:-}" ]]; then
        "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-codex-wrapper "$@"
      else
        command codex -c 'tui.terminal_title=["spinner","app-name","fast-mode","model","status","thread"]' -c 'tui.keymap.global.toggle_fast_mode=["shift-f12"]' -c 'tui.notification_method=osc9' "$@"
      fi
    }
  fi
  export ZDOTDIR="${_interface_manual_claude_user:-$HOME}"
fi
"#;
const ZSHRC: &str = r#"# Generated by Interface. Sources the user's original zshrc, then wraps bare Claude and Codex.
export ZDOTDIR="$_interface_manual_claude_user"
[[ -r "$_interface_manual_claude_user/.zshrc" ]] && builtin source "$_interface_manual_claude_user/.zshrc"
if [[ ! -v functions[claude] && ! -v aliases[claude] ]]; then
  function claude {
    "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-claude-wrapper "$@"
  }
fi
if [[ ! -v functions[codex] && ! -v aliases[codex] ]]; then
  function codex {
    if [[ -x "${INTERFACE_MANUAL_CODEX_EXECUTABLE:-}" ]]; then
      "$INTERFACE_MANUAL_CLAUDE_HOST" --terminal-manual-codex-wrapper "$@"
    else
      command codex -c 'tui.terminal_title=["spinner","app-name","fast-mode","model","status","thread"]' -c 'tui.keymap.global.toggle_fast_mode=["shift-f12"]' -c 'tui.notification_method=osc9' "$@"
    fi
  }
fi
"#;
fn install_manual_claude_zsh_overlay(runtime_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    debug_assert!(ZSHRC.contains(CLAUDE_MANUAL_WRAPPER_MODE));
    debug_assert!(ZSHRC.contains(CODEX_MANUAL_WRAPPER_MODE));

    let directory =
        PrivateDirectory::open_or_create(runtime_root.join(CLAUDE_MANUAL_ZSH_DIRECTORY))?;
    let lock = WriterLock::acquire(&directory, CLAUDE_MANUAL_ZSH_LOCK)?;
    let result = (|| {
        for (name, contents) in [
            (".zshenv", ZSHENV.as_bytes()),
            (".zprofile", ZPROFILE.as_bytes()),
            (".zshrc", ZSHRC.as_bytes()),
        ] {
            let expected = snapshot_target(&directory, name, CLAUDE_MANUAL_ZSH_FILE_BYTES)?;
            if target_holds(&expected, contents) {
                continue;
            }
            atomic_write(
                &directory,
                &lock,
                name,
                contents,
                &expected,
                CLAUDE_MANUAL_ZSH_FILE_BYTES,
            )?;
        }
        directory.assert_unchanged()?;
        Ok::<(), interface_storage::StorageError>(())
    })();
    let release = lock.release(&directory);
    result?;
    release?;
    Ok(directory.path().to_path_buf())
}

/// Whether a private file already holds exactly `contents`. `snapshot_target`
/// has checked its owner, mode and links, so an unchanged install can skip the
/// fully synced rewrite that would otherwise run on every launch.
fn target_holds(expected: &TargetSnapshot, contents: &[u8]) -> bool {
    matches!(expected, TargetSnapshot::File(snapshot)
        if snapshot.sha256()[..] == Sha256::digest(contents)[..])
}

fn install_opencode_activity_transport(runtime_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let directory = PrivateDirectory::open_or_create(runtime_root.join("opencode-activity"))?;
    let lock = WriterLock::acquire(&directory, "activity.lock")?;
    let plugin = directory.path().join("activity-v1.mjs");
    let config = serde_json::to_vec(&serde_json::json!({"plugin": [plugin]}))?;
    let result = (|| {
        for (name, bytes) in [
            (
                "activity-v1.mjs",
                interface_terminal::OPENCODE_ACTIVITY_TRANSPORT.as_bytes(),
            ),
            ("tui-v1.json", config.as_slice()),
        ] {
            let expected = snapshot_target(&directory, name, 32_768)?;
            if !target_holds(&expected, bytes) {
                atomic_write(&directory, &lock, name, bytes, &expected, 32_768)?;
            }
        }
        directory.assert_unchanged()
    })();
    let released = lock.release(&directory);
    result?;
    released?;
    Ok(directory.path().join("tui-v1.json"))
}

fn terminal_launch_environment(home: &Path, executable: &Path) -> Vec<(OsString, OsString)> {
    let prepend = executable
        .parent()
        .into_iter()
        .map(Path::to_path_buf)
        .chain([
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".grok/bin"),
            home.join(".qwen/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    interface_engine::augment_search_path(std::env::var_os("PATH").as_deref(), prepend)
        .map_or_else(|_| Vec::new(), |path| vec![(OsString::from("PATH"), path)])
}

fn resolve_claude_executable(
    home: &std::path::Path,
    executable_lookup: &LoginShellExecutableLookup,
) -> PathBuf {
    // Unlike its siblings, Claude admits a candidate without an execute bit.
    engine_claude::resolve_claude_executable(
        home,
        std::env::var_os(CLAUDE_EXECUTABLE_ENV).map(PathBuf::from),
        executable_lookup.find("claude"),
        Path::is_file,
    )
    .map_or_else(|| home.join(".local/bin/claude"), |resolved| resolved.path)
}

/// Resolves fixed provider commands through one cached login-shell `PATH`.
///
/// Finder-launched applications inherit launchd's minimal `PATH`, so this
/// bounded lookup is required before falling back to known install locations.
/// The fixed shell script receives no provider name, argument, or environment
/// value, while direct PATH traversal validates every fixed binary name.
struct LoginShellExecutableLookup {
    shell: PathBuf,
    timeout: Duration,
    login_path: OnceLock<Option<OsString>>,
    /// An early probe already running; its result fills `login_path`.
    started: Mutex<Option<thread::JoinHandle<Option<OsString>>>>,
}

impl LoginShellExecutableLookup {
    fn new(probe: Option<LoginShellPathProbe>) -> Self {
        let (shell, started) = probe.map_or_else(
            || (login_shell(), None),
            |probe| (probe.shell, probe.running),
        );
        Self {
            shell,
            timeout: EXECUTABLE_LOOKUP_TIMEOUT,
            login_path: OnceLock::new(),
            started: Mutex::new(started),
        }
    }

    fn find(&self, command: &str) -> Option<PathBuf> {
        if !safe_executable_name(command) {
            return None;
        }
        let login_path = self.login_path.get_or_init(|| {
            let started = self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            match started {
                Some(probe) => probe.join().ok().flatten(),
                None => login_shell_path_with(&self.shell, self.timeout),
            }
        });
        executable_on_path(command, login_path.as_deref())
    }
}

fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|candidate| candidate.is_absolute() && candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

/// The user's login-shell `PATH`, looked up on its own thread from the top
/// of `main`, so the shell's startup (`.zprofile`, `.zshrc`, plugin managers)
/// overlaps the journal, broker and store opens that come before the first
/// provider lookup instead of adding to them. Bounded by the same timeout.
pub struct LoginShellPathProbe {
    shell: PathBuf,
    running: Option<thread::JoinHandle<Option<OsString>>>,
}

impl LoginShellPathProbe {
    /// Starts the probe for `$SHELL` (or `/bin/zsh`). If the thread cannot be
    /// started, the lookup runs on first use as before.
    #[must_use]
    pub fn start() -> Self {
        Self::start_with(login_shell(), EXECUTABLE_LOOKUP_TIMEOUT)
    }

    fn start_with(shell: PathBuf, timeout: Duration) -> Self {
        let probed = shell.clone();
        let running = thread::Builder::new()
            .name("iface-login-path".into())
            .spawn(move || login_shell_path_with(&probed, timeout))
            .ok();
        Self { shell, running }
    }

    /// The login shell's `PATH`, waiting for the probe within its timeout.
    /// If its thread never started, the lookup runs here instead.
    #[must_use]
    pub fn finish(self) -> Option<OsString> {
        match self.running {
            Some(probe) => probe.join().ok().flatten(),
            None => login_shell_path_with(&self.shell, EXECUTABLE_LOOKUP_TIMEOUT),
        }
    }
}

/// A fixed provider command (lowercase ASCII letters, digits and `-`) found
/// executable in one of `path`'s absolute directories.
#[must_use]
pub fn provider_executable_on_path(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if !safe_executable_name(command) {
        return None;
    }
    executable_on_path(command, path)
}

/// Whether `command` names a provider command a lookup may search for.
#[must_use]
pub fn safe_executable_name(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= 64
        && command
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn executable_on_path(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    path.and_then(|path| {
        std::env::split_paths(path)
            .filter(|directory| directory.is_absolute() && !directory.as_os_str().is_empty())
            .map(|directory| directory.join(command))
            .find(|candidate| executable_file(candidate))
    })
}

/// A regular file with an execute bit, following symlinks.
#[must_use]
pub fn executable_file(candidate: &Path) -> bool {
    let Ok(metadata) = candidate.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    metadata.permissions().mode() & 0o111 != 0
}

fn login_shell_path_with(shell: &Path, timeout: Duration) -> Option<OsString> {
    if !shell.is_absolute() || timeout.is_zero() {
        return None;
    }

    let mut child = Command::new(shell)
        .args(["-lic", "printf '\n__INTERFACE_LOGIN_PATH__%s\n' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // The reader drains stdout to EOF even after the lookup returns, so a
    // background job started by the user's rc files never meets a closed
    // pipe. Chunks go through an unbounded channel; sends fail silently once
    // the lookup has stopped listening.
    let (chunks, received) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4_096];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    let _ = chunks.send(Ok(buffer[..read].to_vec()));
                }
                Err(error) => {
                    let _ = chunks.send(Err(error));
                    return;
                }
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    // Everything the shell printed is in the pipe once it exits. Read until
    // EOF, or for a short grace when a descendant keeps stdout open, all
    // inside the same deadline, then take the last complete marker line.
    let drained_by = deadline.min(Instant::now() + LOGIN_PATH_DRAIN_GRACE);
    let mut tail = Vec::new();
    loop {
        match received.recv_timeout(drained_by.saturating_duration_since(Instant::now())) {
            Ok(Ok(chunk)) => {
                tail.extend_from_slice(&chunk);
                if tail.len() > EXECUTABLE_LOOKUP_TAIL_BYTES {
                    tail.drain(..tail.len() - EXECUTABLE_LOOKUP_TAIL_BYTES);
                }
            }
            Ok(Err(_)) => return None,
            Err(_) => break,
        }
    }
    let complete = tail.iter().rposition(|byte| *byte == b'\n')?;
    let output = std::str::from_utf8(&tail[..complete]).ok()?;
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(LOGIN_PATH_MARKER))
        .filter(|path| !path.is_empty())
        .map(OsString::from)
}

/// A host that just started runs no provider turn, so a session the cache
/// still lists as working or waiting was left so by a host that stopped
/// without closing it (a crash or a kill). Without this the session reads as
/// working until its next prompt, and the thread reconcile never closes the
/// cut-off run (it closes runs only for sessions that are not busy). Only in
/// memory: opening writes no byte of the cache; the next save records it.
fn settle_sessions_a_crash_left_busy(sessions: &mut SessionRegistry) {
    use interface_contracts::engine::{EngineEvent, EngineEventKind, SessionStatus};
    let busy = sessions
        .all()
        .into_iter()
        .filter(|summary| {
            matches!(
                summary.status,
                SessionStatus::Working | SessionStatus::Waiting
            )
        })
        .map(|summary| {
            (
                interface_contracts::engine::session_key(&summary.engine, &summary.session_id),
                summary.last_activity,
            )
        })
        .collect::<Vec<_>>();
    let idle = EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    };
    for (key, last_activity) in busy {
        // Its last activity stays what it was: nothing happened now.
        if let Err(error) = sessions.apply_event(sessions.revision(), &key, &idle, last_activity) {
            tracing::warn!(%error, "a session left working by a stopped host still reads as working");
        }
    }
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
