use std::{convert::Infallible, error::Error, ffi::OsString, path::Path, sync::Arc};

mod shutdown;

use directories::BaseDirs;
use interface_collaboration::{
    CollaborationBrokerRuntime, CollaborationBrokerRuntimeError, CollaborationBrokerRuntimeOptions,
    CollaborationCoordinator, CollaborationJournal, CollaborationModelIngress,
};
use interface_common::uuid::canonical_uuid_v4;
use interface_contracts::terminal::TerminalLaunchProfileId;
use interface_host::{
    AutomaticWorktreeReleaseService, CollaborationHostDispatcher, CollaborationSessionHooks,
    DeferredCollaborationModelIngress, DesktopCollaborationProjection, DesktopHostDispatcher,
    DurableCollaborationService, HostDispatcher, HostServer, HostServerConfig,
    ManagedWorktreeInventory, NativeHostDispatcher, NativeRuntimeView, OpenedHostServices,
    OperationOwnerRouter, OrchestrationHostControl, PaneMcpServer, PromptContextHostDispatcher,
    RuntimeHostDispatcher, SessionManagementHostDispatcher, SessionRuntimeCollaborationPort,
    SharedRegistryPushes, SupplementalUsageReader, TerminalHostDispatcher, UpdaterHostDispatcher,
    VoiceDeps, VoiceEventTap, VoiceService, WorktreeService,
    bootstrap::open_host_services_at_with_orchestration_and_collaboration,
    collaboration::CollaborationDesktopControl,
    manual_claude_wrapper_requested, manual_codex_wrapper_requested, prepare_orchestration_host,
    run_claude_terminal_session_hook, run_codex_terminal_session_hook, run_manual_claude_wrapper,
    run_manual_codex_wrapper, spawn_update_state_forwarder,
    terminal_transport::{TerminalDataPlane, TerminalDataTransportConfig},
};
use interface_storage::PrivateDirectory;
use interface_terminal::{
    CLAUDE_SESSION_HOOK_MODE, CODEX_SESSION_CAPTURE_TOKEN_ENV, CODEX_SESSION_HOOK_MODE,
    SystemExternalTerminalBackend,
};
use interface_updater::{ReleaseChannel, SystemUpdaterClock, SystemUpdaterVerifier, UreqFetcher};

/// What this host reports in its ready frame and hands the Codex and ACP
/// adapters for their handshakes: the release-stamped product version (see
/// `interface_common::product_version!`). Read in the binary target only:
/// the library, which iface also links, then stays cached across releases.
const PRODUCT_VERSION: &str = interface_common::product_version!();

fn main() {
    // Hook and wrapper modes run on every provider prompt and tool call and
    // never use Tokio, so they finish before any runtime or worker exists.
    match run_helper_mode() {
        Some(0) => return,
        Some(code) => std::process::exit(code),
        None => {}
    }
    // Only the host looks providers up; its login shell runs while the
    // runtime, journal, broker and stores open.
    let login_shell = interface_host::bootstrap::LoginShellPathProbe::start();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Interface Rust host could not start its runtime: {error}");
            std::process::exit(1);
        }
    };
    runtime.block_on(host_main(login_shell));
}

/// Runs the private hook or wrapper mode named by argv, returning its exit
/// code, or `None` when this invocation is the host itself.
fn run_helper_mode() -> Option<i32> {
    if std::env::args().nth(1).as_deref() == Some(interface_terminal::native_checkpoint::HOOK_MODE)
    {
        let args = std::env::args().skip(2).collect::<Vec<_>>();
        if let [provider, event, token, root] = args.as_slice() {
            let profile = match provider.as_str() {
                "agy" => Some(TerminalLaunchProfileId::Gemini),
                "muse" => Some(TerminalLaunchProfileId::Muse),
                "kimi" => Some(TerminalLaunchProfileId::Kimi),
                _ => None,
            };
            if let Some(profile) = profile {
                let _ = interface_host::run_native_terminal_checkpoint_hook(
                    profile,
                    event,
                    token,
                    Path::new(root),
                    std::io::stdin().lock(),
                );
            }
        }
        return Some(0);
    }
    if interface_host::delegation_hook::requested() {
        let _ =
            interface_host::delegation_hook::run(std::io::stdin().lock(), std::io::stdout().lock());
        return Some(0);
    }
    if manual_claude_wrapper_requested() {
        return Some(match run_manual_claude_wrapper() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        });
    }
    if manual_codex_wrapper_requested() {
        return Some(match run_manual_codex_wrapper() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        });
    }
    match claude_terminal_session_hook_mode() {
        ClaudeTerminalSessionHookMode::Valid {
            capture_token,
            receipts_root,
        } => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let result = run_claude_terminal_session_hook(
                &capture_token,
                &receipts_root,
                stdin.lock(),
                stdout.lock(),
            );
            Some(i32::from(result.is_err()))
        }
        ClaudeTerminalSessionHookMode::CodexValid { receipts_root } => {
            // Codex adds hook stdout to the model's context and surfaces a
            // failing hook to the user. Valid starts supply delegation context;
            // rejected payloads and capture failures remain silent.
            let capture_token = std::env::var(CODEX_SESSION_CAPTURE_TOKEN_ENV).unwrap_or_default();
            let stdin = std::io::stdin();
            let _ = run_codex_terminal_session_hook(&capture_token, &receipts_root, stdin.lock());
            Some(0)
        }
        ClaudeTerminalSessionHookMode::InvalidReserved => Some(0),
        ClaudeTerminalSessionHookMode::NotHook => None,
    }
}

