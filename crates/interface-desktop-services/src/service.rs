mod acp_profiles;
#[path = "agent_consultation.rs"]
mod agent_consultation;
#[path = "agent_control.rs"]
mod agent_control;
#[path = "agent_work.rs"]
mod agent_work;
mod appearance;
mod background;
mod customization;
mod folder_trust;
#[path = "harness.rs"]
mod harness;
mod terminal_state;
mod workspace_leases;
mod workspace_ops;
pub use agent_control::AgentThreadLaunch;
pub use agent_work::AGENT_WORK_PROJECTION_FAILED;
pub use folder_trust::{FolderTrustDecision, FolderTrustState};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
pub use terminal_state::{SavedTerminalProviderSessionPersistence, TerminalReopenAuthorityState};

use acp_profiles::RecoverableAcpProfileStore;
use appearance::RecoverableAppearanceStore;
use interface_common::paths::lexically_normalized;
use interface_contracts::ipc::CHANNELS;
use interface_review::{ReviewCheckRunner, ReviewGitRunner};
use interface_settings::{
    AcpProfileStore, AppearanceStore, SettingsError, TerminalReopenAuthorityStore,
    TerminalWorkspaceStore, resolve_default_directory, terminal_reopen_authority_path,
};
use interface_workspace::{WorkspaceAuthority, WorkspaceEditorError};
use serde::Serialize;
use serde_json::Value;
use terminal_state::{
    RecoverableTerminalReopenAuthorityStore, RecoverableTerminalWorkspaceStore, TerminalState,
};
use workspace_leases::WorkspaceLeaseState;

use crate::{
    DesktopServiceError,
    checkpoint::{CheckpointHooks, CheckpointService},
};

pub const NATIVE_DIRECTORY_PICKER_START_PATH: &str = "iface:nativeDirectoryPickerStartPath";
pub const INTERNAL_DESKTOP_OPERATIONS: [&str; 1] = [NATIVE_DIRECTORY_PICKER_START_PATH];

/// The exact 0.1.169 operations implemented by this service slice.
pub const COVERED_OPERATIONS: [&str; 29] = [
    CHANNELS.background_control,
    CHANNELS.customization_control,
    CHANNELS.agent_control,
    CHANNELS.harness_control,
    CHANNELS.recovery_control,
    CHANNELS.get_appearance,
    CHANNELS.set_appearance,
    CHANNELS.get_acp_profiles,
    CHANNELS.set_acp_profiles,
    CHANNELS.get_terminal_workspaces,
    CHANNELS.set_terminal_workspaces,
    CHANNELS.resolve_moved_terminal_folder,
    CHANNELS.get_default_directory,
    CHANNELS.pick_directory,
    CHANNELS.create_project,
    CHANNELS.read_file,
    CHANNELS.read_file_snapshot,
    CHANNELS.save_file,
    CHANNELS.set_workspace_open,
    CHANNELS.list_dir,
    CHANNELS.list_project_files,
    CHANNELS.search_project,
    CHANNELS.list_customization,
    CHANNELS.read_repository_review,
    CHANNELS.discover_review_checks,
    CHANNELS.run_review_checks,
    CHANNELS.create_review_checkpoint,
    CHANNELS.list_review_checkpoints,
    CHANNELS.restore_review_checkpoint,
];

pub struct DesktopServiceConfig {
    pub appearance_file: PathBuf,
    pub acp_profiles_file: PathBuf,
    pub terminal_workspaces_file: PathBuf,
    pub home_directory: PathBuf,
    pub customization_user_root: Option<PathBuf>,
    pub review_checkpoint_store_path: Option<PathBuf>,
}

#[derive(Default)]
pub struct DesktopServiceReviewPorts {
    pub git_runner: Option<Arc<dyn ReviewGitRunner>>,
    pub check_runner: Option<Arc<dyn ReviewCheckRunner>>,
    pub checkpoint_hooks: Option<Arc<dyn CheckpointHooks>>,
}

