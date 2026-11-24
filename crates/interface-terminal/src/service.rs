mod provider_recovery;
mod servers;
use provider_recovery::{
    Binding, BindingTarget, bind_provider_session, handle_provider_session_wake,
    has_resumable_provider_binding, manual_provider_profile, mark_terminal_closing,
    provider_session_capture_kind, provider_session_capture_profile, public_provider_session_state,
    refresh_final_provider_session_binding, seal_nested_provider_for_terminal,
};

mod callers;
mod handoff;
mod output;
mod polling;
mod provider_session;
mod shell_handoff;
mod startup_download;
mod teardown;
mod ticker;
mod titles;
mod workspace_sleep;
use handoff::{default_handoff_directory, validated_pass_off};
use output::OutputBuffer;
pub use output::{TerminalOutputChunk, TerminalOutputRead, TerminalOutputReader};
use provider_session::UnavailableProviderSessionLookup;
pub use provider_session::{
    TerminalLifecycleSink, TerminalProviderSession, TerminalProviderSessionCaptureOwner,
    TerminalProviderSessionCaptureProcess, TerminalProviderSessionLookup,
    TerminalProviderSessionPersistence, TerminalProviderSessionTransition,
};
use teardown::{
    abort_prepared_terminal, release_terminal_entries, run_output_reader, run_process_waiter,
};
pub use titles::TerminalTitlePersistence;
use titles::apply_auto_title;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::Utc;
use interface_common::sync::lock;
use interface_common::uuid::{canonical_uuid, canonical_uuid_v4};
use interface_contracts::canonical_timestamp;
use interface_contracts::engine::SessionKey;
use interface_contracts::terminal::{
    CreateTerminalRequest, DismissTerminalRequest, DismissTerminalResult, RenameTerminalRequest,
    ResizeTerminalRequest, TERMINAL_LAUNCH_REQUEST_VERSION, TerminalGeneration,
    TerminalGenerationRequest, TerminalHandoffKind, TerminalHandoffV1, TerminalId,
    TerminalLaunchProfile, TerminalLaunchProfileId, TerminalLaunchRequestV1,
    TerminalLaunchSessionV1, TerminalLifecycleEvent, TerminalSize, TerminalSnapshot, TerminalState,
    TerminalSummary,
};

use crate::embedded::{
    SpawnedTerminal, TerminalInput, TerminalProcessControl, TerminalProcessIdentity,
    TerminalPtyBackend, TerminalSpawnSpec,
};
use crate::error::{TerminalBackendError, TerminalError};
use crate::launch::{ResolvedTerminalLaunch, TerminalLaunchCatalog, bind_handoff_prompt};
use crate::osc_title::OscTitleScanner;
use crate::proc_ancestry::{owned_ancestor, process_parents};
use crate::proc_cwd::live_process_cwd;
use crate::safety::{
    ApprovedTerminalRoots, AuthorizedDirectory, assert_directory, root_error, snapshot_directory,
};
use crate::startup_files::{StartupFetch, SystemTerminalStartupFiles, TerminalStartupFiles};

const MIN_COLUMNS: u16 = 2;
const MAX_COLUMNS: u16 = 1_000;
const MIN_ROWS: u16 = 1;
const MAX_ROWS: u16 = 500;
const MAX_TITLE_CHARS: usize = 120;
const MAX_INPUT_BYTES: usize = 64 * 1_024;
const PROCESS_TEARDOWN_RETRY_DELAY: Duration = Duration::from_millis(50);
/// A retry that keeps failing (a nested provider outliving its shell, a group
/// that will not die) backs off to this, instead of a `ps` spawn every 50 ms
/// for as long as it lives.
const PROCESS_TEARDOWN_RETRY_MAX: Duration = Duration::from_millis(500);
const UNCLAIMED_OUTPUT_GRACE: Duration = Duration::from_millis(500);
const PROVIDER_SESSION_STARTUP_POLL_DELAY: Duration = Duration::from_millis(100);
const PROVIDER_SESSION_STARTUP_POLLS: usize = 20;
fn now() -> String {
    canonical_timestamp(Utc::now())
}

/// Resource identity passed to a host-provided workspace/worktree lease
/// provider. The lease itself remains owned by the terminal until its process
/// exits, which prevents cleanup while a shell still has that cwd.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalLeaseRequest {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub session_key: Option<SessionKey>,
}

/// An opaque lease guard. Dropping the guard releases the host's workspace
/// protection.
pub trait TerminalWorkspaceLease: Send {}

/// Hook used to integrate terminal lifetimes with a worktree manager without
/// coupling this crate to any one workspace store.
pub trait TerminalWorkspaceLeaseProvider: Send + Sync {
    /// # Errors
    ///
    /// Returns a backend error when the host cannot protect the requested
    /// workspace for this terminal generation.
    fn acquire(
        &self,
        request: &TerminalLeaseRequest,
    ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError>;
}

/// Runtime limits for a terminal service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalServiceConfig {
    pub max_terminals: usize,
    pub output_capacity_bytes: usize,
    pub output_read_chunk_bytes: usize,
}

impl Default for TerminalServiceConfig {
    fn default() -> Self {
        Self {
            // ponytail: a resource guard, not a product limit; the renderer
            // caps panes per workspace at 64 and tabs per pane at 32.
            max_terminals: 256,
            output_capacity_bytes: 1_024 * 1_024,
            output_read_chunk_bytes: 16 * 1_024,
        }
    }
}