#[allow(clippy::too_many_lines)] // Keep the one-writer owner graph visibly composed before ready.
async fn host_main(login_shell: interface_host::bootstrap::LoginShellPathProbe) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    // stderr reaches the Dev console; WARN and ERROR also go to `host.log`
    // in the data root, which a packaged app's user can reach (Help menu).
    let host_log = interface_host::host_log::HostLogSink::default();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr.and(host_log.clone().with_max_level(tracing::Level::WARN)))
        .with_ansi(false)
        .init();
    let roots = match configured_host_roots() {
        Ok(roots) => roots,
        Err(error) => {
            tracing::error!(%error, "Interface Rust service roots were refused");
            std::process::exit(1);
        }
    };
    if let Err(error) = host_log.attach(&roots.data_root) {
        tracing::warn!(%error, "host.log is unavailable; warnings reach stderr only");
    }
    let store_notices = Arc::new(interface_host::diagnostics::StoreNotices::default());
    let prepared_orchestration = match prepare_orchestration_host(&roots.data_root).await {
        Ok(prepared) => Some(prepared),
        Err(error) => {
            tracing::warn!(
                %error,
                "Interface shared orchestration is unavailable; ordinary provider sessions remain available"
            );
            None
        }
    };
    let orchestration_runtime_epoch = prepared_orchestration.as_ref().map_or_else(
        || uuid::Uuid::new_v4().to_string(),
        |prepared| prepared.runtime_epoch().to_owned(),
    );
    // The durable writer opens before a provider can receive collaboration
    // metadata. This is also the proof used by the frozen safe-degradation
    // rule when the owner-private broker itself cannot start.
    let mut collaboration_journal =
        match CollaborationJournal::open(roots.data_root.join("collaboration")) {
            Ok(journal) => Some(journal),
            Err(error) => {
                tracing::error!(%error, "Interface collaboration journal did not open");
                std::process::exit(1);
            }
        };
    let deferred_ingress = Arc::new(DeferredCollaborationModelIngress::new());
    let model_ingress: Arc<dyn CollaborationModelIngress> = deferred_ingress.clone();
    let collaboration_broker = match CollaborationBrokerRuntime::start(
        CollaborationBrokerRuntimeOptions::frozen(
            roots.data_root.join("collaboration"),
            model_ingress,
        ),
    )
    .await
    {
        Ok(broker) => Some(Arc::new(broker)),
        Err(error) => {
            if !close_terminal_collaboration_journal(&mut collaboration_journal) {
                tracing::error!(%error, "Interface collaboration broker did not open");
                std::process::exit(1);
            }
            tracing::warn!(
                %error,
                "Interface collaboration is unavailable; providers are starting without cross-model tools"
            );
            None
        }
    };
    let provider_bindings = collaboration_broker
        .as_ref()
        .map(|broker| broker.provider_bindings());
    let mut teardown = StartupTeardown {
        broker: collaboration_broker.clone(),
        journal: collaboration_journal,
        collaboration: None,
        orchestration: None,
        runtime: None,
    };
    let opened = match open_host_services_at_with_orchestration_and_collaboration(
        &roots.data_root,
        &roots.home,
        &orchestration_runtime_epoch,
        prepared_orchestration
            .as_ref()
            .is_some_and(interface_host::PreparedOrchestrationHost::owns_stores),
        provider_bindings.as_ref(),
        Some(login_shell),
        PRODUCT_VERSION,
    ) {
        Ok(opened) => opened,
        Err(error) => match teardown
            .abort(&error, "Interface Rust service graph did not open")
            .await {},
    };
    let OpenedHostServices {
        data_root,
        desktop,
        runtime,
        terminal_launch_catalog,
        claude_projects_root,
        claude_terminal_session_receipts_root,
        antigravity_usage,
        opencode_usage,
        worktrees,
        orchestration_runtime_epoch,
        voice_mcp,
        agent_tools,
        team_tools,
        registry_quarantined,
        provider_catalog,
        provider_accounts,
        folder_trust,
    } = opened;
    // Models & providers → Refresh and the launcher's Try again find the
    // provider CLIs again (protocol 41).
    let provider_refresh = Arc::new(interface_host::provider_refresh::ProviderRefresh::new(
        Arc::new(provider_catalog),
    ));
    if let Some(artifact) = &registry_quarantined {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::Sessions,
            artifact,
        );
    }
    let session_runtime = Arc::new(runtime);
    teardown.runtime = Some(session_runtime.clone());
    if let Some(smoke) = &roots.smoke
        && let Err(error) = smoke.assert_unchanged()
    {
        match teardown
            .abort(&error, "Interface release smoke roots changed identity")
            .await {}
    }
    let config = HostServerConfig {
        host_version: PRODUCT_VERSION,
        ..HostServerConfig::default()
    };
    let (push_sender, push_receiver) = tokio::sync::mpsc::channel(config.outbound_capacity);
    let mut terminal_data_plane =
        match TerminalDataPlane::from_process_pipes(TerminalDataTransportConfig::default()) {
            Ok(plane) => Some(plane),
            Err(error) => {
                // Standalone protocol tests and non-Electron embedders may not
                // provide descriptors 3/4. Keep the control protocol available,
                // but terminal creation fails explicitly instead of spawning an
                // interactive process whose bytes have nowhere to go.
                tracing::warn!(%error, "integrated terminal data descriptors are unavailable");
                None
            }
        };
    desktop.set_agents_root(host_agents_root(
        &roots.data_root,
        BaseDirs::new()
            .map(|base| production_data_root(&base))
            .as_deref(),
        &roots.home,
        std::env::var("INTERFACE_DEV_SUPERVISED").as_deref() == Ok("1"),
    ));
    // One whole-registry coalescer for the runtime, collaboration and
    // orchestration owners; only this function closes it, after they drain.
    // Threads reconcile against every round it sends.
    let thread_registry = Arc::new(interface_host::ThreadRegistrySignal::default());
    // The permission mode the owner chose for each session: threads resume
    // in it, and (protocol 68) the desktop's registry rows carry it.
    let session_access = interface_host::session_access::HostSessionAccess::open(&data_root);
    if let Some(artifact) = session_access
        .as_ref()
        .and_then(|access| access.quarantined_artifact())
    {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::PermissionModes,
            &artifact,
        );
    }
    let registry_pushes = SharedRegistryPushes::start_observed(
        session_runtime.clone(),
        push_sender.clone(),
        thread_registry.clone(),
        session_access
            .clone()
            .map(|access| access as Arc<dyn interface_host::dispatcher::SessionAccessMemory>),
    );
    let collaboration_projection = Arc::new(DesktopCollaborationProjection::new());
    let orchestration_control = if let Some(prepared) = prepared_orchestration {
        let orchestration_worktrees: Arc<dyn WorktreeService> = worktrees.clone();
        match OrchestrationHostControl::start_with_services_and_projection(
            prepared,
            session_runtime.as_ref(),
            collaboration_projection.clone(),
            Some(orchestration_worktrees),
            Some(push_sender.clone()),
            Some(registry_pushes.clone()),
        )
        .await
        {
            Ok(control) => Arc::new(control),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Interface shared orchestration host did not open; ordinary provider sessions remain available"
                );
                Arc::new(OrchestrationHostControl::unavailable(error))
            }
        }
    } else {
        Arc::new(OrchestrationHostControl::unavailable(
            "Shared orchestration startup failed.",
        ))
    };
    teardown.orchestration = Some(orchestration_control.clone());
    // Agents never start a consult or handoff: its target would run outside their gate.
    let mut collaboration_port = SessionRuntimeCollaborationPort::new(
        session_runtime.clone(),
        collaboration_projection.clone(),
    )
    .with_agent_sessions(desktop.clone());
    if collaboration_broker.is_some() {
        let collaboration_worktrees: Arc<dyn ManagedWorktreeInventory> = worktrees.clone();
        collaboration_port = collaboration_port
            .with_shared_pushes(push_sender.clone(), registry_pushes.clone())
            .with_worktree_inventory(collaboration_worktrees);
    }
    let collaboration_port = Arc::new(collaboration_port);
    let collaboration = if let Some(journal) = teardown.journal.take() {
        match DurableCollaborationService::start(
            CollaborationCoordinator::new(journal),
            collaboration_port.clone(),
            collaboration_projection,
            orchestration_runtime_epoch.clone(),
            Some(push_sender.clone()),
        )
        .await
        {
            Ok(service) => Some(Arc::new(service)),
            Err(error) => match teardown
                .abort(&error, "Interface collaboration recovery failed")
                .await {},
        }
    } else {
        None
    };
    if let Some(collaboration) = &collaboration {
        teardown.collaboration = Some(collaboration.clone());
        let model_ingress: Arc<dyn CollaborationModelIngress> = collaboration.clone();
        if let Err(error) = deferred_ingress.attach(model_ingress) {
            match teardown
                .abort(
                    &error,
                    "Interface collaboration model ingress did not attach",
                )
                .await {}
        }
    }
    drop(teardown);
    let collaboration_hooks = collaboration.as_ref().map(|collaboration| {
        let hooks: Arc<dyn CollaborationSessionHooks> = collaboration.clone();
        hooks
    });
    let pasted_images_dir = PrivateDirectory::open_or_create(roots.data_root.join("pasted-images"))
        .ok()
        .map(|directory| directory.path().to_path_buf());
    let prompt_context_dispatcher = Arc::new(PromptContextHostDispatcher::system(
        session_runtime.clone(),
        desktop.clone(),
        pasted_images_dir,
    ));
    let prompt_context: Arc<dyn HostDispatcher> = prompt_context_dispatcher.clone();
    let updater_service = interface_host::updater::updater_service(
        data_root.clone(),
        ReleaseChannel::compiled(),
        std::env::var("INTERFACE_UPDATE_URL").ok(),
        Arc::new(UreqFetcher::new()),
        Arc::new(SystemUpdaterVerifier),
        Arc::new(SystemUpdaterClock),
    );
    let channel_schedule = updater_service.spawn_channel_schedule();
    let update_forwarder = spawn_update_state_forwarder(&updater_service, push_sender.clone());
    let updater_version: Arc<dyn interface_host::diagnostics::AppVersion> = updater_service.clone();
    let theme_watcher = interface_host::theme_watch::spawn_theme_folder_watcher(
        interface_desktop_services::custom_themes_directory(&roots.home),
        push_sender.clone(),
        interface_host::theme_watch::THEME_FOLDER_POLL,
    );
    let updater = Arc::new(UpdaterHostDispatcher::new(updater_service));
    let updater_owner: Arc<dyn HostDispatcher> = updater.clone();
    let native_runtime: Arc<dyn NativeRuntimeView> = session_runtime.clone();
    let release_runtime: Arc<dyn NativeRuntimeView> = session_runtime.clone();
    let release_worktrees: Arc<dyn ManagedWorktreeInventory> = worktrees.clone();
    let automatic_worktree_release = Arc::new(
        AutomaticWorktreeReleaseService::new(
            release_runtime,
            desktop.clone(),
            release_worktrees,
            push_sender.clone(),
            collaboration_hooks.clone(),
        )
        .with_shared_registry_pushes(registry_pushes.clone()),
    );
    let terminal_runtime: Arc<dyn NativeRuntimeView> = session_runtime.clone();
    let terminal_data_sender = terminal_data_plane
        .as_ref()
        .map(|plane| plane.outbound.clone());
    let worker_executable = terminal_launch_catalog
        .executable(TerminalLaunchProfileId::Opencode)
        .map(Path::to_path_buf);
    let claude_titles = terminal_launch_catalog
        .executable(TerminalLaunchProfileId::Claude)
        .map(Path::to_path_buf);
    let terminal_dispatcher = match TerminalHostDispatcher::production(
        terminal_runtime,
        desktop.clone(),
        automatic_worktree_release.clone(),
        push_sender.clone(),
        terminal_data_sender,
        terminal_launch_catalog,
        claude_projects_root,
        claude_terminal_session_receipts_root,
    ) {
        Ok(dispatcher) => Arc::new(dispatcher),
        Err(error) => {
            tracing::error!(%error, "Interface integrated terminal owner did not open");
            std::process::exit(1);
        }
    };
    terminal_dispatcher.attach_provider_refresh(provider_refresh.clone());
    // Protocol 60: provider panes run on their provider's default account,
    // a sign-in pane on the account it names.
    if let Some(accounts) = &provider_accounts {
        terminal_dispatcher.install_accounts(accounts.clone());
    }
    terminal_dispatcher.attach_prompt_context(prompt_context_dispatcher.service());
    // Protocol 51: check each provider CLI against what Interface depends on
    // when its version changes (help and version output only).
    if let Some(watch) = interface_host::provider_canary::ProviderCanaryWatch::open(&data_root) {
        terminal_dispatcher
            .start_provider_canary(watch, interface_host::provider_canary::STARTUP_CHECK_DELAY);
    }
    if let Err(error) = terminal_dispatcher.start_shell_handoff() {
        tracing::error!(%error, "Interface integrated terminal shell handoff did not open");
        std::process::exit(1);
    }
    if let Some(data_plane) = terminal_data_plane.as_mut() {
        let failures = data_plane.subscribe_failures();
        let (_empty_sender, empty_receiver) = tokio::sync::mpsc::channel(1);
        let inbound = std::mem::replace(&mut data_plane.inbound, empty_receiver);
        if let Err(error) = terminal_dispatcher.start_input_pump(inbound) {
            tracing::error!(%error, "Interface integrated terminal input owner did not open");
            std::process::exit(1);
        }
        if let Err(error) = terminal_dispatcher.start_failure_monitor(failures) {
            tracing::error!(%error, "Interface integrated terminal failure monitor did not open");
            std::process::exit(1);
        }
    }
    let pane_mcp = PaneMcpServer::start_with_reports(
        &data_root,
        terminal_dispatcher.clone(),
        worker_executable,
    );
    // Grok and Antigravity read the pane server from an `iface` entry in their
    // own user config, which the owner turns on in Settings (protocol 48);
    // nothing is written at start.
    let terminal_owner: Arc<dyn HostDispatcher> = terminal_dispatcher.clone();
    let desktop_owner: Arc<dyn HostDispatcher> = Arc::new(
        DesktopHostDispatcher::new(desktop.clone())
            .with_session_root_provider(session_runtime.clone())
            .with_worktree_release(automatic_worktree_release.clone()),
    );
    // Threads: the one `threads.v2.json` writer. Away is attached once it
    // exists, since it sends through the runtime this host hooks.
    let thread_workers = Arc::new(interface_host::threads::WorkerSessions {
        orchestration: Some(OrchestrationHostControl::clone(&orchestration_control)),
        away: std::sync::OnceLock::new(),
    });
    let thread_store = interface_host::ThreadHost::open_store(&data_root);
    if let Some(artifact) = thread_store
        .as_ref()
        .ok()
        .and_then(interface_settings::threads::ThreadStore::quarantined_artifact)
    {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::Threads,
            artifact,
        );
    }
    // One writer per provider session: a Code pane's session is never driven
    // from Threads or the runtime too (review C1).
    let session_claims = Arc::new(interface_runtime::SessionClaims::new(
        terminal_dispatcher.clone() as Arc<dyn interface_runtime::TerminalSessionBindings>,
    ));
    // A consult or handoff never writes to a session a pane holds (review B4).
    if collaboration_port
        .set_session_claims(session_claims.clone())
        .is_err()
    {
        tracing::error!("the collaboration port's session claims were already attached");
    }
    // Second opinion resumes a thread through the guarded runtime, which
    // exists only below.
    let thread_runtime = Arc::new(interface_host::agents::LateDispatcher::default());
    let thread_host = interface_host::ThreadHost::new(interface_host::ThreadHostConfig {
        store: thread_store,
        ports: interface_host::ThreadPorts {
            sessions: Arc::new(interface_host::threads::RuntimeThreadSessions(
                session_runtime.clone(),
            )),
            hidden: thread_workers.clone(),
            authority: Arc::new(interface_host::threads::ListDirAuthority(
                desktop_owner.clone(),
            )),
            git: Arc::new(interface_host::threads::SystemThreadGit),
            pull_requests: Arc::new(interface_host::threads::GhPullRequests),
            ship: Arc::new(interface_host::threads::SystemShipTools),
            titles: Arc::new(interface_host::threads::ClaudeThreadTitles {
                executable: claude_titles,
                accounts: provider_accounts.clone(),
            }),
            clock: Arc::new(interface_host::threads::now_ms),
            claims: session_claims.clone(),
            consults: Arc::new(interface_host::threads::HostThreadConsults {
                runtime: thread_runtime.clone(),
                collaboration: collaboration
                    .clone()
                    .map(|collaboration| collaboration as Arc<dyn CollaborationDesktopControl>),
            }),
        },
        pushes: Some(push_sender.clone()),
        registry_pushes: Some(registry_pushes.clone()),
        registry: thread_registry.clone(),
        // Protocol 56: each thread's own record of what happened.
        journal: Some(interface_host::ThreadJournalConfig {
            data_root: data_root.clone(),
            notices: Some(store_notices.clone()),
        }),
    });
    // Settings › Threads › Pane tools without asking rebuilds the launch catalog.
    thread_host.attach_provider_refresh(provider_refresh.clone());
    let runtime_worktrees: Arc<dyn WorktreeService> = worktrees.clone();
    let voice_tap = VoiceEventTap::default();
    let away_tap = interface_host::project_away::AwayEventTap::new(Arc::new(voice_tap.clone()));
    // The agent host answers approvals on the unguarded runtime, which is attached below.
    let agent_runtime = Arc::new(interface_host::agents::LateDispatcher::default());
    let agent_host = interface_host::agents::AgentHost::new(
        desktop.clone(),
        agent_runtime.clone(),
        session_runtime.clone(),
    );
    agent_tools.attach(interface_host::agent_tools::AgentToolServers::new(
        &agent_host,
    ));
    let mut runtime_dispatcher = RuntimeHostDispatcher::with_pushes_and_worktrees(
        session_runtime.clone(),
        push_sender.clone(),
        runtime_worktrees,
    )
    .with_shared_registry_pushes(registry_pushes.clone())
    .with_worktree_release(automatic_worktree_release.clone())
    .with_event_observer(Arc::new(interface_host::agents::AgentEventTap {
        desktop: desktop.clone(),
        next: Arc::new(thread_host.run_tap(Arc::new(away_tap.clone()))),
        host: agent_host.clone(),
    }))
    .with_resume_requirements(agent_host.clone())
    .with_session_claims(session_claims.clone())
    .with_provider_refresh(provider_refresh)
    .with_send_hooks(Arc::new(thread_host.clone()))
    // A thread whose provider keeps no history shows its journal.
    .with_history_fallback(Arc::new(thread_host.clone()))
    .with_prompt_recovery(desktop.clone())
    // Sessions a listing finds (a terminal CLI run) get their thread records.
    .with_listing_observer(thread_registry);
    if let Some(hooks) = collaboration_hooks.clone() {
        runtime_dispatcher = runtime_dispatcher.with_collaboration_hooks(hooks);
    }
    // Threads resume in the permission mode the owner chose, across restarts.
    if let Some(access) = session_access.clone() {
        runtime_dispatcher = runtime_dispatcher.with_session_access(access);
    }
    // Protocol 57: threads team tools started resume one hop down.
    runtime_dispatcher = runtime_dispatcher.with_team_children(Arc::new(thread_host.clone()));
    let runtime_dispatcher = Arc::new(runtime_dispatcher);
    let unguarded_runtime: Arc<dyn HostDispatcher> = runtime_dispatcher.clone();
    agent_runtime.attach(unguarded_runtime.clone());
    // Every other caller answers approvals, resumes and sends through the agent guard.
    let runtime: Arc<dyn HostDispatcher> = Arc::new(
        interface_host::agents::AgentRuntimeGuard::new(unguarded_runtime, agent_host.clone()),
    );
    thread_runtime.attach(runtime.clone());
    // Protocol 57: team tools start and message threads through the guarded
    // runtime, reading the permission mode each caller runs in.
    let team_store = interface_host::ThreadHost::open_team_store(&data_root);
    if let Some(artifact) = team_store
        .as_ref()
        .ok()
        .and_then(interface_settings::thread_team::ThreadTeamStore::quarantined_artifact)
    {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::ThreadTeam,
            artifact,
        );
    }
    thread_host.attach_team(interface_host::ThreadTeamConfig {
        runtime: runtime.clone(),
        access: session_access
            .clone()
            .map(|access| access as Arc<dyn interface_host::dispatcher::SessionAccessMemory>),
        store: team_store,
    });
    team_tools.attach(interface_host::team_tools::TeamToolServers::new(
        thread_host.clone(),
    ));
    // Protocol 63: each thread's stashed drafts, and thread references
    // resolved (and their pasted images kept) by the threads owner.
    let stash_store = interface_host::ThreadHost::open_stash_store(&data_root);
    if let Some(artifact) = stash_store
        .as_ref()
        .ok()
        .and_then(interface_settings::draft_stash::DraftStashStore::quarantined_artifact)
    {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::DraftStash,
            artifact,
        );
    }
    thread_host.attach_stash(stash_store);
    // Protocol 67: the secrets the owner gives agents, in owner-only files
    // under the data root (expired ones are swept now and hourly).
    thread_host.attach_secrets(&data_root);
    // Protocol 67: each thread's open side question (its side thread hidden).
    thread_host.attach_sides(interface_host::ThreadHost::open_side_store(&data_root));
    prompt_context_dispatcher
        .service()
        .attach_threads(Arc::new(thread_host.clone()));
    // Protocol 62: scheduled prompts and pull-request watch, sent through
    // the same guarded runtime; `iface trigger` reaches them over host.sock.
    let schedule_store = interface_host::ThreadHost::open_schedule_store(&data_root);
    if let Some(artifact) = schedule_store
        .as_ref()
        .ok()
        .and_then(interface_settings::thread_schedules::ThreadScheduleStore::quarantined_artifact)
    {
        store_notices.record(
            interface_host::diagnostics::SetAsideStore::ThreadSchedules,
            artifact,
        );
    }
    thread_host.attach_schedules(schedule_store);
    // Protocol 66: Resume at reset may reuse a prompt the limit refused.
    thread_host.attach_prompt_receipts(desktop.clone());
    orchestration_control.attach_schedule_trigger(Arc::new(thread_host.clone()));
    // Protocol 65: agents outside Interface (`iface mcp`) reach the threads
    // through one owner-only socket; off until the owner turns it on.
    let outside_agents =
        interface_host::outside_agents::OutsideAgentsServer::start(thread_host.clone(), &data_root);
    let agent_runner = interface_host::agents::AgentRunner::start(agent_host.clone());
    // Each Away loop runs as a thread of this host (review B3).
    let away_service = interface_host::project_away::AwayHostDispatcher::new(
        &data_root,
        runtime.clone(),
        away_tap,
        terminal_dispatcher.clone(),
        Some(thread_host.clone()),
    );
    let _ = thread_workers.away.set(away_service.clone());
    let voice_service = VoiceService::new(VoiceDeps {
        runtime: session_runtime.clone(),
        sessions: runtime.clone(),
        terminal: terminal_dispatcher.clone(),
        desktop: desktop.clone(),
        pushes: push_sender.clone(),
        data_root: data_root.clone(),
        events: voice_tap,
    });
    voice_mcp.attach(voice_service.clone());
    let voice_owner: Arc<dyn HostDispatcher> = voice_service.clone();
    let mut session_management_dispatcher =
        SessionManagementHostDispatcher::with_pushes(session_runtime.clone(), push_sender.clone())
            .with_shared_registry_pushes(registry_pushes.clone())
            .with_worktree_release(automatic_worktree_release.clone())
            .with_rename_hooks(Arc::new(thread_host.clone()))
            .with_removal_hooks(Arc::new(thread_host.clone()));
    let supplemental_usage: Vec<Arc<dyn SupplementalUsageReader>> = [
        antigravity_usage.map(|reader| reader as Arc<dyn SupplementalUsageReader>),
        opencode_usage.map(|reader| reader as Arc<dyn SupplementalUsageReader>),
    ]
    .into_iter()
    .flatten()
    .collect();
    session_management_dispatcher =
        session_management_dispatcher.with_supplemental_usage(supplemental_usage);
    // Protocol 68: the listing carries each session's permission mode.
    if let Some(access) = session_access.clone() {
        session_management_dispatcher = session_management_dispatcher.with_session_access(access);
    }
    if let Some(hooks) = collaboration_hooks.clone() {
        session_management_dispatcher =
            session_management_dispatcher.with_collaboration_hooks(hooks);
    }
    // `sendLive` reaches agent sessions too, so it passes the same read-only check.
    let session_management: Arc<dyn HostDispatcher> =
        Arc::new(interface_host::agents::AgentRuntimeGuard::new(
            Arc::new(session_management_dispatcher),
            agent_host.clone(),
        ));
    // Protocol 66: a limit without a reset is read from the usage windows.
    thread_host.attach_usage(session_management.clone());
    let native_worktrees: Arc<dyn ManagedWorktreeInventory> = worktrees;
    let mut native_dispatcher = NativeHostDispatcher::new(
        native_runtime,
        desktop,
        native_worktrees,
        Arc::new(SystemExternalTerminalBackend),
    );
    native_dispatcher = native_dispatcher
        .with_pushes(push_sender.clone())
        .with_shared_registry_pushes(registry_pushes.clone());
    if let Some(hooks) = collaboration_hooks {
        native_dispatcher = native_dispatcher.with_collaboration_hooks(hooks);
    }
    let native: Arc<dyn HostDispatcher> = Arc::new(native_dispatcher);
    let orchestration_owner: Arc<dyn HostDispatcher> = orchestration_control.clone();
    let collaboration_owner: Arc<dyn HostDispatcher> = collaboration.as_ref().map_or_else(
        || Arc::new(CollaborationHostDispatcher::unavailable()) as Arc<dyn HostDispatcher>,
        |collaboration| {
            Arc::new(CollaborationHostDispatcher::new(collaboration.clone()))
                as Arc<dyn HostDispatcher>
        },
    );
    // The phone answers a thread's approval through the same guarded owner.
    let phone_runtime = runtime.clone();
    let phone_claims = session_claims;
    let dispatcher = match OperationOwnerRouter::new(
        runtime,
        session_management,
        desktop_owner,
        native,
        orchestration_owner,
        collaboration_owner,
        prompt_context,
        terminal_owner,
        updater_owner,
        voice_owner,
    ) {
        Ok(dispatcher) => dispatcher,
        Err(error) => {
            tracing::error!(%error, "Interface operation ownership is invalid");
            std::process::exit(1);
        }
    };
    let remote = Arc::new(interface_host::remote::RemoteHostDispatcher::new(
        terminal_dispatcher.clone(),
        session_runtime.clone(),
        std::env::var_os("INTERFACE_REMOTE_ASSETS").map_or_else(
            || std::path::PathBuf::from("electron/dist-remote"),
            std::path::PathBuf::from,
        ),
        Some(interface_host::remote::PhoneThreadPorts {
            threads: thread_host.clone(),
            runtime: phone_runtime,
            claims: phone_claims,
        }),
    ));
    remote.restore().await;
    if let Some(accounts) = &provider_accounts {
        thread_host.attach_accounts(accounts.clone());
    }
    // Protocol 72: untrusted Claude threads say so, and the owner's answer
    // is recorded here.
    thread_host.attach_folder_trust(folder_trust);
    thread_host.start();
    let diagnostics = interface_host::diagnostics::DiagnosticsHost::new(
        interface_host::diagnostics::DiagnosticsHostConfig {
            data_root: data_root.clone(),
            home: roots.home.clone(),
            product_version: PRODUCT_VERSION.to_owned(),
            app_version: Some(updater_version),
            probes: session_runtime.clone(),
            notices: store_notices,
            revealer: Arc::new(interface_host::diagnostics::FinderRevealer),
        },
    );
    let dispatcher = dispatcher
        .with_remote(remote.clone())
        .with_away(away_service.clone())
        .with_agents(agent_host)
        .with_threads(Arc::new(thread_host.clone()))
        .with_diagnostics(Arc::new(diagnostics));
    let dispatcher = match provider_accounts {
        Some(accounts) => dispatcher.with_provider_accounts(Arc::new(
            interface_host::provider_accounts::ProviderAccountsHost::new(accounts),
        )),
        None => dispatcher,
    };
    let server = HostServer::with_pushes(Arc::new(dispatcher), push_receiver, config);
    let server_result = server.run(tokio::io::stdin(), tokio::io::stdout()).await;
    remote.close().await;
    shutdown::terminals_first(terminal_dispatcher.close(), async {
        shutdown::timed("agent scheduler", agent_runner.shutdown()).await;
        shutdown::timed("away", away_service.shutdown()).await;
        shutdown::timed("voice", voice_service.shutdown()).await;
        drop(outside_agents);
        shutdown::timed("threads", thread_host.shutdown()).await;
        if let Some(pane_mcp) = &pane_mcp {
            shutdown::timed("pane delegates", pane_mcp.shutdown()).await;
        }
    })
    .await;
    if let Some(data_plane) = terminal_data_plane.take() {
        shutdown::timed("terminal data plane", data_plane.shutdown()).await;
    }
    shutdown::timed("updater", updater.close()).await;
    update_forwarder.abort();
    channel_schedule.abort();
    theme_watcher.abort();
    let broker_stop_result = shutdown::timed(
        "broker stop",
        stop_collaboration_broker(collaboration_broker.as_deref()),
    )
    .await;
    let collaboration_close_result = match collaboration {
        Some(collaboration) => shutdown::timed("collaboration", collaboration.close()).await,
        None => Ok(()),
    };
    let close_result = shutdown::timed("orchestration", orchestration_control.close()).await;
    let runtime_close_result = shutdown::timed("providers", session_runtime.shutdown()).await;
    shutdown::timed("runtime events", runtime_dispatcher.close_event_pumps()).await;
    // After the pumps: their last events and closes are journalled first.
    shutdown::timed("thread journal", thread_host.drain_journal()).await;
    // Every registry owner has drained: send the final frame and stop.
    shutdown::timed("registry pushes", registry_pushes.close()).await;
    shutdown::timed("worktree release", automatic_worktree_release.close()).await;
    // A release above can point a session back home after provider shutdown
    // already flushed the session cache: write that change before exiting.
    shutdown::timed(
        "session cache",
        session_runtime.services().flush_registry_cache(),
    )
    .await;
    let broker_drain_result = shutdown::timed(
        "broker drain",
        drain_collaboration_broker(collaboration_broker.as_deref()),
    )
    .await;
    // The first failure, in this order, fails the process.
    for (failed, what) in [
        (
            server_result.err().map(|error| error.to_string()),
            "Interface Rust host stopped",
        ),
        (
            close_result.err(),
            "Interface orchestration shutdown failed",
        ),
        (
            collaboration_close_result
                .err()
                .map(|error| error.to_string()),
            "Interface collaboration shutdown failed",
        ),
        (
            runtime_close_result.err().map(|error| error.to_string()),
            "Interface provider runtime shutdown failed",
        ),
        (
            broker_stop_result.err().map(|error| error.to_string()),
            "Interface collaboration broker shutdown failed",
        ),
        (
            broker_drain_result.err().map(|error| error.to_string()),
            "Interface collaboration broker drain failed",
        ),
    ] {
        if let Some(error) = failed {
            tracing::error!(%error, "{what}");
            std::process::exit(1);
        }
    }
}