struct CachedAuthoritySnapshot {
    generation: u64,
    session_roots: Vec<PathBuf>,
    authority: Arc<WorkspaceAuthority>,
}

/// Long-lived, transport-neutral implementation of the settings/workspace/
/// Review desktop slice. All mutable stores have one in-process writer.
pub struct DesktopService {
    prompt_recovery: Mutex<Option<interface_settings::PromptRecoveryStore>>,
    output_reviews: Mutex<Option<interface_settings::OutputReviewStore>>,
    harness: Mutex<Option<interface_settings::HarnessStore>>,
    agents: Mutex<Option<interface_settings::AgentStore>>,
    agent_decisions: Mutex<Option<interface_settings::AgentDecisionLog>>,
    pending_agent_approvals: Mutex<Vec<interface_contracts::agents::AgentPendingApproval>>,
    agents_root: Mutex<Option<PathBuf>>,
    /// Where New project creates (`projects-folder.v1.json`); `None` when the
    /// file could not be opened, which leaves `~/Developer/Projects`.
    projects_folder: Mutex<Option<interface_settings::ProjectsFolderStore>>,
    /// The owner's folder trust answers (`trusted-folders.v1.json`) and the
    /// decision built on them.
    folder_trust: folder_trust::FolderTrustParts,
    background: Mutex<Option<interface_settings::BackgroundStore>>,
    appearance: Mutex<RecoverableAppearanceStore>,
    custom_themes: interface_settings::CustomThemeLibrary,
    acp_profiles: Mutex<RecoverableAcpProfileStore>,
    active_acp_profiles: Vec<interface_contracts::ipc::AcpProfile>,
    terminal_state: Mutex<TerminalState>,
    terminal_workspaces_file: PathBuf,
    terminal_reopen_authority_file: PathBuf,
    home_directory: PathBuf,
    default_directory: PathBuf,
    customization_user_root: Option<PathBuf>,
    customization_roots: Mutex<interface_workspace::CustomizationRoots>,
    authority: WorkspaceAuthority,
    authority_generation: AtomicU64,
    authority_snapshot: Mutex<Option<CachedAuthoritySnapshot>>,
    workspace_leases: Mutex<WorkspaceLeaseState>,
    checkpoints: CheckpointService,
    review: DesktopServiceReviewPorts,
}

#[must_use]
pub fn agent_now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

impl DesktopService {
    /// # Errors
    /// Refuses unsafe phone identity storage or another writer.
    pub fn open_phone_connection(
        &self,
    ) -> Result<interface_settings::PhoneConnectionStore, SettingsError> {
        interface_settings::PhoneConnectionStore::open(
            &self
                .terminal_workspaces_file
                .with_file_name("phone-connection.v1.json"),
        )
    }
    /// # Errors
    /// Refuses unsafe trust storage or another writer.
    pub fn open_remote_trust(&self) -> Result<interface_settings::RemoteTrustStore, SettingsError> {
        interface_settings::RemoteTrustStore::open(
            &self
                .terminal_workspaces_file
                .with_file_name("remote-trust.v1.json"),
        )
    }

    /// Private display metadata is separate from provider reopen authority.
    #[must_use]
    pub fn terminal_titles_file(&self) -> PathBuf {
        self.terminal_workspaces_file
            .with_file_name("terminal-titles.v1.json")
    }