impl TerminalServiceConfig {
    fn validate(self) -> Result<Self, TerminalError> {
        if self.max_terminals == 0
            || self.output_capacity_bytes == 0
            || self.output_read_chunk_bytes == 0
            || self.output_read_chunk_bytes > self.output_capacity_bytes
        {
            return Err(TerminalError::InvalidConfiguration);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CreateFingerprint {
    cwd: PathBuf,
    size: TerminalSize,
    title: String,
    session_key: Option<SessionKey>,
    launch: Option<interface_contracts::terminal::TerminalLaunchRequestV1>,
    launch_session: Option<TerminalLaunchSessionV1>,
}

fn preflight_create(
    state: &ServiceState,
    id: TerminalId,
    fingerprint: &CreateFingerprint,
    max_terminals: usize,
) -> Result<Option<TerminalSummary>, TerminalError> {
    if state.retired.contains(&id) {
        return Err(TerminalError::RetiredId(id));
    }
    if let Some(entry) = state.entries.get(&id) {
        if entry.fingerprint == *fingerprint {
            return Ok(Some(entry.summary.clone()));
        }
        return Err(TerminalError::CreateConflict(id));
    }
    if state.entries.len() >= max_terminals {
        return Err(TerminalError::LimitReached(max_terminals));
    }
    Ok(None)
}

struct TerminalResources {
    input: Arc<dyn TerminalInput>,
    control: Arc<dyn TerminalProcessControl>,
    output: Arc<OutputBuffer>,
    lease: Option<Box<dyn TerminalWorkspaceLease>>,
}

#[derive(Default)]
struct ProviderSessionWake {
    pending: Mutex<bool>,
    changed: Condvar,
}

impl ProviderSessionWake {
    fn signal(&self) {
        *lock(&self.pending) = true;
        self.changed.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let mut pending = lock(&self.pending);
        if !*pending {
            pending = self
                .changed
                .wait_timeout(pending, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        std::mem::take(&mut *pending)
    }
}

/// Whether a generation's clean exit is still owed a shell (protocol 50).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellHandoff {
    Owed,
    /// Someone asked this generation to close (`close`: a tab closed, a
    /// workspace deleted, a voice or phone close, quitting). Unlike
    /// `closing`, a natural exit never sets it, so a clean exit that follows
    /// is not the agent finishing by itself and is never handed a shell.
    CloseRequested,
    /// Its clean exit was already handed a shell.
    HandedOff,
}

#[allow(clippy::struct_excessive_bools)] // Independent lifecycle fences model one terminal state machine.
struct TerminalEntry {
    power: Arc<Mutex<workspace_sleep::WorkspacePower>>,
    summary: TerminalSummary,
    fingerprint: CreateFingerprint,
    /// The approved root that owns this pane. Equal to `cwd` for every pane
    /// except a pass-off that started in a subdirectory of its root, and it is
    /// the root — never the subdirectory — that a reopen re-authorizes and
    /// that the workspace-cleanup lease protects.
    root: PathBuf,
    cwd: PathBuf,
    resources: TerminalResources,
    /// True once a human has named this terminal through `rename`. A title the
    /// shell announces over OSC never overwrites one the user typed.
    ///
    /// Deliberately not on `TerminalSummary`: this is host-side policy, the
    /// renderer only draws `title`, and keeping it internal avoids versioning
    /// the terminal wire contract for a field nobody reads.
    title_is_user: bool,
    title_lookup_at: Option<Instant>,
    /// Session whose file-backed task name is current. OSC must not replace it.
    /// This is display policy for Codex, Grok, and Antigravity, not resume authority.
    file_title_session: Option<String>,
    shell_display: Option<ShellDisplay>,
    /// Fresh, generation-scoped correlation for Claude lookup wake-ups. It is
    /// never session authority; a restart rotates it, and the reserved OSC
    /// carrying it never reaches the renderer.
    provider_session_capture_token: Option<String>,
    /// Provider whose private metadata owns this capture. A manually invoked
    /// Claude process begins while the public launch is still Shell, so this
    /// cannot be derived from `summary.launch`.
    provider_session_profile: Option<TerminalLaunchProfileId>,
    provider_session_capture_kind: Option<ProviderSessionCaptureKind>,
    /// Spawn-time identity used to validate the provider's live session
    /// pointer. This is independent of any bytes the provider prints.
    provider_session_process: Option<TerminalProcessIdentity>,
    provider_session_cwd: Option<PathBuf>,
    /// Coalesced wake-up for the rate-limited lookup worker. PTY bytes may set
    /// this bit, but only host-verified provider metadata can mutate session
    /// identity.
    provider_session_wake: Option<Arc<ProviderSessionWake>>,
    /// Set before the reaped-process receipt lookup. It fences any ordinary
    /// lookup that began earlier from applying after the final result.
    provider_session_finalizing: bool,
    /// A second manual Claude process replaced the previous exact foreground
    /// owner without an intervening stable shell state. Keep the last public
    /// binding until the new receipt is proven, but allow even its `startup`
    /// transition to replace that predecessor.
    provider_session_replacing: bool,
    /// Explicit close or PTY-leader exit fences the nested-child observer from
    /// demoting a binding that shutdown still needs to reopen.
    closing: bool,
    /// Whether this generation's clean exit is still owed a shell.
    shell_handoff: ShellHandoff,
    /// This generation's refreshes on the service ticker.
    ticks: ticker::EntryTicks,
    /// The startup-file fetch this generation waits on, until it is done.
    startup_fetch: Option<Arc<StartupFetch>>,
}

impl TerminalEntry {
    /// This generation still holds exactly this capture: the same provider
    /// process under the same capture token.
    fn owns_capture(
        &self,
        generation: TerminalGeneration,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> bool {
        self.summary.generation == generation
            && self.provider_session_process.as_ref() == Some(process)
            && self.provider_session_capture_token.as_deref() == Some(capture_token)
    }

    const fn is_live(&self) -> bool {
        matches!(
            self.summary.state,
            TerminalState::Starting | TerminalState::Running
        )
    }

    fn ensure_running(&self) -> Result<(), TerminalError> {
        if self.is_live() {
            Ok(())
        } else {
            Err(TerminalError::NotRunning(self.summary.id))
        }
    }

    /// This generation's capture belongs to an armed login shell, whose
    /// provider runs as a foreground child rather than the direct process.
    const fn manual_capture(&self) -> bool {
        matches!(
            self.provider_session_capture_kind,
            Some(ProviderSessionCaptureKind::ManualShell { .. })
        )
    }

    /// Forgets this generation's provider capture. Whether a replacement is
    /// pending (`provider_session_replacing`) is left to the callers that
    /// reset it, exactly as before.
    fn clear_provider_capture(&mut self) {
        self.provider_session_capture_token = None;
        self.provider_session_profile = None;
        self.provider_session_capture_kind = None;
        self.provider_session_process = None;
        self.provider_session_cwd = None;
        self.provider_session_wake = None;
        self.provider_session_finalizing = false;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProviderSessionCaptureKind {
    Direct,
    ManualShell {
        original_launch: Option<TerminalLaunchRequestV1>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManualProviderExit {
    Unchanged,
    RearmedShell,
    ReplacedProvider,
}

struct ShellDisplay {
    launch: Option<TerminalLaunchRequestV1>,
    title: String,
    profile: Option<TerminalLaunchProfileId>,
}

impl ShellDisplay {
    fn new(summary: &TerminalSummary) -> Option<Self> {
        summary
            .launch
            .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell)
            .then(|| Self {
                launch: summary.launch,
                title: summary.title.clone(),
                profile: None,
            })
    }
}

#[derive(Default)]
struct ServiceState {
    revision: u64,
    entries: BTreeMap<TerminalId, TerminalEntry>,
    retired: HashSet<TerminalId>,
    /// Whether the service's one ticker thread is running.
    ticker: bool,
}

struct Shared {
    title_persistence: Arc<dyn TerminalTitlePersistence>,
    codex_names: Mutex<crate::codex_names::CodexNames>,
    grok_names: Mutex<crate::grok_names::GrokNames>,
    antigravity_names: Mutex<crate::antigravity_names::AntigravityNames>,
    state: Mutex<ServiceState>,
    lifecycle_gate: Mutex<()>,
    event_gate: Mutex<()>,
    /// Serializes durable provider-owner changes with live/final receipt
    /// reads. In particular, shutdown cannot observe `process: None` between
    /// a successful nested-owner write and the matching in-memory attach.
    provider_session_gates: ProviderOwnerGates,
    backend: Arc<dyn TerminalPtyBackend>,
    leases: Arc<dyn TerminalWorkspaceLeaseProvider>,
    events: Arc<dyn TerminalLifecycleSink>,
    /// The current catalog. A refresh replaces the whole of it; every launch
    /// takes one snapshot (`launches()`) and uses it from start to spawn, so
    /// a swap mid-launch never mixes two catalogs. Running panes keep the
    /// process they started.
    launches: std::sync::RwLock<Arc<TerminalLaunchCatalog>>,
    provider_sessions: Arc<dyn TerminalProviderSessionLookup>,
    handoff_directory: PathBuf,
    startup_files: Arc<dyn TerminalStartupFiles>,
    /// Protocol 60: the host's provider accounts; `None` runs every launch on
    /// the Default account, exactly as before.
    accounts: std::sync::OnceLock<Arc<dyn crate::launch::TerminalAccounts>>,
    /// One live fetch per cwd, shared by every pane that starts there. Weak:
    /// a fetch lives as long as a pane is waiting on it or a read is in flight.
    startup_fetches: Mutex<BTreeMap<PathBuf, Weak<StartupFetch>>>,
    /// Where the startup-file walk stops, and what `~/` names on the wire.
    home: Option<PathBuf>,
    config: TerminalServiceConfig,
}

impl Shared {
    /// `launches` with the provider account a launch of `launch`'s profile
    /// runs on applied (protocol 60): `account` when named, otherwise the
    /// account `session`'s chat is bound to, otherwise the profile's default
    /// account. The Default account answers `launches` itself.
    pub(crate) fn account_launches(
        &self,
        launches: Arc<TerminalLaunchCatalog>,
        launch: Option<TerminalLaunchRequestV1>,
        account: Option<&str>,
        session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Arc<TerminalLaunchCatalog>, TerminalError> {
        let (Some(accounts), Some(launch)) = (self.accounts.get(), launch) else {
            return Ok(launches);
        };
        match accounts
            .launch(launch.profile_id, account, session)
            .map_err(TerminalError::AccountUnavailable)?
        {
            None => Ok(launches),
            Some(applied) => Ok(Arc::new(launches.for_account(launch.profile_id, &applied)?)),
        }
    }

    /// The catalog a launch uses from start to spawn.
    pub(crate) fn launches(&self) -> Arc<TerminalLaunchCatalog> {
        Arc::clone(
            &self
                .launches
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

// The map only coordinates live operations and holds weak references. Exact
// (terminal, generation) owners serialize with their own close/final checkpoint;
// an unrelated pane never waits behind their provider filesystem/process lookup.
fn provider_session_gate(
    shared: &Shared,
    id: TerminalId,
    generation: TerminalGeneration,
) -> Arc<Mutex<()>> {
    owner_gate(&shared.provider_session_gates, (id, generation.0))
}

type ProviderOwnerGates = Mutex<BTreeMap<(TerminalId, u64), Weak<Mutex<()>>>>;
fn owner_gate(gates: &ProviderOwnerGates, key: (TerminalId, u64)) -> Arc<Mutex<()>> {
    let mut gates = lock(gates);
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(Mutex::new(()));
    gates.insert(key, Arc::downgrade(&gate));
    gate
}

impl Drop for Shared {
    fn drop(&mut self) {
        let (entries, mut revision) = {
            let state = self
                .state
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (mem::take(&mut state.entries), state.revision)
        };
        release_terminal_entries(self, entries, &mut revision);
    }
}

/// A manually invoked provider's independently captured cwd, when it is still
/// a canonical directory inside `root`. A renamed, removed, symlinked, or
/// escaped directory cannot form a safe automatic-reopen tuple, so attaching,
/// sealing and filing all refuse it; its sealed receipt remains for startup
/// recovery instead.
fn validated_manual_provider_cwd(captured: &Path, root: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(captured).ok()?;
    (canonical == *captured
        && canonical.is_dir()
        && (canonical == root || canonical.starts_with(root)))
    .then_some(canonical)
}

/// Long-lived, thread-safe registry and lifecycle owner for integrated PTYs.
///
/// Clones share one registry. Dropping the final clone tears down every live
/// process tree before releasing workspace leases.
#[derive(Clone)]
pub struct TerminalService {
    shared: Arc<Shared>,
}

struct PreparedTerminal {
    spawned: SpawnedTerminal,
    output: Arc<OutputBuffer>,
    lease: Box<dyn TerminalWorkspaceLease>,
}

/// One generation's private provider-session capture: its kind, the provider
/// it is polled as, and the fresh token that correlates its wakes.
struct GenerationCapture {
    kind: ProviderSessionCaptureKind,
    profile_id: TerminalLaunchProfileId,
    token: String,
}

impl GenerationCapture {
    /// A fresh capture when this launch's profile has one.
    fn for_launch(
        launches: &TerminalLaunchCatalog,
        launch: Option<TerminalLaunchRequestV1>,
    ) -> Option<Self> {
        let kind = provider_session_capture_kind(launches, launch)?;
        Some(Self {
            profile_id: provider_session_capture_profile(&kind, launch),
            kind,
            token: TerminalId::random().to_string(),
        })
    }

    /// The durable owner committed before this generation can execute.
    fn owner(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        root: &str,
        cwd: &str,
        prior_launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> TerminalProviderSessionCaptureOwner {
        TerminalProviderSessionCaptureOwner {
            terminal_id: id,
            terminal_generation: generation,
            capture_token: self.token.clone(),
            root: root.to_owned(),
            cwd: cwd.to_owned(),
            prior_launch_session: prior_launch_session.cloned(),
            nested_shell: matches!(self.kind, ProviderSessionCaptureKind::ManualShell { .. }),
        }
    }
}

/// What a create or a restart spawns one generation with. Everything else
/// about the spawn is the same for both.
struct GenerationSpec<'a> {
    id: TerminalId,
    generation: TerminalGeneration,
    directory: &'a AuthorizedDirectory,
    /// `directory`'s root and cwd as the capture owner records them.
    root: &'a str,
    cwd: &'a str,
    size: TerminalSize,
    session_key: Option<SessionKey>,
    launch: ResolvedTerminalLaunch,
    capture: Option<&'a GenerationCapture>,
    /// The binding the new generation starts with, which its capture owner
    /// records as the predecessor of any receipt.
    prior_launch_session: Option<&'a TerminalLaunchSessionV1>,
}

/// A spawned generation and what its entry is built from; everything else
/// the entry holds starts empty or is derived from the summary.
struct NewGeneration {
    summary: TerminalSummary,
    fingerprint: CreateFingerprint,
    directory: AuthorizedDirectory,
    title_is_user: bool,
    file_title_session: Option<String>,
    capture: Option<GenerationCapture>,
    provider_session_process: Option<TerminalProcessIdentity>,
    prepared: PreparedTerminal,
}

/// Everything a [`TerminalService`] is built from.
///
/// The three collaborators have no safe default — a stand-in lease provider
/// would let worktree cleanup run under a live pane — so they are named by
/// every caller. `launches` and `handoff_directory` do have one, which
/// [`TerminalServiceParts::new`] supplies: fill the rest with
/// `..TerminalServiceParts::new(..)` and override only what differs.
pub struct TerminalServiceParts {
    pub title_persistence: Arc<dyn TerminalTitlePersistence>,
    pub config: TerminalServiceConfig,
    pub backend: Arc<dyn TerminalPtyBackend>,
    pub leases: Arc<dyn TerminalWorkspaceLeaseProvider>,
    pub events: Arc<dyn TerminalLifecycleSink>,
    /// Host-owned fixed launch profiles. Defaults to the login shell only.
    pub launches: TerminalLaunchCatalog,
    /// Provider-owned live session lookup. Defaults to an unavailable lookup,
    /// which leaves capture-enabled launches safely id-less.
    pub provider_sessions: Arc<dyn TerminalProviderSessionLookup>,
    /// Where pass-off transcripts land. Defaults to the process temp
    /// directory; tests point it at their own so one create is observable.
    pub handoff_directory: PathBuf,
    /// Where a pane's startup files are fetched from. Defaults to the local
    /// file system; tests stand in one whose downloads they control.
    pub startup_files: Arc<dyn TerminalStartupFiles>,
    /// Protocol 60: the host's provider accounts. `None`, the default, runs
    /// every launch on the Default account.
    pub accounts: Option<Arc<dyn crate::launch::TerminalAccounts>>,
}

impl TerminalServiceParts {
    #[must_use]
    pub fn new(
        config: TerminalServiceConfig,
        backend: Arc<dyn TerminalPtyBackend>,
        leases: Arc<dyn TerminalWorkspaceLeaseProvider>,
        events: Arc<dyn TerminalLifecycleSink>,
    ) -> Self {
        Self {
            title_persistence: Arc::new(titles::EphemeralTitles),
            config,
            backend,
            leases,
            events,
            launches: TerminalLaunchCatalog::shell_only(),
            provider_sessions: Arc::new(UnavailableProviderSessionLookup),
            handoff_directory: default_handoff_directory(),
            startup_files: Arc::new(SystemTerminalStartupFiles),
            accounts: None,
        }
    }
}

impl TerminalService {
    /// Construct a service from its parts.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid service configuration.
    pub fn new(parts: TerminalServiceParts) -> Result<Self, TerminalError> {
        let TerminalServiceParts {
            title_persistence,
            config,
            backend,
            leases,
            events,
            launches,
            provider_sessions,
            handoff_directory,
            startup_files,
            accounts,
        } = parts;
        let config = config.validate()?;
        Ok(Self {
            shared: Arc::new(Shared {
                title_persistence,
                codex_names: Mutex::new(crate::codex_names::CodexNames::default()),
                grok_names: Mutex::new(crate::grok_names::GrokNames::default()),
                antigravity_names: Mutex::new(crate::antigravity_names::AntigravityNames::default()),
                state: Mutex::new(ServiceState::default()),
                lifecycle_gate: Mutex::new(()),
                event_gate: Mutex::new(()),
                provider_session_gates: Mutex::new(BTreeMap::new()),
                backend,
                leases,
                events,
                launches: std::sync::RwLock::new(Arc::new(launches)),
                provider_sessions,
                handoff_directory,
                startup_files,
                accounts: accounts.map_or_else(std::sync::OnceLock::new, |accounts| {
                    let installed = std::sync::OnceLock::new();
                    let _ = installed.set(accounts);
                    installed
                }),
                startup_fetches: Mutex::new(BTreeMap::new()),
                home: std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|home| home.is_absolute()),
                config,
            }),
        })
    }

    /// Installs the host's provider accounts (protocol 60) once, after the
    /// service exists. Later launches apply them; a second call is ignored.
    pub fn install_accounts(&self, accounts: Arc<dyn crate::launch::TerminalAccounts>) {
        let _ = self.shared.accounts.set(accounts);
    }

    /// Live PTYs retain their host-authorized roots after a saved workspace is
    /// reopened. These are process-lifetime grants, not renderer roster paths.
    #[must_use]
    pub fn active_workspace_roots(&self) -> Vec<PathBuf> {
        let state = lock(&self.shared.state);
        state
            .entries
            .values()
            .filter(|entry| entry.summary.state == TerminalState::Running && !entry.closing)
            .map(|entry| entry.root.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn snapshot(&self) -> TerminalSnapshot {
        let state = lock(&self.shared.state);
        TerminalSnapshot {
            revision: state.revision,
            terminals: state
                .entries
                .values()
                .map(|entry| entry.summary.clone())
                .collect(),
        }
    }

    /// Releases every settled terminal entry during explicit host shutdown.
    /// No `Removed` lifecycle is published: saved restart authority must
    /// survive an orderly app exit. Returns `false` if any process is still
    /// starting or running, in which case the caller must finish teardown
    /// before retrying.
    #[must_use]
    pub fn release_settled_for_shutdown(&self) -> bool {
        let _lifecycle = lock(&self.shared.lifecycle_gate);
        let (entries, mut revision) = {
            let mut state = lock(&self.shared.state);
            if state.entries.values().any(TerminalEntry::is_live) {
                return false;
            }
            (mem::take(&mut state.entries), state.revision)
        };
        release_terminal_entries(&self.shared, entries, &mut revision);
        true
    }

    #[must_use]
    pub fn launch_profiles(&self) -> Vec<TerminalLaunchProfile> {
        self.shared.launches().profiles()
    }

    /// The executable each available profile runs now, for a `--version`
    /// read (Settings). Never on the wire: only what it prints is.
    #[must_use]
    pub fn launch_profile_executables(&self) -> Vec<(TerminalLaunchProfileId, PathBuf)> {
        let launches = self.shared.launches();
        launches
            .profiles()
            .into_iter()
            .filter_map(|profile| {
                launches
                    .executable(profile.id)
                    .map(|path| (profile.id, path.to_path_buf()))
            })
            .collect()
    }

    /// Replaces the launch catalog, for a CLI installed (or moved) while
    /// Interface runs. Launches already past their snapshot finish on the
    /// catalog they read; panes already running are untouched.
    pub fn replace_launch_catalog(&self, catalog: TerminalLaunchCatalog) {
        *self
            .shared
            .launches
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(catalog);
    }

    /// Create a caller-identified terminal. Replaying the exact initial
    /// request is idempotent; reusing the ID for different intent fails.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid/unauthorized cwd or size, conflicting IDs,
    /// capacity, lease acquisition, or PTY startup failure.
    pub fn create(
        &self,
        request: CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
    ) -> Result<TerminalSummary, TerminalError> {
        self.create_inner(request, approved, None, None, Ok)
    }

    /// Create with a metadata-only provider-history recovery check. The check
    /// runs behind the create lifecycle gate and only after an exact wire
    /// replay has had a chance to return its existing summary. Mutable or
    /// unavailable provider metadata therefore cannot break create
    /// idempotency, while the pre-check wire binding remains the fingerprint.
    ///
    /// # Errors
    ///
    /// Returns the same validation, authorization, conflict, lease, and spawn
    /// errors as [`Self::create`].
    pub fn create_with_provider_history_check<F>(
        &self,
        request: CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
        history_check: F,
    ) -> Result<TerminalSummary, TerminalError>
    where
        F: FnOnce(CreateTerminalRequest) -> Result<CreateTerminalRequest, TerminalError>,
    {
        self.create_inner(request, approved, None, None, history_check)
    }

    /// Recreates one host-confirmed saved terminal inside the exact retained
    /// root that owns it. The root is not accepted from the wire: the caller
    /// must derive it from durable host state and include it in `approved`.
    /// This narrow seam lets a pass-off pane whose saved cwd is a descendant
    /// survive a cold restart without widening ordinary create authorization.
    ///
    /// # Errors
    ///
    /// Returns the same validation, authorization, conflict, lease, and spawn
    /// errors as [`Self::create_with_provider_history_check`].
    pub fn create_with_provider_history_check_in_retained_root<F>(
        &self,
        request: CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
        retained_root: &Path,
        history_check: F,
    ) -> Result<TerminalSummary, TerminalError>
    where
        F: FnOnce(CreateTerminalRequest) -> Result<CreateTerminalRequest, TerminalError>,
    {
        self.create_inner(request, approved, Some(retained_root), None, history_check)
    }

    /// Return an already-owned terminal when `request` is its exact original
    /// create intent, without consulting mutable host authority or provider
    /// metadata.
    ///
    /// This is a narrow host preflight for transport retries. It never creates
    /// an entry and returns `None` when the ID is not live. A handoff's payload
    /// is intentionally absent from the create fingerprint, just as it is in
    /// [`Self::create`]; replaying one therefore observes the existing pane
    /// without rewriting its transcript or re-reading its source process.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid request, a retired ID, or an existing
    /// ID whose original create fingerprint differs.
    pub fn exact_create_replay(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<Option<TerminalSummary>, TerminalError> {
        self.exact_create_replay_inner(request, false)
    }

    /// Return an existing exact create while treating its complete provider
    /// reopen identity as host-authoritative rather than renderer intent.
    ///
    /// The desktop host uses this only after matching the live terminal to its
    /// private reopen-authority sidecar. Every other create field remains part
    /// of the immutable fingerprint. This lets a lost-response retry carrying
    /// a renderer roster that is one provider transition behind observe the
    /// already-created PTY instead of conflicting or spawning another.
    ///
    /// # Errors
    ///
    /// Returns the same validation and conflict errors as
    /// [`Self::exact_create_replay`].
    pub fn exact_create_replay_with_authoritative_provider_session(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<Option<TerminalSummary>, TerminalError> {
        self.exact_create_replay_inner(request, true)
    }

    fn exact_create_replay_inner(
        &self,
        request: &CreateTerminalRequest,
        provider_session_is_authoritative: bool,
    ) -> Result<Option<TerminalSummary>, TerminalError> {
        validate_size(request.size)?;
        if request.cwd.is_empty() {
            return Err(TerminalError::UnsafeCwd);
        }
        validated_pass_off(request)?;

        let _gate = lock(&self.shared.lifecycle_gate);
        let existing_cwd = {
            let state = lock(&self.shared.state);
            if state.retired.contains(&request.id) {
                return Err(TerminalError::RetiredId(request.id));
            }
            let Some(entry) = state.entries.get(&request.id) else {
                return Ok(None);
            };
            entry.fingerprint.cwd.clone()
        };
        let cwd = if request.handoff.is_some() {
            existing_cwd
        } else {
            snapshot_directory(Path::new(&request.cwd))
                .map_err(root_error)?
                .path
        };
        let title = normalize_title(
            request
                .title
                .as_deref()
                .unwrap_or_else(|| default_title(&cwd)),
        )?;
        let mut fingerprint = CreateFingerprint {
            cwd,
            size: request.size,
            title,
            session_key: request.session_key.clone(),
            launch: request.launch,
            launch_session: request.launch_session.clone(),
        };
        let state = lock(&self.shared.state);
        if provider_session_is_authoritative && let Some(entry) = state.entries.get(&request.id) {
            // Ignore exactly the host-owned reopen tuple by comparing against
            // the immutable values captured with the original create. The
            // current public summary may already contain a later manual-shell
            // promotion, so it cannot be used as the create fingerprint here.
            fingerprint.cwd.clone_from(&entry.fingerprint.cwd);
            fingerprint.launch = entry.fingerprint.launch;
            fingerprint
                .launch_session
                .clone_from(&entry.fingerprint.launch_session);
        }
        preflight_create(
            &state,
            request.id,
            &fingerprint,
            self.shared.config.max_terminals,
        )
    }

    #[allow(clippy::too_many_lines)] // One lifecycle gate keeps identity, spawn, and publication ordered.
    fn create_inner<F>(
        &self,
        request: CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
        retained_root: Option<&Path>,
        shell_handoff: Option<TerminalId>,
        history_check: F,
    ) -> Result<TerminalSummary, TerminalError>
    where
        F: FnOnce(CreateTerminalRequest) -> Result<CreateTerminalRequest, TerminalError>,
    {
        let request_id = request.id;
        let fingerprint_launch_session = request.launch_session.clone();
        validate_size(request.size)?;
        if request.cwd.is_empty() {
            return Err(TerminalError::UnsafeCwd);
        }
        validated_pass_off(&request)?;
        let directory = match shell_handoff {
            Some(replaces) => self.shell_handoff_directory(replaces, &request, approved)?,
            None => self.create_directory(&request, approved, retained_root)?,
        };
        let title = normalize_title(
            request
                .title
                .as_deref()
                .unwrap_or_else(|| default_title(&directory.cwd.path)),
        )?;
        let fingerprint = CreateFingerprint {
            cwd: directory.cwd.path.clone(),
            size: request.size,
            title: title.clone(),
            session_key: request.session_key.clone(),
            launch: request.launch,
            launch_session: fingerprint_launch_session,
        };

        // Return an exact replay before consulting mutable provider history.
        // The check itself runs outside this global gate so a slow store cannot
        // stall creates and restarts for unrelated terminal profiles.
        {
            let _gate = lock(&self.shared.lifecycle_gate);
            let state = lock(&self.shared.state);
            if let Some(existing) = preflight_create(
                &state,
                request_id,
                &fingerprint,
                self.shared.config.max_terminals,
            )? {
                return Ok(existing);
            }
        }
        let history_result = history_check(request);

        // Recheck after the unlocked lookup: another caller may have created
        // this ID while provider metadata was being read. An exact replay wins
        // even when that read failed, preserving the create contract.
        let _gate = lock(&self.shared.lifecycle_gate);
        let codex_cwd_shared = {
            let state = lock(&self.shared.state);
            if let Some(existing) = preflight_create(
                &state,
                request_id,
                &fingerprint,
                self.shared.config.max_terminals,
            )? {
                return Ok(existing);
            }
            // Both the peer scan and the worktree check inside `resolve` ask
            // about the directory this pane will actually start in, which for
            // a pass-off is the source agent's, not the one the request named.
            shares_live_codex_cwd(&state, request_id, &directory.cwd.path)
        };
        let request = history_result?;
        let (title, title_is_user, file_title_session) = self.initial_title(&request, title)?;
        // One catalog from here to the spawn, whatever a refresh swaps in,
        // with the provider account this pane runs on applied.
        let launches = self.shared.account_launches(
            self.shared.launches(),
            request.launch,
            request.account.as_deref(),
            request.launch_session.as_ref(),
        )?;
        // A sign-in pane runs the CLI's login flow: no chat to capture.
        let capture = if request.sign_in {
            None
        } else {
            GenerationCapture::for_launch(&launches, request.launch)
        };
        let public_launch_session =
            public_provider_session_state(capture.is_some(), request.launch_session.as_ref());
        // Deliberately after the idempotence check: the handoff is excluded
        // from the create fingerprint, so a replay carrying a different
        // transcript returns the existing entry — and must not rewrite the file
        // the live pane was already pointed at. This gate already spans a lease
        // acquisition and a PTY spawn, so one small write adds nothing
        // meaningful to its hold time.
        let generation = TerminalGeneration::initial();
        let (launch, typed_handoff) = self.create_launch(
            &launches,
            &request,
            &directory.cwd.path,
            codex_cwd_shared,
            capture.as_ref(),
        )?;

        let root = path_string(&directory.root.path)?.to_owned();
        let cwd = path_string(&directory.cwd.path)?.to_owned();
        let (prepared, provider_session_process) = self.spawn_generation(
            GenerationSpec {
                id: request.id,
                generation,
                directory: &directory,
                root: &root,
                cwd: &cwd,
                size: request.size,
                session_key: request.session_key.clone(),
                launch,
                capture: capture.as_ref(),
                prior_launch_session: public_launch_session.as_ref(),
            },
            || self.discard_handoff(&request),
        )?;
        let timestamp = now();
        let summary = TerminalSummary {
            id: request.id,
            generation,
            title,
            root,
            cwd,
            session_key: request.session_key,
            launch: request.launch,
            launch_session: public_launch_session,
            state: TerminalState::Running,
            size: request.size,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            exit: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            passed_off_from: request
                .handoff
                .as_ref()
                .map(|handoff| handoff.source_terminal_id),
            replaces: shell_handoff,
        };
        if let Some(replaces) = shell_handoff
            && let Some(entry) = lock(&self.shared.state).entries.get_mut(&replaces)
        {
            entry.shell_handoff = ShellHandoff::HandedOff;
        }
        let summary = self.install_generation(NewGeneration {
            summary,
            fingerprint,
            directory,
            title_is_user,
            file_title_session,
            capture,
            provider_session_process,
            prepared,
        });
        if let Some(prompt) = typed_handoff {
            self.type_handoff_when_ready(
                TerminalGenerationRequest {
                    id: summary.id,
                    generation: summary.generation,
                },
                prompt,
            );
        }
        Ok(summary)
    }

    /// Rename a live or retained terminal entry.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid title, missing ID, or stale generation.
    pub fn rename(&self, request: RenameTerminalRequest) -> Result<TerminalSummary, TerminalError> {
        let RenameTerminalRequest {
            id,
            generation,
            title: requested_title,
        } = request;
        let title = normalize_title(&requested_title)?;
        // The event gate orders this rename against other renames, auto
        // titles, restarts and dismissals for the whole call. The global
        // state lock is not held across the durable pin write, so input,
        // resizes, snapshots and pollers of every pane keep running during
        // its fsync; the generation is validated before and after it.
        let event_gate = lock(&self.shared.event_gate);
        current_entry(&lock(&self.shared.state), id, generation)?;
        self.shared
            .title_persistence
            .pin_title(id, &title)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        let result = {
            let mut state = lock(&self.shared.state);
            let entry = current_entry_mut(&mut state, id, generation)?;
            // Latch first: re-typing the name a terminal already shows is how a
            // user pins an auto-title, and the unchanged-title early return
            // below would otherwise discard that intent.
            entry.title_is_user = true;
            if entry.summary.title == title {
                return Ok(entry.summary.clone());
            }
            entry.summary.title = title;
            entry.summary.updated_at = now();
            let summary = entry.summary.clone();
            state.revision = state.revision.saturating_add(1);
            let event = TerminalLifecycleEvent::Upsert {
                revision: state.revision,
                terminal: summary.clone(),
            };
            (summary, event)
        };
        self.shared.events.publish(result.1);
        drop(event_gate);
        Ok(result.0)
    }

    /// Resize an active PTY, fenced by generation.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid dimensions, missing/stale/non-running
    /// entries, or a PTY resize failure.
    pub fn resize(&self, request: ResizeTerminalRequest) -> Result<TerminalSummary, TerminalError> {
        validate_size(request.size)?;
        // A resize has two externally visible halves: the ioctl changes the
        // process' real PTY geometry, then the summary/event records it. Keep
        // those halves behind the same gate so concurrent renderer requests
        // cannot apply in one order and publish in another. This also prevents
        // an equal-to-summary request from returning early while an older
        // physical resize is still in flight.
        let event_gate = lock(&self.shared.event_gate);
        let control = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            entry.ensure_running()?;
            if entry.summary.size == request.size {
                return Ok(entry.summary.clone());
            }
            Arc::clone(&entry.resources.control)
        };
        control
            .resize(request.size)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        let result = {
            let mut state = lock(&self.shared.state);
            let entry = current_entry_mut(&mut state, request.id, request.generation)?;
            entry.ensure_running()?;
            entry.summary.size = request.size;
            entry.summary.updated_at = now();
            let summary = entry.summary.clone();
            state.revision = state.revision.saturating_add(1);
            let event = TerminalLifecycleEvent::Upsert {
                revision: state.revision,
                terminal: summary.clone(),
            };
            (summary, event)
        };
        self.shared.events.publish(result.1);
        drop(event_gate);
        Ok(result.0)
    }

    /// Write exact input bytes to an active PTY. This operation is not
    /// replay-safe after an uncertain caller-side transport failure.
    ///
    /// # Errors
    ///
    /// Returns an error for an oversized write, missing/stale/non-running
    /// entry, or PTY write failure.
    pub fn write(
        &self,
        request: TerminalGenerationRequest,
        bytes: &[u8],
    ) -> Result<(), TerminalError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(TerminalError::InputTooLarge);
        }
        let input = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            entry.ensure_running()?;
            Arc::clone(&entry.resources.input)
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let attention = self.wake_for_input(request, bytes)?;
        let written = input
            .write(bytes)
            .map_err(|error| TerminalError::Backend(error.to_string()));
        if attention {
            let power = {
                let state = lock(&self.shared.state);
                current_entry(&state, request.id, request.generation)
                    .map(|entry| Arc::clone(&entry.power))
            };
            if let Ok(power) = power {
                workspace_sleep::publish_attention(
                    &self.shared,
                    request.id,
                    request.generation,
                    &power,
                );
            }
        }
        written
    }

    /// The retired Clean settings channel. It stays served as wire protocol:
    /// a well-formed request for the current running generation is answered
    /// that terminal settings are unavailable, and nothing is written.
    ///
    /// # Errors
    /// Rejects malformed screens and stale or stopped generations.
    pub fn advance_controls(
        &self,
        request: &interface_contracts::terminal_controls::TerminalControlsRequestV1,
    ) -> Result<interface_contracts::terminal_controls::TerminalControlsStateV1, TerminalError>
    {
        if request.version != 1 || !crate::screen::valid_screen(&request.screen) {
            return Err(TerminalError::InvalidConfiguration);
        }
        let state = lock(&self.shared.state);
        current_entry(&state, request.id, request.generation)?.ensure_running()?;
        Ok(
            interface_contracts::terminal_controls::TerminalControlsStateV1 {
                version: 1,
                error: Some("Terminal settings controls are no longer available.".into()),
                ..Default::default()
            },
        )
    }

    /// Request process-tree termination. Repeating the close for the current
    /// generation is idempotent, including after the exit event lands.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/stale entry or backend teardown failure.
    pub fn close(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<TerminalSummary, TerminalError> {
        let (summary, control, output) = {
            // Provider attach/demotion owns the same gate across its durable
            // owner transaction. Marking close inside that order prevents a
            // shutdown-triggered kill from being mistaken for a natural
            // manual-Claude exit and persisted as a shell demotion.
            let gate = provider_session_gate(&self.shared, request.id, request.generation);
            let _provider_session_gate = lock(&gate);
            let mut state = lock(&self.shared.state);
            let entry = current_entry_mut(&mut state, request.id, request.generation)?;
            entry.closing = true;
            entry.shell_handoff = ShellHandoff::CloseRequested;
            (
                entry.summary.clone(),
                entry
                    .is_live()
                    .then(|| Arc::clone(&entry.resources.control)),
                Arc::clone(&entry.resources.output),
            )
        };
        // Explicit close also seals the producer side. Queued chunks remain
        // available to the current or a replacement output reader, while a
        // producer blocked on a full buffer wakes and releases its PTY handle.
        output.close();
        seal_nested_provider_for_terminal(&self.shared, request.id, request.generation)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        let teardown = control.map_or(Ok(()), |control| control.terminate_tree());
        teardown.map_err(|error| TerminalError::Backend(error.to_string()))?;
        Ok(summary)
    }

    /// Whether one exact generation has reached a terminal state (or no
    /// longer exists) after all lifecycle publication that established that
    /// state. Acquiring `event_gate` first turns this into a barrier against
    /// the waiter window between mutating the service summary and publishing
    /// its final Upsert.
    #[must_use]
    pub fn generation_has_settled(&self, request: TerminalGenerationRequest) -> bool {
        let _event_gate = lock(&self.shared.event_gate);
        let state = lock(&self.shared.state);
        state
            .entries
            .get(&request.id)
            .is_none_or(|entry| entry.summary.generation != request.generation || !entry.is_live())
    }

    /// Restart a retained exited/failed entry with the same ID and the next
    /// generation. The original exact root must still be authorized.
    ///
    /// # Errors
    ///
    /// Returns an error for missing/stale/running/exhausted entries,
    /// authorization drift, lease failure, or PTY startup failure.
    pub fn restart(
        &self,
        request: TerminalGenerationRequest,
        approved: &ApprovedTerminalRoots,
    ) -> Result<TerminalSummary, TerminalError> {
        let _gate = lock(&self.shared.lifecycle_gate);
        let (previous, fingerprint, root_path, cwd_path, title_is_user, file_title_session) = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if entry.is_live() {
                return Err(TerminalError::StillRunningForRestart(request.id));
            }
            if entry.provider_session_capture_token.is_some() {
                return Err(TerminalError::ProviderSessionCaptureUnavailable(
                    entry
                        .summary
                        .launch
                        .map_or(TerminalLaunchProfileId::Claude, |launch| launch.profile_id),
                ));
            }
            (
                entry.summary.clone(),
                entry.fingerprint.clone(),
                entry.root.clone(),
                entry.cwd.clone(),
                entry.title_is_user,
                entry.file_title_session.clone(),
            )
        };
        let generation = previous.generation.next();
        if generation == previous.generation {
            return Err(TerminalError::GenerationExhausted(request.id));
        }
        // Re-authorize before the peer scan and the launch resolve: both ask
        // about the directory this pane will actually reopen in, and for a
        // pass-off pane that is a verified descendant of its root rather than
        // the root itself. The second state read is safe because
        // `lifecycle_gate` is held across both, so no entry can appear between
        // them.
        let directory = reauthorized(approved, &root_path, &cwd_path)?;
        let codex_cwd_shared = {
            let state = lock(&self.shared.state);
            shares_live_codex_cwd(&state, request.id, &directory.cwd.path)
        };
        let reopened_launch_session = reopened(previous.launch_session.as_ref());
        let launches = self.shared.account_launches(
            self.shared.launches(),
            previous.launch,
            None,
            reopened_launch_session.as_ref(),
        )?;
        let capture = GenerationCapture::for_launch(&launches, previous.launch);
        // A restart is a reopen, not a second first launch: replaying the
        // create-time argv would ask the CLI to mint the id it already owns.
        let launch = launches.resolve_with_peers(
            previous.launch,
            reopened_launch_session.as_ref(),
            codex_cwd_shared,
            capture.as_ref().map(|capture| capture.token.as_str()),
        )?;
        let root = path_string(&directory.root.path)?.to_owned();
        let cwd = path_string(&directory.cwd.path)?.to_owned();
        let next_launch_session =
            public_provider_session_state(capture.is_some(), reopened_launch_session.as_ref());
        let (prepared, provider_session_process) = self.spawn_generation(
            GenerationSpec {
                id: request.id,
                generation,
                directory: &directory,
                root: &root,
                cwd: &cwd,
                size: previous.size,
                session_key: previous.session_key.clone(),
                launch,
                capture: capture.as_ref(),
                prior_launch_session: next_launch_session.as_ref(),
            },
            || {},
        )?;
        let mut summary = previous;
        summary.generation = generation;
        summary.launch_session = next_launch_session;
        summary.fast_mode = None;
        summary.startup_download = None;
        summary.state = TerminalState::Running;
        summary.updated_at = now();
        summary.exit = None;
        Ok(self.install_generation(NewGeneration {
            summary,
            fingerprint,
            directory,
            // A restart is a reopen of the same tab, so a name the user
            // typed outlives the process that was running under it.
            title_is_user,
            file_title_session,
            capture,
            provider_session_process,
            prepared,
        }))
    }

    /// Synchronously file and retire any capture evidence left by a settled
    /// generation before its restart transaction begins.
    ///
    /// The host calls this before opening its authority transaction so the
    /// lifecycle sink can make a recovered provider binding durable. A
    /// retryable persistence/checkpoint/retirement failure leaves the capture
    /// attached to the exact generation and refuses the restart preflight.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing, stale, or running generation, or while
    /// provider capture settlement remains retryable.
    #[allow(clippy::too_many_lines)] // One gate keeps lookup, persistence, checkpoint, and retirement atomic.
    pub fn settle_provider_capture_for_restart(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<(), TerminalError> {
        let _lifecycle_gate = lock(&self.shared.lifecycle_gate);
        let event_gate = lock(&self.shared.event_gate);
        let capture = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if entry.is_live() {
                return Err(TerminalError::StillRunningForRestart(request.id));
            }
            let Some(capture_token) = entry.provider_session_capture_token.clone() else {
                return Ok(());
            };
            let profile_id = entry.provider_session_profile.ok_or(
                TerminalError::ProviderSessionCaptureUnavailable(TerminalLaunchProfileId::Claude),
            )?;
            if entry.manual_capture()
                && entry.provider_session_process.is_some()
                && entry.provider_session_cwd.is_none()
            {
                return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
            }
            (
                profile_id,
                capture_token,
                entry.provider_session_process.clone(),
            )
        };
        let (profile_id, capture_token, process) = capture;
        if let Some(process) = process.as_ref() {
            let provider_session = self
                .shared
                .provider_sessions
                .final_session(profile_id, process, &capture_token)
                .map_err(|_| TerminalError::ProviderSessionCaptureUnavailable(profile_id))?;
            if let Some(provider_session) = provider_session {
                if !is_provider_session_id(&provider_session.id) {
                    return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
                }
                let (event, effective_session, effective_summary) = {
                    let mut state = lock(&self.shared.state);
                    let entry = current_entry_mut(&mut state, request.id, request.generation)?;
                    if !entry.owns_capture(request.generation, process, &capture_token) {
                        return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
                    }
                    let manual_cwd = entry.provider_session_cwd.clone();
                    let changed = match bind_provider_session(
                        entry.binding_target(),
                        profile_id,
                        &provider_session,
                        manual_cwd,
                    ) {
                        Binding::Changed => true,
                        Binding::Unchanged => false,
                        Binding::Refused => {
                            return Err(TerminalError::ProviderSessionCaptureUnavailable(
                                profile_id,
                            ));
                        }
                    };
                    let summary = entry.summary.clone();
                    let effective = summary
                        .launch_session
                        .clone()
                        .ok_or(TerminalError::ProviderSessionCaptureUnavailable(profile_id))?;
                    let event = changed.then(|| {
                        state.revision = state.revision.saturating_add(1);
                        TerminalLifecycleEvent::Upsert {
                            revision: state.revision,
                            terminal: summary.clone(),
                        }
                    });
                    (event, effective, summary)
                };
                let persistence = self
                    .shared
                    .events
                    .persist_provider_reopen(&effective_summary);
                if let Some(event) = event {
                    self.shared.events.publish(event);
                }
                if persistence == TerminalProviderSessionPersistence::RetryableFailure
                    || self
                        .shared
                        .provider_sessions
                        .checkpoint_capture(profile_id, &capture_token, &effective_session)
                        .is_err()
                {
                    return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
                }
            }
        }
        self.shared
            .provider_sessions
            .end_capture(profile_id, &capture_token)
            .map_err(|_| TerminalError::ProviderSessionCaptureUnavailable(profile_id))?;
        let mut state = lock(&self.shared.state);
        let entry = current_entry_mut(&mut state, request.id, request.generation)?;
        if entry.provider_session_capture_token.as_deref() != Some(&capture_token)
            || entry.provider_session_process != process
        {
            return Err(TerminalError::ProviderSessionCaptureUnavailable(profile_id));
        }
        entry.clear_provider_capture();
        drop(event_gate);
        Ok(())
    }

    /// Restore the provider-session binding that preceded one failed restart
    /// and retire that failed generation's capture evidence.
    ///
    /// This host-only rollback seam is deliberately generation-fenced and only
    /// accepts a settled process. Taking `event_gate` before inspecting state
    /// also makes a successful no-op an event-publication barrier: every
    /// lifecycle update for the failed generation is in the sink before the
    /// caller completes its outer transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing or stale generation, while that exact
    /// process is still starting or running, or when capture retirement remains
    /// retryable. In the latter case the in-memory capture is preserved so a
    /// later retry or orderly shutdown cannot silently discard its evidence.
    pub fn restore_settled_provider_session_binding(
        &self,
        request: TerminalGenerationRequest,
        prior: Option<TerminalLaunchSessionV1>,
    ) -> Result<TerminalSummary, TerminalError> {
        let _lifecycle_gate = lock(&self.shared.lifecycle_gate);
        let event_gate = lock(&self.shared.event_gate);
        let capture = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if entry.is_live() {
                return Err(TerminalError::StillRunning(request.id));
            }
            entry
                .provider_session_profile
                .zip(entry.provider_session_capture_token.clone())
        };
        if let Some((profile_id, capture_token)) = capture.as_ref()
            && self
                .shared
                .provider_sessions
                .end_capture(*profile_id, capture_token)
                .is_err()
        {
            return Err(TerminalError::ProviderSessionCaptureUnavailable(
                *profile_id,
            ));
        }
        let (summary, event) = {
            let mut state = lock(&self.shared.state);
            let entry = current_entry_mut(&mut state, request.id, request.generation)?;
            if capture.as_ref().is_some_and(|(_, token)| {
                entry.provider_session_capture_token.as_ref() != Some(token)
            }) {
                return Err(TerminalError::StaleGeneration {
                    id: request.id,
                    expected: request.generation,
                    actual: entry.summary.generation,
                });
            }
            let changed = entry.summary.launch_session != prior;
            if changed {
                entry.summary.launch_session = prior;
                entry.summary.updated_at = now();
            }
            entry.clear_provider_capture();
            let summary = entry.summary.clone();
            let event = changed.then(|| {
                state.revision = state.revision.saturating_add(1);
                TerminalLifecycleEvent::Upsert {
                    revision: state.revision,
                    terminal: summary.clone(),
                }
            });
            (summary, event)
        };
        if let Some(event) = event {
            self.shared.events.publish(event);
        }
        drop(event_gate);
        Ok(summary)
    }

    /// Remove an exited/failed entry. Missing IDs are an idempotent no-op;
    /// running entries must be explicitly closed first.
    ///
    /// # Errors
    ///
    /// Returns an error only when the matching terminal is still running.
    pub fn dismiss(
        &self,
        request: DismissTerminalRequest,
    ) -> Result<DismissTerminalResult, TerminalError> {
        self.dismiss_checked(request.id, None)
    }

    /// Remove an exited/failed entry only after a host-owned durable
    /// pre-removal action succeeds.
    ///
    /// The callback runs while the service lifecycle and event gates are held,
    /// after state validation but before capture retirement or in-memory
    /// removal. It must not publish a terminal lifecycle event. The desktop
    /// host uses this seam to revoke exact cold-reopen authority without
    /// holding its state-store lock across event publication.
    ///
    /// The outer result belongs to the host callback; the inner result is the
    /// normal terminal dismissal result.
    ///
    /// # Errors
    ///
    /// Returns the callback's outer error without retiring provider evidence
    /// or removing the entry. Terminal validation and capture failures are
    /// returned in the inner result.
    pub fn dismiss_after<E>(
        &self,
        request: DismissTerminalRequest,
        before_remove: impl FnOnce() -> Result<(), E>,
    ) -> Result<Result<DismissTerminalResult, TerminalError>, E> {
        self.dismiss_checked_after(request.id, None, before_remove)
    }

    /// Remove an exited/failed entry only when it is still the named process
    /// generation. This host-only form fences delayed rollback work from
    /// dismissing a newer successful restart that reused the same terminal ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal is running or the ID now names a
    /// different generation. A missing ID remains an idempotent no-op.
    pub fn dismiss_generation(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<DismissTerminalResult, TerminalError> {
        self.dismiss_checked(request.id, Some(request.generation))
    }

    /// Remove one settled, unexposed generation without retiring its stable
    /// terminal ID or publishing a lifecycle removal. The host uses this only
    /// to roll back a spawn whose lifecycle events were transactionally held
    /// from the renderer; a later cold-reopen retry must be allowed to reuse
    /// the saved ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the generation is stale or still running. A
    /// missing ID is an idempotent no-op.
    pub fn abandon_generation(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<DismissTerminalResult, TerminalError> {
        let _lifecycle = lock(&self.shared.lifecycle_gate);
        let _event_gate = lock(&self.shared.event_gate);
        let capture = {
            let state = lock(&self.shared.state);
            let Some(entry) = state.entries.get(&request.id) else {
                return Ok(DismissTerminalResult {
                    id: request.id,
                    removed: false,
                });
            };
            if entry.summary.generation != request.generation {
                return Err(TerminalError::StaleGeneration {
                    id: request.id,
                    expected: request.generation,
                    actual: entry.summary.generation,
                });
            }
            if entry.is_live() {
                return Err(TerminalError::StillRunning(request.id));
            }
            entry
                .provider_session_profile
                .zip(entry.provider_session_capture_token.clone())
        };
        if let Some((profile_id, capture_token)) = capture.as_ref() {
            self.shared
                .provider_sessions
                .end_capture(*profile_id, capture_token)
                .map_err(|_| TerminalError::ProviderSessionCaptureUnavailable(*profile_id))?;
        }
        let removed = {
            let mut state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            if capture.as_ref().is_some_and(|(_, token)| {
                entry.provider_session_capture_token.as_ref() != Some(token)
            }) {
                return Err(TerminalError::StaleGeneration {
                    id: request.id,
                    expected: request.generation,
                    actual: entry.summary.generation,
                });
            }
            state
                .entries
                .remove(&request.id)
                .ok_or(TerminalError::NotFound(request.id))?
        };
        removed.resources.output.close();
        Ok(DismissTerminalResult {
            id: request.id,
            removed: true,
        })
    }

    fn dismiss_checked(
        &self,
        id: TerminalId,
        expected_generation: Option<TerminalGeneration>,
    ) -> Result<DismissTerminalResult, TerminalError> {
        match self.dismiss_checked_after(id, expected_generation, || {
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(result) => result,
            Err(never) => match never {},
        }
    }

    fn dismiss_checked_after<E>(
        &self,
        id: TerminalId,
        expected_generation: Option<TerminalGeneration>,
        before_remove: impl FnOnce() -> Result<(), E>,
    ) -> Result<Result<DismissTerminalResult, TerminalError>, E> {
        // Restart holds the same gate across spawn and publication. Without
        // this lock, a delayed cleanup could inspect generation N, race a
        // restart while it spawns N+1, and remove the ID before N+1 lands.
        let _lifecycle = lock(&self.shared.lifecycle_gate);
        let event_gate = lock(&self.shared.event_gate);
        let capture = {
            let state = lock(&self.shared.state);
            let Some(entry) = state.entries.get(&id) else {
                before_remove()?;
                return Ok(Ok(DismissTerminalResult { id, removed: false }));
            };
            if let Some(expected) = expected_generation
                && entry.summary.generation != expected
            {
                return Ok(Err(TerminalError::StaleGeneration {
                    id,
                    expected,
                    actual: entry.summary.generation,
                }));
            }
            if entry.is_live() {
                return Ok(Err(TerminalError::StillRunning(id)));
            }
            entry
                .provider_session_profile
                .zip(entry.provider_session_capture_token.clone())
        };
        // This is the last fallible step before provider evidence retirement
        // and in-memory removal. If it fails, the entry and its capture remain
        // byte-for-byte owned by the service.
        before_remove()?;
        if let Some((profile_id, capture_token)) = capture.as_ref() {
            if self
                .shared
                .provider_sessions
                .end_capture(*profile_id, capture_token)
                .is_err()
            {
                return Ok(Err(TerminalError::ProviderSessionCaptureUnavailable(
                    *profile_id,
                )));
            }
        }
        let removed = {
            let mut state = lock(&self.shared.state);
            let Some(entry) = state.entries.get(&id) else {
                return Ok(Ok(DismissTerminalResult { id, removed: false }));
            };
            if let Some(expected) = expected_generation
                && entry.summary.generation != expected
            {
                return Ok(Err(TerminalError::StaleGeneration {
                    id,
                    expected,
                    actual: entry.summary.generation,
                }));
            }
            if entry.is_live() {
                return Ok(Err(TerminalError::StillRunning(id)));
            }
            if capture.as_ref().is_some_and(|(_, token)| {
                entry.provider_session_capture_token.as_ref() != Some(token)
            }) {
                return Ok(Err(TerminalError::StaleGeneration {
                    id,
                    expected: expected_generation.unwrap_or(entry.summary.generation),
                    actual: entry.summary.generation,
                }));
            }
            let generation = entry.summary.generation;
            let removed = state
                .entries
                .remove(&id)
                .expect("validated terminal entry remains present under lifecycle gate");
            state.retired.insert(id);
            state.revision = state.revision.saturating_add(1);
            let event = TerminalLifecycleEvent::Removed {
                revision: state.revision,
                id,
                generation,
            };
            (removed, event)
        };
        removed.0.resources.output.close();
        self.shared.events.publish(removed.1);
        drop(event_gate);
        Ok(Ok(DismissTerminalResult { id, removed: true }))
    }

    /// Claim the one output consumer for a terminal generation.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/stale entry or a second active reader.
    pub fn take_output(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<TerminalOutputReader, TerminalError> {
        let output = {
            let state = lock(&self.shared.state);
            let entry = current_entry(&state, request.id, request.generation)?;
            Arc::clone(&entry.resources.output)
        };
        output.claim()
    }

    /// Where a create starts, and which approved root owns it: a pass-off
    /// follows its source agent, anything else opens exactly the approved root
    /// the request named.
    fn create_directory(
        &self,
        request: &CreateTerminalRequest,
        approved: &ApprovedTerminalRoots,
        retained_root: Option<&Path>,
    ) -> Result<AuthorizedDirectory, TerminalError> {
        match request.handoff.as_ref() {
            Some(handoff) => {
                self.pass_off_directory(handoff.source_terminal_id, &request.cwd, approved)
            }
            None => match retained_root {
                // A slash-command session at "/" must not retain the whole
                // filesystem. Re-derive from the saved cwd against the current
                // allowlist so poisoned reopen proofs heal on the next spawn.
                Some(root) if root.parent().is_none() => approved
                    .authorize(Path::new(&request.cwd))
                    .map(AuthorizedDirectory::exact)
                    .or_else(|_| approved.authorize_pass_off(Path::new(&request.cwd)))
                    .map_err(root_error),
                Some(root) => approved
                    .authorize_retained(Path::new(&request.cwd), root)
                    .map_err(root_error),
                None => approved
                    .authorize(Path::new(&request.cwd))
                    .map(AuthorizedDirectory::exact)
                    .map_err(root_error),
            },
        }
    }

    /// The argv this create will spawn: the catalog's program for the profile,
    /// bound to its chat, and then — for a pass-off — pointed at the transcript
    /// this writes. A pass-off to a CLI that takes no initial prompt also
    /// returns the prompt to type once its TUI is ready.
    ///
    /// The two steps are in this order and only in this order:
    /// `bind_handoff_prompt` must run after session binding, because the Codex
    /// arm of `bind_session` prepends `resume --last` and would push the prompt
    /// out of its trailing position.
    fn create_launch(
        &self,
        launches: &TerminalLaunchCatalog,
        request: &CreateTerminalRequest,
        cwd: &Path,
        codex_cwd_shared: bool,
        capture: Option<&GenerationCapture>,
    ) -> Result<(ResolvedTerminalLaunch, Option<String>), TerminalError> {
        if request.sign_in {
            return Ok((launches.resolve_sign_in(request.launch)?, None));
        }
        let launch = launches.resolve_with_peers(
            request.launch,
            request.launch_session.as_ref(),
            codex_cwd_shared,
            capture.map(|capture| capture.token.as_str()),
        )?;
        self.pass_off_launch(request, launch, cwd)
    }

    /// Spawn one generation in the fail-closed provider-effect order: commit
    /// the capture owner, take the workspace lease, spawn, check the cwd did
    /// not change under the spawn, and record a direct child's identity. On
    /// any failure the capture owner is retired only once no process this
    /// spawned can still be alive (a teardown that cannot be proven keeps the
    /// lease and resources on a retry thread instead), then `on_failure`
    /// undoes the caller's own preparation.
    fn spawn_generation(
        &self,
        spec: GenerationSpec<'_>,
        on_failure: impl Fn(),
    ) -> Result<(PreparedTerminal, Option<TerminalProcessIdentity>), TerminalError> {
        let GenerationSpec {
            id,
            generation,
            directory,
            root,
            cwd,
            size,
            session_key,
            launch,
            capture,
            prior_launch_session,
        } = spec;
        let failed = |settled: bool, error: TerminalError| {
            if settled && let Some(capture) = capture {
                let _ = self
                    .shared
                    .provider_sessions
                    .end_capture(capture.profile_id, &capture.token);
            }
            on_failure();
            error
        };
        if let Some(capture) = capture {
            let owner = capture.owner(id, generation, root, cwd, prior_launch_session);
            if self
                .shared
                .provider_sessions
                .begin_capture(capture.profile_id, &owner)
                .is_err()
            {
                return Err(failed(
                    true,
                    TerminalError::ProviderSessionCaptureUnavailable(capture.profile_id),
                ));
            }
        }
        assert_directory(&directory.cwd).map_err(|_| failed(true, TerminalError::CwdChanged))?;
        // The lease names the approved root, not the pane's starting
        // directory: a host asks `workspace_is_open` with the worktree root,
        // and a pass-off pane deeper inside must still protect the whole tree.
        let lease = self
            .shared
            .leases
            .acquire(&TerminalLeaseRequest {
                id,
                generation,
                root: directory.root.path.clone(),
                cwd: directory.cwd.path.clone(),
                session_key,
            })
            .map_err(|error| failed(true, TerminalError::Lease(error.to_string())))?;
        let spawned = self
            .shared
            .backend
            .spawn(&TerminalSpawnSpec {
                cwd: directory.cwd.path.clone(),
                size,
                launch,
            })
            .map_err(|error| failed(true, TerminalError::Backend(error.to_string())))?;
        let prepared = PreparedTerminal {
            spawned,
            output: Arc::new(OutputBuffer::new(
                id,
                generation,
                self.shared.config.output_capacity_bytes,
            )),
            lease,
        };
        if assert_directory(&directory.cwd).is_err() {
            let settled = abort_prepared_terminal(prepared);
            return Err(failed(settled, TerminalError::CwdChanged));
        }
        let Some(capture) =
            capture.filter(|capture| matches!(capture.kind, ProviderSessionCaptureKind::Direct))
        else {
            return Ok((prepared, None));
        };
        let recorded = prepared
            .spawned
            .control
            .process_identity()
            .filter(|process| {
                self.shared
                    .provider_sessions
                    .record_process(capture.profile_id, &capture.token, process)
                    .is_ok()
            });
        let Some(process) = recorded else {
            let settled = abort_prepared_terminal(prepared);
            return Err(failed(
                settled,
                TerminalError::ProviderSessionCaptureUnavailable(capture.profile_id),
            ));
        };
        Ok((prepared, Some(process)))
    }

    /// Publish a spawned generation in place of whatever its id held, then
    /// start what serves it: the provider poll, the ticker, and the PTY reader
    /// and waiter. The caller holds `lifecycle_gate` throughout, so what is
    /// derived here from the summary cannot change before it is used.
    fn install_generation(&self, new: NewGeneration) -> TerminalSummary {
        let NewGeneration {
            summary,
            fingerprint,
            directory,
            title_is_user,
            file_title_session,
            capture,
            provider_session_process,
            prepared,
        } = new;
        let (id, generation) = (summary.id, summary.generation);
        let shell_display = ShellDisplay::new(&summary);
        let ticks = ticker::EntryTicks::armed(&summary, shell_display.is_some());
        let is_opencode = summary
            .launch
            .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Opencode);
        let power = Arc::new(Mutex::new(workspace_sleep::WorkspacePower::new(&summary)));
        // Start (or join) the cwd's startup-file fetch outside the gates.
        let startup_fetch = startup_download::startup_fetch(&self.shared, &directory.cwd.path);
        let wake = capture
            .as_ref()
            .map(|_| Arc::new(ProviderSessionWake::default()));
        let (capture_token, capture_profile, capture_kind) = match capture {
            Some(GenerationCapture {
                kind,
                profile_id,
                token,
            }) => (Some(token), Some(profile_id), Some(kind)),
            None => (None, None, None),
        };
        let polled_wake = wake
            .clone()
            .filter(|_| provider_recovery::captured_provider(capture_profile));
        let attached = provider_session_process.is_some();
        let event_gate = lock(&self.shared.event_gate);
        let (event, replaced, start_ticker) = {
            let mut state = lock(&self.shared.state);
            state.revision = state.revision.saturating_add(1);
            let event = TerminalLifecycleEvent::Upsert {
                revision: state.revision,
                terminal: summary.clone(),
            };
            // Under the same lock as the insert, so a ticker about to exit on
            // an empty registry cannot strand this entry.
            let start_ticker = !mem::replace(&mut state.ticker, true);
            let replaced = state.entries.insert(
                id,
                TerminalEntry {
                    power: Arc::clone(&power),
                    summary: summary.clone(),
                    fingerprint,
                    root: directory.root.path,
                    cwd: directory.cwd.path,
                    resources: TerminalResources {
                        input: Arc::clone(&prepared.spawned.input),
                        control: Arc::clone(&prepared.spawned.control),
                        output: Arc::clone(&prepared.output),
                        lease: Some(prepared.lease),
                    },
                    title_is_user,
                    title_lookup_at: None,
                    file_title_session,
                    shell_display,
                    provider_session_capture_token: capture_token,
                    provider_session_profile: capture_profile,
                    provider_session_capture_kind: capture_kind,
                    provider_session_process,
                    provider_session_cwd: None,
                    provider_session_wake: wake,
                    provider_session_finalizing: false,
                    provider_session_replacing: false,
                    closing: false,
                    shell_handoff: ShellHandoff::Owed,
                    ticks,
                    startup_fetch: Some(startup_fetch),
                },
            );
            (
                event,
                replaced.map(|entry| entry.resources.output),
                start_ticker,
            )
        };
        if let Some(output) = replaced {
            output.close();
        }
        self.shared.events.publish(event);
        drop(event_gate);
        if let Some(wake) = polled_wake {
            self.start_provider_session_poll(id, generation, wake, attached);
        }
        if start_ticker {
            self.spawn_ticker();
        }
        self.start_process_tasks(
            id,
            generation,
            prepared.spawned,
            prepared.output,
            power,
            is_opencode,
        );
        summary
    }

    fn start_process_tasks(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        spawned: SpawnedTerminal,
        output: Arc<OutputBuffer>,
        power: Arc<Mutex<workspace_sleep::WorkspacePower>>,
        is_opencode: bool,
    ) {
        let SpawnedTerminal {
            mut reader,
            input: _,
            control,
            mut waiter,
        } = spawned;
        let read_chunk_bytes = self.shared.config.output_read_chunk_bytes;
        let output_for_reader = Arc::clone(&output);
        let output_for_waiter = output;
        let (reader_finished_tx, reader_finished_rx) = mpsc::channel();
        // The reader thread is the one place every byte of every terminal
        // passes, attached or not, so the title scan lives here rather than in
        // the renderer: a tab nobody has opened still names itself.
        let titles = Arc::downgrade(&self.shared);
        let private_titles = crate::opencode_activity::TerminalOutputFilter::new(is_opencode);
        thread::spawn(move || {
            run_output_reader(
                &titles,
                TerminalGenerationRequest { id, generation },
                reader.as_mut(),
                read_chunk_bytes,
                &output_for_reader,
                &power,
                private_titles,
            );
            let _ = reader_finished_tx.send(());
        });

        let shared = Arc::downgrade(&self.shared);
        let request = TerminalGenerationRequest { id, generation };
        thread::spawn(move || {
            run_process_waiter(
                &shared,
                request,
                waiter.as_mut(),
                control.as_ref(),
                &reader_finished_rx,
                &output_for_waiter,
            );
        });
    }
}

/// Claude and the renderer mint version-4 identifiers; Codex mints version 7;
/// `OpenCode` mints its own `ses_…` ids, which only its capture ever produces.
fn is_provider_session_id(value: &str) -> bool {
    canonical_uuid(value, b'4')
        || canonical_uuid(value, b'7')
        || crate::opencode_session::opencode_session_id(value)
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Gemini, value).is_some()
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Muse, value).is_some()
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Kimi, value).is_some()
}

fn current_entry(
    state: &ServiceState,
    id: TerminalId,
    generation: TerminalGeneration,
) -> Result<&TerminalEntry, TerminalError> {
    let entry = state.entries.get(&id).ok_or(TerminalError::NotFound(id))?;
    if entry.summary.generation != generation {
        return Err(TerminalError::StaleGeneration {
            id,
            expected: generation,
            actual: entry.summary.generation,
        });
    }
    Ok(entry)
}

fn current_entry_mut(
    state: &mut ServiceState,
    id: TerminalId,
    generation: TerminalGeneration,
) -> Result<&mut TerminalEntry, TerminalError> {
    let entry = state
        .entries
        .get_mut(&id)
        .ok_or(TerminalError::NotFound(id))?;
    if entry.summary.generation != generation {
        return Err(TerminalError::StaleGeneration {
            id,
            expected: generation,
            actual: entry.summary.generation,
        });
    }
    Ok(entry)
}

/// True when a terminal other than `id` is live in `cwd` under the Codex
/// profile. Codex reopens are folder-scoped, so a peer there is the one case
/// where "the newest thread" is not this pane's to take.
fn shares_live_codex_cwd(state: &ServiceState, id: TerminalId, cwd: &Path) -> bool {
    state.entries.iter().any(|(other, entry)| {
        *other != id
            && entry.is_live()
            && entry.cwd == cwd
            && entry
                .fingerprint
                .launch
                .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Codex)
    })
}

/// Re-authorize a retained pane's directories for a reopen.
///
/// The root is held to the exact rule, exactly as a reopen always has been. A
/// pane whose working directory *is* its root takes that path and nothing
/// else, so ordinary panes behave identically. Only a pass-off that started
/// inside its root needs the wider rule, and it is additionally required to
/// still resolve to the very same root — a directory that has since come to
/// sit under a different approved root is refused.
fn reauthorized(
    approved: &ApprovedTerminalRoots,
    root: &Path,
    cwd: &Path,
) -> Result<AuthorizedDirectory, TerminalError> {
    let root = approved.authorize(root).map_err(root_error)?;
    if cwd == root.path {
        return Ok(AuthorizedDirectory::exact(root));
    }
    let directory = approved.authorize_pass_off(cwd).map_err(root_error)?;
    if directory.root != root {
        return Err(TerminalError::UnauthorizedCwd);
    }
    Ok(directory)
}

/// The create-time chat binding, rewritten as "open the one you already made".
///
/// `None` stays `None`: a terminal launched without a binding has no chat to
/// reopen, and inventing one here would resume somebody else's.
fn reopened(session: Option<&TerminalLaunchSessionV1>) -> Option<TerminalLaunchSessionV1> {
    session.map(|session| TerminalLaunchSessionV1 {
        resume: true,
        ..session.clone()
    })
}

fn validate_size(size: TerminalSize) -> Result<(), TerminalError> {
    if !(MIN_COLUMNS..=MAX_COLUMNS).contains(&size.columns)
        || !(MIN_ROWS..=MAX_ROWS).contains(&size.rows)
    {
        return Err(TerminalError::InvalidSize);
    }
    Ok(())
}

fn normalize_title(title: &str) -> Result<String, TerminalError> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > MAX_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        return Err(TerminalError::InvalidTitle);
    }
    Ok(title.to_owned())
}

fn default_title(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Terminal")
}

fn path_string(path: &Path) -> Result<&str, TerminalError> {
    path.to_str().ok_or(TerminalError::UnsafeCwd)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