/// What a failed startup has opened so far. `abort` closes it in the order
/// the normal shutdown uses (stop the broker, close the journal, the
/// collaboration service, orchestration and the runtime, then drain the
/// broker) and exits.
struct StartupTeardown {
    broker: Option<Arc<CollaborationBrokerRuntime>>,
    journal: Option<CollaborationJournal>,
    collaboration: Option<Arc<DurableCollaborationService<SessionRuntimeCollaborationPort>>>,
    orchestration: Option<Arc<OrchestrationHostControl>>,
    runtime: Option<Arc<interface_runtime::SessionRuntime>>,
}

impl StartupTeardown {
    async fn abort(&mut self, error: &dyn std::fmt::Display, what: &'static str) -> Infallible {
        let _ = stop_collaboration_broker(self.broker.as_deref()).await;
        close_collaboration_journal(&mut self.journal);
        if let Some(collaboration) = &self.collaboration {
            let _ = collaboration.close().await;
        }
        if let Some(orchestration) = &self.orchestration {
            let _ = orchestration.close().await;
        }
        if let Some(runtime) = &self.runtime {
            let _ = runtime.shutdown().await;
        }
        let _ = drain_collaboration_broker(self.broker.as_deref()).await;
        tracing::error!(%error, "{what}");
        std::process::exit(1);
    }
}