    /// Opens the same private stores and resolves the same first-run directory
    /// as Interface 0.1.169. A malformed ACP roster is ignored for this startup
    /// (matching Electron), while the file remains available for replacement.
    /// An unreadable or unsafe appearance file starts with the system theme
    /// and refuses later saves rather than failing host boot.
    ///
    /// # Errors
    ///
    /// Refuses unsafe private-store paths or an unavailable home directory.
    #[allow(clippy::too_many_lines)]
    pub fn open(
        config: DesktopServiceConfig,
        review: DesktopServiceReviewPorts,
    ) -> Result<Self, DesktopServiceError> {
        let default_directory = resolve_default_directory(&config.home_directory)
            .map_err(|error| map_startup_settings_error(&error))?;
        let terminal_reopen_authority_file =
            terminal_reopen_authority_path(&config.terminal_workspaces_file)
                .map_err(|error| map_startup_settings_error(&error))?;

        let appearance = match AppearanceStore::open(&config.appearance_file)
            .and_then(|mut store| store.load().map(|_| store))
        {
            Ok(store) => RecoverableAppearanceStore::Ready(store),
            Err(error) => {
                eprintln!(
                    "[desktop] starting with the system theme; appearance.json unreadable: {error}"
                );
                RecoverableAppearanceStore::Unavailable(config.appearance_file.clone())
            }
        };

        // main.ts catches both an unsafe target and a broken roster, starts
        // with built-ins, and keeps the file untouched so Settings can replace
        // it deliberately. A broken roster keeps its store writable.
        let (acp_profiles, active_acp_profiles) =
            match AcpProfileStore::open(&config.acp_profiles_file) {
                Ok(mut store) => {
                    let active = store
                        .load()
                        .inspect_err(|error| {
                            eprintln!("[desktop] ACP roster unreadable, using built-ins: {error}");
                        })
                        .unwrap_or_default();
                    (RecoverableAcpProfileStore::Ready(store), active)
                }
                Err(error) => {
                    eprintln!("[desktop] ACP profile roster unavailable at startup: {error}");
                    (
                        RecoverableAcpProfileStore::Unavailable(config.acp_profiles_file.clone()),
                        Vec::new(),
                    )
                }
            };

        // A saved terminal roster is convenience state: an unreadable or
        // malformed file starts empty rather than refusing to boot, exactly as
        // the ACP roster does.
        let (terminal_workspaces, saved_terminal_workspaces) =
            match TerminalWorkspaceStore::open(&config.terminal_workspaces_file)
                .and_then(|mut store| store.load().map(|saved| (store, saved)))
            {
                Ok((store, saved)) => {
                    (RecoverableTerminalWorkspaceStore::Ready(store), Some(saved))
                }
                Err(error) => {
                    eprintln!(
                        "[desktop] saved terminal workspaces unavailable at startup: {error}"
                    );
                    (
                        RecoverableTerminalWorkspaceStore::Unavailable(
                            config.terminal_workspaces_file.clone(),
                        ),
                        None,
                    )
                }
            };
        // A malformed/unreadable renderer roster proves neither membership nor
        // absence. Keep the host sidecar untouched and refuse cold authority
        // for this process; a later explicit roster save may repair the roster,
        // but cannot manufacture host proof.
        let terminal_reopen_authorities = saved_terminal_workspaces.as_ref().map_or_else(
            || RecoverableTerminalReopenAuthorityStore::Unavailable,
            |saved| {
                let opened = TerminalReopenAuthorityStore::open(&terminal_reopen_authority_file)
                    .and_then(|mut store| {
                        store.load()?;
                        if !store.is_initialized() {
                            let pending = saved
                                .iter()
                                .flat_map(|workspace| workspace.terminals.iter().cloned())
                                .collect::<Vec<_>>();
                            store.initialize_pending_migration(&pending)?;
                        }
                        store.narrow_to_saved(saved)?;
                        Ok(store)
                    });
                match opened {
                    Ok(store) => RecoverableTerminalReopenAuthorityStore::Ready(store),
                    Err(error) => {
                        eprintln!(
                            "[desktop] saved panes cannot cold-reopen this launch; reopen authority unavailable: {error}"
                        );
                        RecoverableTerminalReopenAuthorityStore::Unavailable
                    }
                }
            },
        );

        let checkpoints = CheckpointService::new(
            config.review_checkpoint_store_path,
            review.checkpoint_hooks.clone(),
        );
        let (agents, agent_decisions) =
            agent_control::open_agents(&config.terminal_workspaces_file);
        let harness = config.terminal_workspaces_file.parent().and_then(|root| {
            interface_settings::HarnessStore::open(&root.join("harness.v1.json"))
                .inspect_err(|error| eprintln!("[desktop] harness state unavailable: {error}"))
                .ok()
        });
        let projects_folder = config.terminal_workspaces_file.parent().and_then(|root| {
            interface_settings::ProjectsFolderStore::open(&root.join("projects-folder.v1.json"))
                .inspect(|store| {
                    if store.quarantined() {
                        eprintln!("[desktop] projects-folder.v1.json was unreadable; set aside");
                    }
                })
                .inspect_err(|error| {
                    eprintln!("[desktop] Projects folder setting unavailable: {error}");
                })
                .ok()
        });
        let folder_trust =
            folder_trust::FolderTrustParts::open(config.terminal_workspaces_file.parent());
        let background = interface_settings::BackgroundStore::open(
            &config
                .terminal_workspaces_file
                .with_file_name("background.v1.json"),
        )
        .ok();
        let mut customization_roots =
            interface_workspace::CustomizationRoots::for_home(&config.home_directory);
        if let Some(root) = &config.customization_user_root {
            customization_roots.claude.clone_from(root);
        }
        let output_reviews = config.terminal_workspaces_file.parent().and_then(|root| {
            interface_settings::OutputReviewStore::open(
                &root.join(interface_settings::OUTPUT_REVIEWS_FILE_NAME),
            )
            .inspect_err(|error| eprintln!("[desktop] output reviews unavailable: {error}"))
            .ok()
        });
        let prompt_recovery = config.terminal_workspaces_file.parent().and_then(|root| {
            interface_settings::PromptRecoveryStore::open(&root.join("prompt-recovery.v1.json"))
                .inspect_err(|error| eprintln!("[desktop] prompt recovery unavailable: {error}"))
                .ok()
        });
        Ok(Self {
            customization_roots: Mutex::new(customization_roots),
            background: Mutex::new(background),
            prompt_recovery: Mutex::new(prompt_recovery),
            output_reviews: Mutex::new(output_reviews),
            harness: Mutex::new(harness),
            agents: Mutex::new(agents),
            agent_decisions: Mutex::new(agent_decisions),
            pending_agent_approvals: Mutex::new(Vec::new()),
            agents_root: Mutex::new(None),
            projects_folder: Mutex::new(projects_folder),
            folder_trust,
            appearance: Mutex::new(appearance),
            custom_themes: interface_settings::CustomThemeLibrary::new(
                interface_settings::custom_themes_directory(&config.home_directory),
            ),
            acp_profiles: Mutex::new(acp_profiles),
            active_acp_profiles,
            terminal_state: Mutex::new(TerminalState {
                workspaces: terminal_workspaces,
                authorities: terminal_reopen_authorities,
            }),
            terminal_workspaces_file: config.terminal_workspaces_file,
            terminal_reopen_authority_file,
            home_directory: config.home_directory,
            default_directory,
            customization_user_root: config.customization_user_root,
            authority: WorkspaceAuthority::new(),
            authority_generation: AtomicU64::new(0),
            authority_snapshot: Mutex::new(None),
            workspace_leases: Mutex::new(WorkspaceLeaseState::default()),
            checkpoints,
            review,
        })
    }