/// The capture helper is an exact correlated private mode, not another host
/// startup option. Rejecting missing, malformed, or extra argv prevents an
/// accidental flag collision from turning a normal host invocation into a
/// hook process.
#[derive(Debug, Eq, PartialEq)]
enum ClaudeTerminalSessionHookMode {
    NotHook,
    InvalidReserved,
    Valid {
        capture_token: String,
        receipts_root: std::path::PathBuf,
    },
    /// The Codex hook carries its capture token in the environment, so argv
    /// names only the receipt root.
    CodexValid {
        receipts_root: std::path::PathBuf,
    },
}

fn claude_terminal_session_hook_mode() -> ClaudeTerminalSessionHookMode {
    claude_terminal_session_hook_mode_from(std::env::args_os().skip(1))
}

fn claude_terminal_session_hook_mode_from(
    arguments: impl IntoIterator<Item = OsString>,
) -> ClaudeTerminalSessionHookMode {
    let mut arguments = arguments.into_iter();
    let mode = arguments.next();
    if mode.as_deref() == Some(std::ffi::OsStr::new(CODEX_SESSION_HOOK_MODE)) {
        let Some(receipts_root) = arguments.next().map(std::path::PathBuf::from) else {
            return ClaudeTerminalSessionHookMode::InvalidReserved;
        };
        if arguments.next().is_some() || !receipts_root.is_absolute() {
            return ClaudeTerminalSessionHookMode::InvalidReserved;
        }
        return ClaudeTerminalSessionHookMode::CodexValid { receipts_root };
    }
    if mode.as_deref() != Some(std::ffi::OsStr::new(CLAUDE_SESSION_HOOK_MODE)) {
        return ClaudeTerminalSessionHookMode::NotHook;
    }
    let Some(token) = arguments.next().and_then(|token| token.into_string().ok()) else {
        return ClaudeTerminalSessionHookMode::InvalidReserved;
    };
    let Some(receipts_root) = arguments.next().map(std::path::PathBuf::from) else {
        return ClaudeTerminalSessionHookMode::InvalidReserved;
    };
    if arguments.next().is_some() || !canonical_uuid_v4(&token) || !receipts_root.is_absolute() {
        return ClaudeTerminalSessionHookMode::InvalidReserved;
    }
    ClaudeTerminalSessionHookMode::Valid {
        capture_token: token,
        receipts_root,
    }
}

#[cfg(test)]
mod claude_terminal_hook_mode_tests {
    use super::*;

    const TOKEN: &str = "11111111-1111-4111-8111-111111111111";
    const RECEIPTS: &str = "/private/tmp/interface-claude-receipts";

    fn mode(arguments: &[&str]) -> ClaudeTerminalSessionHookMode {
        claude_terminal_session_hook_mode_from(
            arguments.iter().map(|argument| OsString::from(*argument)),
        )
    }

    #[test]
    fn private_hook_mode_requires_exact_mode_and_canonical_token() {
        assert_eq!(
            mode(&[CLAUDE_SESSION_HOOK_MODE, TOKEN, RECEIPTS]),
            ClaudeTerminalSessionHookMode::Valid {
                capture_token: TOKEN.into(),
                receipts_root: RECEIPTS.into(),
            }
        );
        assert_eq!(mode(&[]), ClaudeTerminalSessionHookMode::NotHook);
        assert_eq!(
            mode(&[CODEX_SESSION_HOOK_MODE, RECEIPTS]),
            ClaudeTerminalSessionHookMode::CodexValid {
                receipts_root: RECEIPTS.into(),
            }
        );
        assert_eq!(
            mode(&[CODEX_SESSION_HOOK_MODE, "relative"]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[CODEX_SESSION_HOOK_MODE, RECEIPTS, TOKEN]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[CLAUDE_SESSION_HOOK_MODE]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&["--different-mode", TOKEN]),
            ClaudeTerminalSessionHookMode::NotHook
        );
        assert_eq!(
            mode(&[CLAUDE_SESSION_HOOK_MODE, TOKEN]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[CLAUDE_SESSION_HOOK_MODE, TOKEN, "relative"]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[CLAUDE_SESSION_HOOK_MODE, TOKEN, RECEIPTS, "unexpected"]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[
                CLAUDE_SESSION_HOOK_MODE,
                "11111111-1111-3111-8111-111111111111",
                RECEIPTS
            ]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
        assert_eq!(
            mode(&[
                CLAUDE_SESSION_HOOK_MODE,
                "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
                RECEIPTS
            ]),
            ClaudeTerminalSessionHookMode::InvalidReserved
        );
    }
}

async fn stop_collaboration_broker(
    broker: Option<&CollaborationBrokerRuntime>,
) -> Result<(), CollaborationBrokerRuntimeError> {
    match broker {
        Some(broker) => broker.stop().await,
        None => Ok(()),
    }
}

async fn drain_collaboration_broker(
    broker: Option<&CollaborationBrokerRuntime>,
) -> Result<(), CollaborationBrokerRuntimeError> {
    match broker {
        Some(broker) => broker.drain().await,
        None => Ok(()),
    }
}

fn close_terminal_collaboration_journal(journal: &mut Option<CollaborationJournal>) -> bool {
    let Some(mut journal) = journal.take() else {
        return false;
    };
    let terminal_only = journal
        .list()
        .is_ok_and(|records| records.iter().all(|record| record.state.is_terminal()));
    terminal_only && journal.close().is_ok()
}

fn close_collaboration_journal(journal: &mut Option<CollaborationJournal>) {
    if let Some(mut journal) = journal.take() {
        let _ = journal.close();
    }
}

struct ConfiguredHostRoots {
    data_root: std::path::PathBuf,
    home: std::path::PathBuf,
    smoke: Option<SmokeRoots>,
}

fn configured_host_roots() -> Result<ConfiguredHostRoots, Box<dyn Error>> {
    if let Some(roots) = configured_smoke_roots()? {
        return Ok(ConfiguredHostRoots {
            data_root: roots.data_root.path().to_path_buf(),
            home: roots.home.path().to_path_buf(),
            smoke: Some(roots),
        });
    }
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("INTERFACE_HOST_TEST_DATA_ROOT") {
        let home = std::env::var_os("INTERFACE_HOST_TEST_HOME")
            .ok_or("INTERFACE_HOST_TEST_HOME is required with the test data root")?;
        return Ok(ConfiguredHostRoots {
            data_root: root.into(),
            home: home.into(),
            smoke: None,
        });
    }
    let base = BaseDirs::new().ok_or("the platform data directory is unavailable")?;
    Ok(ConfiguredHostRoots {
        data_root: production_data_root(&base),
        home: base.home_dir().to_path_buf(),
        smoke: None,
    })
}

fn production_data_root(base: &BaseDirs) -> std::path::PathBuf {
    base.data_local_dir().join("Interface")
}

/// Agents live in `Interface Agents` only for a host on the production data root. Every other
/// data root is a separate profile with its own stores: the Dev app's (`INTERFACE_DEV_PROFILE`,
/// handed over as the debug-only test data root whether or not `dev.mjs` supervises it) and
/// the test and smoke roots. Those use `Interface Dev Agents`, so a Dev host never adopts
/// production agents. `INTERFACE_DEV_SUPERVISED` stays an extra Dev signal.
fn host_agents_root(
    data_root: &Path,
    production_data_root: Option<&Path>,
    home: &Path,
    dev_supervised: bool,
) -> std::path::PathBuf {
    let production = !dev_supervised && production_data_root == Some(data_root);
    interface_desktop_services::agents_root(home, !production)
}

const SMOKE_DATA_ROOT_ENV: &str = "INTERFACE_HOST_SMOKE_DATA_ROOT";
const SMOKE_HOME_ENV: &str = "INTERFACE_HOST_SMOKE_HOME";

struct SmokeRoots {
    data_root: PrivateDirectory,
    home: PrivateDirectory,
}

impl SmokeRoots {
    fn assert_unchanged(&self) -> Result<(), Box<dyn Error>> {
        self.data_root.assert_unchanged()?;
        self.home.assert_unchanged()?;
        Ok(())
    }
}

fn configured_smoke_roots() -> Result<Option<SmokeRoots>, Box<dyn Error>> {
    let base = BaseDirs::new().ok_or("the platform home directory is unavailable")?;
    validate_smoke_roots(
        std::env::var_os(SMOKE_DATA_ROOT_ENV),
        std::env::var_os(SMOKE_HOME_ENV),
        base.home_dir(),
    )
}

fn validate_smoke_roots(
    data_root: Option<OsString>,
    home: Option<OsString>,
    live_home: &Path,
) -> Result<Option<SmokeRoots>, Box<dyn Error>> {
    let (data_root, home) = match (data_root, home) {
        (None, None) => return Ok(None),
        (Some(data_root), Some(home)) => (data_root, home),
        _ => {
            return Err(
                format!("{SMOKE_DATA_ROOT_ENV} and {SMOKE_HOME_ENV} must be set together").into(),
            );
        }
    };
    let data_root = PrivateDirectory::open(Path::new(&data_root))?;
    let home = PrivateDirectory::open(Path::new(&home))?;
    let data_path = std::fs::canonicalize(data_root.path())?;
    let home_path = std::fs::canonicalize(home.path())?;
    let live_home = std::fs::canonicalize(live_home)?;
    if data_path == home_path
        || data_path.starts_with(&live_home)
        || home_path.starts_with(&live_home)
    {
        return Err(
            "release smoke roots must be distinct disposable directories outside the live home"
                .into(),
        );
    }
    Ok(Some(SmokeRoots { data_root, home }))
}

#[cfg(test)]
mod agents_root_tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn only_the_production_data_root_uses_the_production_agents_folder() {
        let home = Path::new("/Users/me");
        let production = PathBuf::from("/Users/me/Library/Application Support/Interface");
        let agents = home.join("Interface Agents");
        let dev_agents = home.join("Interface Dev Agents");
        assert_eq!(
            host_agents_root(&production, Some(&production), home, false),
            agents
        );
        assert_eq!(
            host_agents_root(
                Path::new("/Users/me/Library/Application Support/Interface/"),
                Some(&production),
                home,
                false
            ),
            agents
        );
        // The Dev app's profile, started supervised or from the Dev app bundle alone.
        let dev_profile = Path::new("/Users/me/Library/Application Support/Iface Dev");
        assert_eq!(
            host_agents_root(dev_profile, Some(&production), home, false),
            dev_agents
        );
        assert_eq!(
            host_agents_root(dev_profile, Some(&production), home, true),
            dev_agents
        );
        // Test and smoke roots, and a platform without a data directory.
        assert_eq!(
            host_agents_root(Path::new("/tmp/smoke/data"), Some(&production), home, false),
            dev_agents
        );
        assert_eq!(host_agents_root(&production, None, home, false), dev_agents);
        // The supervised Dev signal still wins on the production data root.
        assert_eq!(
            host_agents_root(&production, Some(&production), home, true),
            dev_agents
        );
    }
}

#[cfg(test)]
mod smoke_root_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn private_directory(path: &Path) {
        std::fs::create_dir(path).expect("create private fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("set private fixture mode");
    }