    /// Grants a durable, host-approved workspace root. The wire's default-
    /// directory and picker operations call this themselves. Current session
    /// roots are supplied separately per operation so archive/forget can narrow
    /// authority without revoking explicit user grants.
    ///
    /// # Errors
    ///
    /// Refuses missing, relative, linked, or unstable directories.
    pub fn grant_workspace_root(&self, root: &Path) -> Result<PathBuf, DesktopServiceError> {
        let canonical = canonical_grant_candidate(root).map_err(|()| {
            map_editor_error(
                WorkspaceEditorError::PathChanged,
                &root.to_string_lossy(),
                "open",
            )
        })?;
        let granted = self
            .mutate_authority(|authority| authority.grant_root(&canonical))
            .map_err(|error| map_editor_error(error, &root.to_string_lossy(), "open"))?;
        Ok(granted)
    }

    pub fn revoke_workspace_root(&self, root: &Path) {
        self.mutate_authority(|authority| authority.revoke_root(root));
    }

    /// Every mutation of the shared authority goes through here. The cached
    /// snapshot is only valid while the generation moves with the roots, and
    /// `clear()` is one call away from being added without that bump.
    fn mutate_authority<T>(&self, mutate: impl FnOnce(&WorkspaceAuthority) -> T) -> T {
        let mutated = mutate(&self.authority);
        self.authority_generation.fetch_add(1, Ordering::AcqRel);
        mutated
    }

    #[must_use]
    pub fn workspace_roots(&self) -> Vec<PathBuf> {
        self.authority.roots()
    }

    /// Known local agent artifact directories under the owner's home directory
    /// (such as Oh My Pi session artifacts and Antigravity brain artifacts)
    /// that read-only output preview may inspect when an agent links to a
    /// generated file outside the project workspace.
    #[must_use]
    pub fn agent_artifact_roots(&self) -> Vec<PathBuf> {
        [".omp/agent/sessions", ".gemini/antigravity-cli/brain"]
            .into_iter()
            .filter_map(|rel| std::fs::canonicalize(self.home_directory.join(rel)).ok())
            .collect()
    }

    /// Exact roots that may own a terminal process. The host-resolved default
    /// directory is always eligible, even before the renderer asks to display
    /// it; explicit picker/editor grants remain additive. This keeps terminal
    /// bootstrap authority in Rust without broadening ordinary workspace
    /// editor authority or trusting a renderer-local remembered path.
    #[must_use]
    pub fn terminal_workspace_roots(&self) -> Vec<PathBuf> {
        let mut roots = BTreeSet::from([self.default_directory.clone()]);
        roots.extend(self.workspace_roots());
        roots.into_iter().collect()
    }

    /// Dispatches one frozen desktop operation with no session roots. Payloads
    /// are strict camel-case named objects; only zero-argument calls accept
    /// `null` or `{}`. The work is synchronous filesystem and subprocess work,
    /// so async callers place it on a bounded blocking executor.
    ///
    /// # Errors
    ///
    /// Preserves the exact errors returned by
    /// [`Self::dispatch_blocking_with_session_roots`].
    pub fn dispatch_blocking(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        self.dispatch_blocking_with_session_roots(operation, payload, &[])
    }