    #[test]
    fn release_smoke_override_requires_both_private_roots() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let data = temporary.path().join("data");
        private_directory(&data);
        let error = validate_smoke_roots(Some(data.into_os_string()), None, temporary.path())
            .err()
            .expect("unpaired override must fail");
        assert!(error.to_string().contains("must be set together"));
    }

    #[test]
    fn release_smoke_override_accepts_only_disposable_sibling_roots() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let live_home = temporary.path().join("live-home");
        let data = temporary.path().join("smoke-data");
        let home = temporary.path().join("smoke-home");
        private_directory(&live_home);
        private_directory(&data);
        private_directory(&home);

        let accepted = validate_smoke_roots(
            Some(data.clone().into_os_string()),
            Some(home.clone().into_os_string()),
            &live_home,
        )
        .expect("validate disposable roots")
        .expect("configured roots");
        assert_eq!(accepted.data_root.path(), data);
        assert_eq!(accepted.home.path(), home);

        let nested = live_home.join("nested");
        private_directory(&nested);
        let error = validate_smoke_roots(
            Some(nested.into_os_string()),
            Some(home.into_os_string()),
            &live_home,
        )
        .err()
        .expect("live-home descendant must fail");
        assert!(error.to_string().contains("outside the live home"));
    }
}

#[cfg(test)]
mod collaboration_startup_tests {
    use interface_collaboration::{
        CollaborationEngineId, CollaborationIntentInput, CollaborationMode, CollaborationOrigin,
        CollaborationPolicyOverrides, CreateCollaborationRecordOptions,
        create_collaboration_record, normalize_collaboration_intent,
    };