    /// Synchronous host entrypoint with a per-request snapshot of current
    /// session roots. These roots are intentionally not persisted into the
    /// durable picker/default grant set.
    ///
    /// # Errors
    ///
    /// Returns a stable safe error for invalid payloads, refused authority,
    /// persistence, or an operation outside this parity slice.
    pub fn dispatch_blocking_with_session_roots(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        if operation == CHANNELS.recovery_control {
            return self.recovery_command(payload);
        }
        if operation == CHANNELS.harness_control {
            return self.harness_command(payload, session_roots);
        }
        if operation == CHANNELS.agent_control {
            return self.agent_control(payload);
        }

        if operation == CHANNELS.background_control {
            return self.background_control(operation, payload);
        }
        if operation == CHANNELS.get_appearance {
            return self.get_appearance(operation, payload);
        }
        if operation == CHANNELS.set_appearance {
            return self.set_appearance(operation, payload);
        }
        if operation == CHANNELS.get_acp_profiles {
            return self.get_acp_profiles(operation, &payload);
        }
        if operation == CHANNELS.set_acp_profiles {
            return self.set_acp_profiles(operation, payload);
        }
        if operation == CHANNELS.get_terminal_workspaces {
            return self.get_terminal_workspaces(operation, &payload);
        }
        if operation == CHANNELS.set_terminal_workspaces {
            return self.set_terminal_workspaces(operation, payload);
        }
        if operation == CHANNELS.resolve_moved_terminal_folder {
            return self.resolve_moved_terminal_folder(operation, payload);
        }
        if operation == CHANNELS.get_default_directory {
            return self.get_default_directory(operation, &payload);
        }
        if operation == CHANNELS.create_project {
            return self.create_project(operation, payload);
        }
        if operation == CHANNELS.pick_directory {
            return self.pick_directory(operation, payload);
        }
        if operation == NATIVE_DIRECTORY_PICKER_START_PATH {
            return self.native_directory_picker_start_path(operation, payload);
        }
        if operation == CHANNELS.read_file {
            return self.read_file(operation, payload, session_roots);
        }
        if operation == CHANNELS.read_file_snapshot {
            return self.read_file_snapshot(operation, payload, session_roots);
        }
        if operation == CHANNELS.save_file {
            return self.save_file(operation, payload, session_roots);
        }
        if operation == CHANNELS.set_workspace_open {
            return self.set_workspace_open(operation, payload);
        }
        if operation == CHANNELS.list_dir {
            return self.list_dir(operation, payload, session_roots);
        }
        if operation == CHANNELS.list_project_files {
            return self.list_project_files(operation, payload, session_roots);
        }
        if operation == CHANNELS.search_project {
            return self.search_project(operation, payload, session_roots);
        }
        if operation == CHANNELS.customization_control {
            return self.customization_control(operation, payload, session_roots);
        }
        if operation == CHANNELS.list_customization {
            return self.list_customization(operation, payload, session_roots);
        }
        if operation == CHANNELS.read_repository_review {
            return self.read_repository_review(operation, payload, session_roots);
        }
        if operation == CHANNELS.discover_review_checks {
            return self.discover_review_checks(operation, payload, session_roots);
        }
        if operation == CHANNELS.run_review_checks {
            return self.run_review_checks(operation, payload, session_roots);
        }
        if operation == CHANNELS.create_review_checkpoint {
            return self.create_review_checkpoint(operation, payload, session_roots);
        }
        if operation == CHANNELS.list_review_checkpoints {
            return self.list_review_checkpoints(operation, payload, session_roots);
        }
        if operation == CHANNELS.restore_review_checkpoint {
            return self.restore_review_checkpoint(operation, payload, session_roots);
        }

        Err(DesktopServiceError::new(
            "operation-not-owned",
            "That operation belongs to another Rust service slice.",
            false,
        ))
    }

    fn workspace_authority_snapshot(&self, session_roots: &[PathBuf]) -> Arc<WorkspaceAuthority> {
        let generation = self.authority_generation.load(Ordering::Acquire);
        if let Ok(cache) = self.authority_snapshot.lock()
            && let Some(cached) = cache.as_ref()
            && cached.generation == generation
            && cached.session_roots == session_roots
        {
            return Arc::clone(&cached.authority);
        }
        let built = Arc::new(self.build_workspace_authority(session_roots));
        if let Ok(mut cache) = self.authority_snapshot.lock() {
            *cache = Some(CachedAuthoritySnapshot {
                generation,
                session_roots: session_roots.to_vec(),
                authority: Arc::clone(&built),
            });
        }
        built
    }

    fn build_workspace_authority(&self, session_roots: &[PathBuf]) -> WorkspaceAuthority {
        let authority = WorkspaceAuthority::new();
        for root in self.authority.roots().iter().chain(session_roots) {
            let Ok(canonical) = canonical_grant_candidate(root) else {
                continue;
            };
            let _ = authority.grant_root(&canonical);
        }
        authority
    }

    fn canonical_authorized_path(
        authority: &WorkspaceAuthority,
        target: &str,
    ) -> Result<PathBuf, DesktopServiceError> {
        Self::canonical_authorized_path_in(&authority.roots(), target)
    }

    fn canonical_authorized_path_in(
        roots: &[PathBuf],
        target: &str,
    ) -> Result<PathBuf, DesktopServiceError> {
        let requested = Path::new(target);
        let canonical = std::fs::canonicalize(requested)
            .map_err(|_| map_editor_error(WorkspaceEditorError::PathChanged, target, "open"))?;
        if roots
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(root))
        {
            return Ok(canonical);
        }
        Err(map_editor_error(
            WorkspaceEditorError::OutsideAuthorizedRoot,
            target,
            "open",
        ))
    }

    fn exact_authorized_root(
        authority: &WorkspaceAuthority,
        target: &str,
        operation: &str,
    ) -> Result<PathBuf, DesktopServiceError> {
        let roots = authority.roots();
        let canonical = Self::canonical_authorized_path_in(&roots, target)?;
        if roots.contains(&canonical) {
            Ok(canonical)
        } else {
            Err(DesktopServiceError::refused(
                &format!("Could not {operation}."),
                "This accepts an exact current session or picked workspace root, not an arbitrary subfolder.",
            ))
        }
    }
}

pub(crate) fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    let source = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    Some(lexically_normalized(&source))
}

fn canonical_grant_candidate(root: &Path) -> Result<PathBuf, ()> {
    let absolute = std::path::absolute(root).map_err(|_| ())?;
    let before = std::fs::symlink_metadata(&absolute).map_err(|_| ())?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err(());
    }
    let canonical = std::fs::canonicalize(&absolute).map_err(|_| ())?;
    let after = std::fs::symlink_metadata(&absolute).map_err(|_| ())?;
    let canonical_metadata = std::fs::symlink_metadata(&canonical).map_err(|_| ())?;
    if !after.is_dir()
        || after.file_type().is_symlink()
        || !canonical_metadata.is_dir()
        || canonical_metadata.file_type().is_symlink()
        || !same_directory_node(&before, &after)
        || !same_directory_node(&after, &canonical_metadata)
    {
        return Err(());
    }
    Ok(canonical)
}

fn same_directory_node(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn encode<T: Serialize>(operation: &str, result: T) -> Result<Value, DesktopServiceError> {
    serde_json::to_value(result).map_err(|_| {
        DesktopServiceError::new(
            "desktop-response-encode-failed",
            format!("Interface could not encode the {operation} response."),
            false,
        )
    })
}

pub(crate) fn display_path(path: &Path) -> Result<String, DesktopServiceError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        DesktopServiceError::new(
            "unsupported-path-encoding",
            "Interface could not represent that local path in the desktop protocol.",
            false,
        )
    })
}

fn shorten_path(target: &str) -> String {
    const MAX_PATH_CHARACTERS: usize = 48;
    if target.chars().count() <= MAX_PATH_CHARACTERS {
        return target.to_owned();
    }
    let parts = target
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some(base) = parts.last() else {
        return target.to_owned();
    };
    let tail = parts
        .iter()
        .rev()
        .take(2)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if tail.chars().count() <= MAX_PATH_CHARACTERS {
        format!("…/{tail}")
    } else {
        format!("…/{base}")
    }
}

fn map_startup_settings_error(error: &SettingsError) -> DesktopServiceError {
    if matches!(error, SettingsError::InvalidHome) {
        return DesktopServiceError::new(
            "default-directory-unavailable",
            "Interface could not resolve the user's home directory.",
            false,
        );
    }
    DesktopServiceError::new(
        "desktop-state-unavailable",
        "Interface could not safely open its private desktop state.",
        false,
    )
}