    use super::*;

    #[test]
    fn broker_failure_can_degrade_only_with_an_empty_terminal_journal() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let mut journal = Some(
            CollaborationJournal::open(temporary.path().join("collaboration"))
                .expect("empty journal"),
        );

        assert!(close_terminal_collaboration_journal(&mut journal));
        assert!(journal.is_none());
        assert!(!close_terminal_collaboration_journal(&mut journal));
    }

    #[test]
    fn broker_failure_cannot_degrade_with_nonterminal_durable_work() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let mut durable = CollaborationJournal::open(temporary.path().join("collaboration"))
            .expect("collaboration journal");
        let intent = normalize_collaboration_intent(
            &CollaborationIntentInput {
                version: Some(1),
                mode: CollaborationMode::Consult,
                source_session_key: "codex#source".to_owned(),
                target_engine: CollaborationEngineId::Claude,
                objective: "Review the current implementation.".to_owned(),
                client_request_id: "request-1".to_owned(),
                origin: CollaborationOrigin::Human,
            },
            None,
        )
        .expect("normalized intent");
        let record = create_collaboration_record(CreateCollaborationRecordOptions {
            collaboration_id: "collaboration-1".to_owned(),
            intent,
            created_at: "2026-08-27T12:00:00.000Z".to_owned(),
            depth: 0,
            limits: CollaborationPolicyOverrides::default(),
        })
        .expect("requested record");
        durable.create(record).expect("persist active record");
        let mut journal = Some(durable);

        assert!(!close_terminal_collaboration_journal(&mut journal));
        assert!(journal.is_none());
    }
}