fn map_settings_error(error: SettingsError) -> DesktopServiceError {
    match error {
        SettingsError::InvalidAcp(detail) => DesktopServiceError::new(
            "invalid-acp-profiles",
            format!("Invalid ACP profile configuration: {detail}"),
            false,
        ),
        other => DesktopServiceError::new("desktop-state-write-failed", other.to_string(), false),
    }
}

fn map_editor_error(
    error: WorkspaceEditorError,
    target: &str,
    operation: &str,
) -> DesktopServiceError {
    let shown = shorten_path(target);
    match error {
        WorkspaceEditorError::OutsideAuthorizedRoot => DesktopServiceError::refused(
            "Outside your open folders.",
            &format!(
                "Interface only opens files inside a folder you have opened. {shown} is outside every open session directory — pick it with Browse… to add it."
            ),
        ),
        WorkspaceEditorError::PathChanged => DesktopServiceError::refused(
            "Folder changed on disk.",
            &format!(
                "{shown} or one of its folders changed while Interface was preparing to {operation} it. Interface stopped before reading or overwriting the replacement; retry from the refreshed file tree."
            ),
        ),
        WorkspaceEditorError::FileTooLarge => DesktopServiceError::refused(
            "File is too large.",
            &format!(
                "{shown} is larger than 5 MB. Interface left it unchanged; use a tool designed for large files."
            ),
        ),
        WorkspaceEditorError::DirectoryTooLarge => DesktopServiceError::refused(
            "Folder is too large.",
            &format!(
                "{shown} contains more than 5,000 entries. Interface did not return a partial, misleading file tree."
            ),
        ),
        WorkspaceEditorError::IsDirectory => DesktopServiceError::refused(
            "That is a folder.",
            &format!("{shown} is a folder, not a file, so there is nothing to open in the editor."),
        ),
        WorkspaceEditorError::ProtectedPath => DesktopServiceError::refused(
            "Protected file.",
            &format!(
                "{shown} looks like a credential or secret store. Interface did not read or change it; use a dedicated credential tool instead."
            ),
        ),
        WorkspaceEditorError::BinaryFile => DesktopServiceError::refused(
            "Binary file not opened.",
            &format!(
                "{shown} is a binary or container format. The text editor did not decode or change it."
            ),
        ),
        WorkspaceEditorError::InvalidText => DesktopServiceError::refused(
            "Binary file not opened.",
            &format!("{shown} is not valid UTF-8 text. Interface did not decode or change it."),
        ),
        WorkspaceEditorError::InvalidSave => DesktopServiceError::refused(
            "Could not save that.",
            "The editor sent an invalid save request. The file was not changed.",
        ),
        WorkspaceEditorError::StaleRevision => DesktopServiceError::refused(
            "File changed on disk.",
            &format!(
                "{shown} was changed by an agent, terminal, or another editor after this tab opened. Nothing was overwritten; reload it or copy your draft before trying again."
            ),
        ),
        WorkspaceEditorError::NotWritable => DesktopServiceError::refused(
            "Permission denied.",
            &format!(
                "macOS is blocking Interface from changing {shown}. Nothing was overwritten; check the file permissions and try again."
            ),
        ),
        WorkspaceEditorError::WriteFailed => DesktopServiceError::refused(
            "Could not save that.",
            &format!(
                "Interface could not safely replace {shown}. The original file was left unchanged."
            ),
        ),
        WorkspaceEditorError::StillDownloading => DesktopServiceError::refused(
            "File is still downloading.",
            &format!(
                "{shown} is still downloading from iCloud. Interface stopped waiting; try again once it has downloaded."
            ),
        ),
        WorkspaceEditorError::WriteUncertain => DesktopServiceError::refused(
            "Save outcome is uncertain.",
            &format!(
                "Interface may have replaced {shown}, but could not prove durability. Reload the file before retrying."
            ),
        ).outcome_unknown(),
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;

mod output_reviews;

mod prompt_recovery;
