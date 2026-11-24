use std::{
    ffi::OsString,
    fs,
    io::{Read, Result as IoResult},
    path::Path,
    pin::Pin,
    sync::{Condvar, MutexGuard},
    task::{Context, Poll},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use interface_contracts::{
    engine::SessionSummary,
    terminal::{TerminalExit, TerminalLaunchProfileId, TerminalSize, TerminalState},
};
use interface_desktop_services::{DesktopServiceConfig, DesktopServiceReviewPorts};
use interface_registry::Worktree;
use interface_runtime::{RuntimeError, RuntimeErrorKind};
use interface_terminal::{
    CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, ResolvedTerminalLaunch, SpawnedTerminal,
    TerminalInput, TerminalLaunchProgram, TerminalProcessControl, TerminalProcessIdentity,
    TerminalProcessWaiter, TerminalProviderSession, TerminalProviderSessionCaptureOwner,
    TerminalProviderSessionTransition, TerminalSpawnSpec,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};

use crate::terminal_transport::{
    TERMINAL_DATA_HEADER_BYTES, TERMINAL_DATA_MAX_PAYLOAD_BYTES, TerminalDataDecoder,
    TerminalDataPlane, TerminalDataSendError, TerminalDataTransportConfig, TerminalQueueLimits,
};

use super::*;

fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Waits until `check` holds, polling every millisecond and panicking with
/// `what` after ten seconds. It sleeps between checks rather than spinning, so
/// the threads under test keep their CPU when the machine is loaded.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting: {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

struct PendingWriter;

impl AsyncWrite for PendingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<IoResult<usize>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Default)]
struct FakeRuntime {
    roots: Mutex<Vec<PathBuf>>,
}

impl NativeRuntimeView for FakeRuntime {
    fn session_is_active(&self, _session_key: &str) -> Result<bool, RuntimeError> {
        Ok(false)
    }

    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        Ok(mutex_lock(&self.roots).clone())
    }

    fn restore_worktree_home(
        &self,
        _session_key: &str,
        _worktree: &Worktree,
    ) -> Result<bool, RuntimeError> {
        Ok(false)
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        Ok(Vec::new())
    }
}

struct FailingRuntime;

impl NativeRuntimeView for FailingRuntime {
    fn session_is_active(&self, _session_key: &str) -> Result<bool, RuntimeError> {
        Ok(false)
    }

    fn approved_session_roots(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        Err(RuntimeError::definitely_unsent(
            RuntimeErrorKind::Registry,
            "registry unavailable",
            true,
        ))
    }

    fn restore_worktree_home(
        &self,
        _session_key: &str,
        _worktree: &Worktree,
    ) -> Result<bool, RuntimeError> {
        Ok(false)
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct ReleaseRecorder {
    workspaces: Mutex<Vec<String>>,
    managed_roots: Mutex<Vec<PathBuf>>,
    managed_key: Mutex<Option<String>>,
    verification_unavailable: AtomicBool,
    verification_calls: std::sync::atomic::AtomicUsize,
    unavailable_on_verification_call: std::sync::atomic::AtomicUsize,
    block_on_verification_call: std::sync::atomic::AtomicUsize,
    verification_blocked: AtomicBool,
    release_verification: AtomicBool,
    verification_changed: tokio::sync::Notify,
}

#[async_trait]
impl AutomaticWorktreeRelease for ReleaseRecorder {
    fn schedule_session_release(&self, _session_key: String) {}

    fn schedule_workspace_release(&self, workspace: String) {
        mutex_lock(&self.workspaces).push(workspace);
    }

    fn managed_workspace_root(&self, workspace: &Path) -> Option<PathBuf> {
        let workspace = std::fs::canonicalize(workspace).ok()?;
        mutex_lock(&self.managed_roots)
            .iter()
            .find(|root| workspace == **root || workspace.starts_with(root))
            .cloned()
    }

    async fn verify_managed_workspace(
        &self,
        workspace: &Path,
        expected_key: Option<&str>,
    ) -> ManagedWorkspaceVerification {
        let call = self.verification_calls.fetch_add(1, Ordering::AcqRel) + 1;
        if self.block_on_verification_call.load(Ordering::Acquire) == call {
            self.verification_blocked.store(true, Ordering::Release);
            while !self.release_verification.load(Ordering::Acquire) {
                self.verification_changed.notified().await;
            }
        }
        if self.verification_unavailable.load(Ordering::Acquire)
            || self
                .unavailable_on_verification_call
                .load(Ordering::Acquire)
                == call
        {
            return ManagedWorkspaceVerification::Unavailable;
        }
        let Some(root) = self.managed_workspace_root(workspace) else {
            return ManagedWorkspaceVerification::NotManaged;
        };
        let key = mutex_lock(&self.managed_key)
            .clone()
            .unwrap_or_else(|| "managed#test".to_owned());
        if expected_key.is_some_and(|expected| expected != key) {
            return ManagedWorkspaceVerification::NotManaged;
        }
        ManagedWorkspaceVerification::Verified(VerifiedManagedWorkspace { root, key })
    }
}

#[derive(Default)]
struct RestartCaptureLookup {
    fail_end_once: AtomicBool,
    ended: Mutex<Vec<String>>,
}

impl TerminalProviderSessionLookup for RestartCaptureLookup {
    fn begin_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn record_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn current_session(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(None)
    }

    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(
            (profile_id == TerminalLaunchProfileId::Claude).then(|| TerminalProviderSession {
                id: "44444444-4444-4444-8444-444444444444".to_owned(),
                transition: TerminalProviderSessionTransition::Resume,
            }),
        )
    }

    fn end_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        if self.fail_end_once.swap(false, Ordering::SeqCst) {
            return Err(TerminalBackendError::new(
                "planned first capture retirement failure",
            ));
        }
        mutex_lock(&self.ended).push(capture_token.to_owned());
        Ok(())
    }
}

#[derive(Default)]
struct GatedCaptureRetirementLookup {
    allow_end: AtomicBool,
    end_calls: std::sync::atomic::AtomicUsize,
}

impl TerminalProviderSessionLookup for GatedCaptureRetirementLookup {
    fn begin_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn record_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn current_session(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(None)
    }

    fn end_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        self.end_calls.fetch_add(1, Ordering::AcqRel);
        if self.allow_end.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(TerminalBackendError::new(
                "capture retirement is intentionally held",
            ))
        }
    }
}

#[derive(Default)]
struct FakeBackend {
    processes: Mutex<Vec<Arc<FakeProcess>>>,
    specs: Mutex<Vec<TerminalSpawnSpec>>,
    expose_process_identity: AtomicBool,
    /// What each spawned process prints; `ready\r\n` when unset.
    output: Mutex<Option<Vec<u8>>>,
}

impl FakeBackend {
    fn process(&self, index: usize) -> Arc<FakeProcess> {
        Arc::clone(&mutex_lock(&self.processes)[index])
    }
}

impl TerminalPtyBackend for FakeBackend {
    fn spawn(&self, spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
        mutex_lock(&self.specs).push(spec.clone());
        let process_id = u32::try_from(mutex_lock(&self.processes).len())
            .expect("test process count fits u32")
            + 10_000;
        let process = Arc::new(FakeProcess {
            identity: self
                .expose_process_identity
                .load(Ordering::Acquire)
                .then(|| TerminalProcessIdentity {
                    process_id,
                    process_group_id: process_id,
                    terminal_device: Some(format!("ttys{process_id}")),
                    process_start: format!("test-process-{process_id}"),
                }),
            ..FakeProcess::default()
        });
        mutex_lock(&self.processes).push(Arc::clone(&process));
        Ok(SpawnedTerminal {
            reader: Box::new(FakeReader {
                process: Arc::clone(&process),
                output: mutex_lock(&self.output)
                    .clone()
                    .unwrap_or_else(|| b"ready\r\n".to_vec()),
                offset: 0,
            }),
            input: Arc::new(FakeInput {
                process: Arc::clone(&process),
            }),
            control: Arc::new(FakeControl {
                process: Arc::clone(&process),
            }),
            waiter: Box::new(FakeWaiter { process }),
        })
    }
}

#[derive(Default)]
struct FakeProcess {
    identity: Option<TerminalProcessIdentity>,
    stopped: Mutex<bool>,
    stopped_changed: Condvar,
    input: Mutex<Vec<u8>>,
    input_blocked: Mutex<bool>,
    input_unblocked: Condvar,
    sizes: Mutex<Vec<TerminalSize>>,
    terminate_failures: std::sync::atomic::AtomicUsize,
    terminate_calls: std::sync::atomic::AtomicUsize,
}

impl FakeProcess {
    fn stop(&self) {
        *mutex_lock(&self.stopped) = true;
        self.stopped_changed.notify_all();
    }

    /// Models a PTY whose child has stopped reading: the write parks.
    fn block_input(&self) {
        *mutex_lock(&self.input_blocked) = true;
    }

    fn unblock_input(&self) {
        *mutex_lock(&self.input_blocked) = false;
        self.input_unblocked.notify_all();
    }

    fn wait_stopped(&self) {
        let mut stopped = mutex_lock(&self.stopped);
        while !*stopped {
            stopped = self
                .stopped_changed
                .wait(stopped)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

struct FakeReader {
    process: Arc<FakeProcess>,
    output: Vec<u8>,
    offset: usize,
}

impl Read for FakeReader {
    fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
        if self.offset < self.output.len() {
            let count = buffer.len().min(self.output.len() - self.offset);
            buffer[..count].copy_from_slice(&self.output[self.offset..self.offset + count]);
            self.offset += count;
            return Ok(count);
        }
        self.process.wait_stopped();
        Ok(0)
    }
}

struct FakeInput {
    process: Arc<FakeProcess>,
}

impl TerminalInput for FakeInput {
    fn write(&self, bytes: &[u8]) -> Result<(), TerminalBackendError> {
        let mut blocked = mutex_lock(&self.process.input_blocked);
        while *blocked {
            blocked = self
                .process
                .input_unblocked
                .wait(blocked)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(blocked);
        mutex_lock(&self.process.input).extend_from_slice(bytes);
        Ok(())
    }

    fn close(&self) {}
}

struct FakeControl {
    process: Arc<FakeProcess>,
}

impl TerminalProcessControl for FakeControl {
    fn process_identity(&self) -> Option<TerminalProcessIdentity> {
        self.process.identity.clone()
    }

    fn resize(&self, size: TerminalSize) -> Result<(), TerminalBackendError> {
        mutex_lock(&self.process.sizes).push(size);
        Ok(())
    }

    fn terminate_tree(&self) -> Result<(), TerminalBackendError> {
        self.process.terminate_calls.fetch_add(1, Ordering::AcqRel);
        if self
            .process
            .terminate_failures
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(TerminalBackendError::new(
                "planned transient terminate failure",
            ));
        }
        self.process.stop();
        Ok(())
    }
}

struct FakeWaiter {
    process: Arc<FakeProcess>,
}

impl TerminalProcessWaiter for FakeWaiter {
    fn wait(&mut self) -> Result<TerminalExit, TerminalBackendError> {
        self.process.wait_stopped();
        Ok(TerminalExit {
            code: Some(0),
            signal: None,
        })
    }
}

struct Fixture {
    temporary: TempDir,
    root: PathBuf,
    default_root: PathBuf,
    desktop: Arc<DesktopService>,
    backend: Arc<FakeBackend>,
    releases: Arc<ReleaseRecorder>,
    pushes: mpsc::Receiver<PushFrame>,
    dispatcher: Arc<TerminalHostDispatcher>,
    plane: Option<TerminalDataPlane>,
    electron_input: DuplexStream,
    electron_output: DuplexStream,
}

#[tokio::test]
async fn pane_title_pin_survives_a_full_host_reopen_and_a_stale_saved_title() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .unwrap();
    fixture.dispatcher.dispatch(CHANNELS.rename_terminal,
        json!({"id": wire_id, "generation": created["generation"], "title": "My durable pane name"}))
        .await.unwrap();
    fixture.desktop.dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces":[{
        "id":"saved", "name":"Workspace 18", "createdAt":"2026-09-11T00:00:00Z", "updatedAt":"2026-09-11T00:00:00Z",
        "terminals":[{"id":wire_id,"title":"Stale automatic name","cwd":fixture.root}]
    }]})).unwrap();
    let fixture = fixture.reopen_without_workspace_grant().await;
    let saved = fixture
        .desktop
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
        .unwrap();
    assert_eq!(saved[0]["terminals"][0]["title"], "My durable pane name");
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .unwrap();
    assert_eq!(created["title"], "My durable pane name");
    assert_eq!(
        fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .title,
        "My durable pane name"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_host_start_forgets_names_only_of_panes_the_roster_cannot_reopen() {
    let fixture = Fixture::new();
    let saved = uuid::Uuid::new_v4();
    let closed = uuid::Uuid::new_v4();
    for (wire_id, title) in [(saved, "Saved pane"), (closed, "Closed pane")] {
        let created = fixture
            .dispatcher
            .dispatch(
                CHANNELS.create_terminal,
                create_payload(wire_id, &fixture.root),
            )
            .await
            .unwrap();
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.rename_terminal,
                json!({"id": wire_id, "generation": created["generation"], "title": title}),
            )
            .await
            .unwrap();
    }
    fixture.desktop.dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces":[{
        "id":"saved", "name":"Workspace 1", "createdAt":"2026-10-05T00:00:00Z", "updatedAt":"2026-10-05T00:00:00Z",
        "terminals":[{"id":saved,"title":"Saved pane","cwd":fixture.root}]
    }]})).unwrap();
    let titles = |fixture: &Fixture| {
        interface_desktop_services::TerminalTitleStore::open(
            &fixture.desktop.terminal_titles_file(),
        )
        .unwrap()
    };
    assert!(titles(&fixture).get(TerminalId::new(closed)).is_some());

    let fixture = fixture.reopen_without_workspace_grant().await;
    let store = titles(&fixture);
    assert_eq!(
        store.get(TerminalId::new(saved)).as_deref(),
        Some("Saved pane")
    );
    assert_eq!(store.get(TerminalId::new(closed)), None);

    // A roster that cannot be read proves nothing, so every name stays.
    let pinned = uuid::Uuid::new_v4();
    titles(&fixture)
        .pin(TerminalId::new(pinned), "Not in the roster")
        .unwrap();
    fs::write(
        fixture.temporary.path().join("terminal-workspaces.json"),
        b"{ malformed",
    )
    .unwrap();
    let fixture = fixture.reopen_without_workspace_grant().await;
    assert_eq!(
        titles(&fixture).get(TerminalId::new(pinned)).as_deref(),
        Some("Not in the roster")
    );
    fixture.shutdown().await;
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_launches(TerminalLaunchCatalog::shell_only())
    }

    fn new_with_launches(launches: TerminalLaunchCatalog) -> Self {
        Self::new_with_launches_and_provider_sessions(launches, None)
    }

    fn new_with_launches_and_provider_sessions(
        launches: TerminalLaunchCatalog,
        provider_sessions: Option<Arc<dyn TerminalProviderSessionLookup>>,
    ) -> Self {
        Self::new_with_parts(
            launches,
            provider_sessions,
            TerminalServiceConfig::default(),
            Arc::new(FakeBackend::default()),
        )
    }

    fn new_with_parts(
        launches: TerminalLaunchCatalog,
        provider_sessions: Option<Arc<dyn TerminalProviderSessionLookup>>,
        config: TerminalServiceConfig,
        backend: Arc<FakeBackend>,
    ) -> Self {
        let temporary = TempDir::new().expect("temporary terminal host");
        let home = temporary.path().join("home");
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory");
        let default_root = std::fs::canonicalize(home.join("Desktop/Projects"))
            .expect("canonical default directory");
        std::fs::create_dir(&root).expect("workspace");
        let root = std::fs::canonicalize(root).expect("canonical workspace");
        let desktop = Arc::new(
            DesktopService::open(
                DesktopServiceConfig {
                    appearance_file: temporary.path().join("appearance.json"),
                    acp_profiles_file: temporary.path().join("acp-profiles.json"),
                    terminal_workspaces_file: temporary.path().join("terminal-workspaces.json"),
                    home_directory: home,
                    customization_user_root: None,
                    review_checkpoint_store_path: Some(temporary.path().join("review-checkpoints")),
                },
                DesktopServiceReviewPorts::default(),
            )
            .expect("desktop service"),
        );
        desktop
            .grant_workspace_root(&root)
            .expect("explicit root grant");
        let runtime = Arc::new(FakeRuntime::default());
        let releases = Arc::new(ReleaseRecorder::default());
        backend
            .expose_process_identity
            .store(provider_sessions.is_some(), Ordering::Release);
        let (push_sender, pushes) = mpsc::channel(32);
        let (electron_input, host_input) = tokio::io::duplex(64 * 1024);
        let (host_output, electron_output) = tokio::io::duplex(64 * 1024);
        let mut plane = TerminalDataPlane::start(
            host_input,
            host_output,
            TerminalDataTransportConfig::default(),
        );
        let dispatcher = Arc::new(
            TerminalHostDispatcher::with_backend_launches_and_claude_store(
                runtime,
                Arc::clone(&desktop),
                releases.clone(),
                push_sender,
                Some(plane.outbound.clone()),
                config,
                backend.clone(),
                launches,
                None,
                provider_sessions,
            )
            .expect("terminal dispatcher"),
        );
        let failures = plane.subscribe_failures();
        let (_unused_sender, empty_receiver) = mpsc::channel(1);
        let inbound = std::mem::replace(&mut plane.inbound, empty_receiver);
        dispatcher.start_input_pump(inbound).expect("input pump");
        dispatcher
            .start_failure_monitor(failures)
            .expect("failure monitor");
        Self {
            temporary,
            root,
            default_root,
            desktop,
            backend,
            releases,
            pushes,
            dispatcher,
            plane: Some(plane),
            electron_input,
            electron_output,
        }
    }

    async fn shutdown(mut self) {
        self.dispatcher.close().await;
        if let Some(plane) = self.plane.take() {
            plane.shutdown().await;
        }
    }

    async fn reopen_without_workspace_grant(self) -> Self {
        self.reopen_without_workspace_grant_with_launches(TerminalLaunchCatalog::shell_only())
            .await
    }

    async fn reopen_without_workspace_grant_with_launches(
        self,
        launches: TerminalLaunchCatalog,
    ) -> Self {
        self.reopen_with_provider_capture(launches, None).await
    }

    async fn reopen_with_provider_capture(
        mut self,
        launches: TerminalLaunchCatalog,
        provider_sessions: Option<Arc<dyn TerminalProviderSessionLookup>>,
    ) -> Self {
        self.dispatcher.close().await;
        if let Some(plane) = self.plane.take() {
            plane.shutdown().await;
        }
        let home = self.temporary.path().join("home");
        let desktop = Arc::new(
            DesktopService::open(
                DesktopServiceConfig {
                    appearance_file: self.temporary.path().join("appearance.json"),
                    acp_profiles_file: self.temporary.path().join("acp-profiles.json"),
                    terminal_workspaces_file: self
                        .temporary
                        .path()
                        .join("terminal-workspaces.json"),
                    home_directory: home,
                    customization_user_root: None,
                    review_checkpoint_store_path: Some(
                        self.temporary.path().join("review-checkpoints"),
                    ),
                },
                DesktopServiceReviewPorts::default(),
            )
            .expect("reopened desktop service"),
        );
        let (push_sender, pushes) = mpsc::channel(32);
        let (electron_input, host_input) = tokio::io::duplex(64 * 1024);
        let (host_output, electron_output) = tokio::io::duplex(64 * 1024);
        let mut plane = TerminalDataPlane::start(
            host_input,
            host_output,
            TerminalDataTransportConfig::default(),
        );
        let dispatcher = Arc::new(
            TerminalHostDispatcher::with_backend_launches_and_claude_store(
                Arc::new(FakeRuntime::default()),
                Arc::clone(&desktop),
                self.releases.clone(),
                push_sender,
                Some(plane.outbound.clone()),
                TerminalServiceConfig::default(),
                self.backend.clone(),
                launches,
                None,
                provider_sessions,
            )
            .expect("reopened terminal dispatcher"),
        );
        let failures = plane.subscribe_failures();
        let (_unused_sender, empty_receiver) = mpsc::channel(1);
        let inbound = std::mem::replace(&mut plane.inbound, empty_receiver);
        dispatcher.start_input_pump(inbound).expect("input pump");
        dispatcher
            .start_failure_monitor(failures)
            .expect("failure monitor");
        self.desktop = desktop;
        self.pushes = pushes;
        self.dispatcher = dispatcher;
        self.plane = Some(plane);
        self.electron_input = electron_input;
        self.electron_output = electron_output;
        self
    }
}

async fn read_data_frame<R>(reader: &mut R, decoder: &mut TerminalDataDecoder) -> TerminalDataFrame
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0_u8; 1024];
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let count = reader.read(&mut bytes).await.expect("terminal output read");
            assert_ne!(count, 0, "terminal output pipe ended");
            let frames = decoder
                .push(&bytes[..count])
                .expect("terminal output frame");
            if let Some(frame) = frames.into_iter().next() {
                return frame;
            }
        }
    })
    .await
    .expect("terminal output timeout")
}

async fn wait_for_state(dispatcher: &TerminalHostDispatcher, state: TerminalState) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = dispatcher
                .dispatch(CHANNELS.list_terminals, Value::Null)
                .await
                .expect("terminal snapshot");
            if snapshot["terminals"][0]["state"] == json!(state) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("terminal lifecycle timeout")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn the_pane_mcp_server_refuses_a_caller_that_is_not_under_a_pane() {
    use crate::mcp_transport::test_support::exchange;
    use crate::pane_mcp::PaneMcpServer;
    use tokio::io::AsyncBufReadExt as _;

    let fixture = Fixture::new();
    let server = PaneMcpServer::start_with_reports(
        fixture.temporary.path(),
        fixture.dispatcher.clone(),
        None,
    )
    .expect("pane socket");
    let socket = fixture.temporary.path().join("panes/iface.sock");
    let stream = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(reader).lines();
    let initialized = exchange(&mut writer, &mut lines, json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}
    }))
    .await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "iface");
    assert_eq!(initialized["result"]["instructions"], "");
    assert!(!fixture.temporary.path().join("panes/delegates").exists());
    let listed = exchange(
        &mut writer,
        &mut lines,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await;
    assert_eq!(
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["pass_off", "read_pane", "send_to_pane"]
    );
    // This test process is nobody's pane child, so the kernel-named peer
    // resolves to no pane and the pass-off is refused, not guessed.
    let refused = exchange(
        &mut writer,
        &mut lines,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "pass_off", "arguments": {"profile": "codex", "prompt": "do X"}}
        }),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true);
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("could not tell which pane called"),
        "{refused}"
    );
    let bad_profile = exchange(
        &mut writer,
        &mut lines,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "pass_off", "arguments": {"profile": "shell", "prompt": "do X"}}
        }),
    )
    .await;
    assert!(
        bad_profile["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown profile shell"),
        "{bad_profile}"
    );
    let unknown_pane = exchange(
        &mut writer,
        &mut lines,
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "read_pane", "arguments": {"pane_id": "nope"}}
        }),
    )
    .await;
    assert_eq!(unknown_pane["result"]["isError"], true);
    // Typing into a pane needs an identified caller on a pass-off edge; this
    // process is nobody's pane, so the send is refused before any lookup.
    let typed = exchange(
        &mut writer,
        &mut lines,
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "send_to_pane", "arguments": {
                "pane_id": uuid::Uuid::new_v4().to_string(), "text": "rm -rf /"
            }}
        }),
    )
    .await;
    assert_eq!(typed["result"]["isError"], true);
    assert!(
        typed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("could not tell which pane called"),
        "{typed}"
    );
    server.stop();
    assert!(!socket.exists());
    fixture.shutdown().await;
}

fn create_payload(id: uuid::Uuid, root: &Path) -> Value {
    json!({
        "id": id,
        "cwd": root,
        "size": {"columns": 90, "rows": 28},
        "title": "Build"
    })
}

#[tokio::test]
async fn native_reopen_resumes_the_exact_agy_and_muse_session() {
    struct Lookup(String);
    impl TerminalProviderSessionLookup for Lookup {
        fn current_session(
            &self,
            _: TerminalLaunchProfileId,
            _: &TerminalProcessIdentity,
            _: &str,
        ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
            Ok(Some(TerminalProviderSession {
                id: self.0.clone(),
                transition: TerminalProviderSessionTransition::Resume,
            }))
        }
    }
    for (profile, binding, raw_id, flag) in [
        (
            TerminalLaunchProfileId::Gemini,
            "agy-d2d9dfef-c119-4df6-8827-2bad4e79eda6",
            "d2d9dfef-c119-4df6-8827-2bad4e79eda6",
            "--conversation",
        ),
        (
            TerminalLaunchProfileId::Muse,
            "muse-01a15add-7a55-74d1-bf33-5d8d5502c449",
            "01a15add-7a55-74d1-bf33-5d8d5502c449",
            "resume",
        ),
    ] {
        let directory = TempDir::new().unwrap();
        let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
            profile,
            executable(&directory, "native"),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
        .with_provider_session_capture()
        .unwrap()])
        .unwrap();
        let lookup: Arc<dyn TerminalProviderSessionLookup> = Arc::new(Lookup(binding.into()));
        let backend = Arc::new(FakeBackend::default());
        backend
            .expose_process_identity
            .store(true, Ordering::Release);
        let fixture = Fixture::new_with_parts(
            catalog.clone(),
            Some(lookup.clone()),
            TerminalServiceConfig::default(),
            backend,
        );
        let id = uuid::Uuid::new_v4();
        let mut request = create_payload(id, &fixture.root);
        request["launch"] = json!({"version":1,"profileId":profile});
        request["launchSession"] = json!({"version":1,"id":binding,"resume":true});
        fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, request.clone())
            .await
            .unwrap();
        save_roster_terminal(
            &fixture,
            &json!({"id":id,"cwd":fixture.root,"title":"Native session","launch":request["launch"],"launchSession":request["launchSession"]}),
        );
        let fixture = fixture
            .reopen_with_provider_capture(catalog, Some(lookup))
            .await;
        fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, request)
            .await
            .unwrap();
        let specs = mutex_lock(&fixture.backend.specs).clone();
        assert_eq!(specs.len(), 2);
        let ResolvedTerminalLaunch::Program { arguments, .. } = &specs[1].launch else {
            panic!("program")
        };
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == [OsString::from(flag), OsString::from(raw_id)])
        );
        assert!(mutex_lock(&fixture.backend.process(1).input).is_empty());
        fixture.shutdown().await;
    }
}

// Protocol v55: a Kimi pane whose plugin hook reported `session_<uuid>`
// reopens that exact session after a host restart.
#[tokio::test]
async fn native_reopen_resumes_the_exact_kimi_session() {
    struct Lookup;
    impl TerminalProviderSessionLookup for Lookup {
        fn current_session(
            &self,
            _: TerminalLaunchProfileId,
            _: &TerminalProcessIdentity,
            _: &str,
        ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
            Ok(Some(TerminalProviderSession {
                id: "kimi-3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73".into(),
                transition: TerminalProviderSessionTransition::Resume,
            }))
        }
    }
    let directory = TempDir::new().unwrap();
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Kimi,
        executable(&directory, "kimi"),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
    .with_provider_session_capture()
    .unwrap()])
    .unwrap();
    let lookup: Arc<dyn TerminalProviderSessionLookup> = Arc::new(Lookup);
    let backend = Arc::new(FakeBackend::default());
    backend
        .expose_process_identity
        .store(true, Ordering::Release);
    let fixture = Fixture::new_with_parts(
        catalog.clone(),
        Some(lookup.clone()),
        TerminalServiceConfig::default(),
        backend,
    );
    let id = uuid::Uuid::new_v4();
    let mut request = create_payload(id, &fixture.root);
    request["launch"] = json!({"version":1,"profileId":"kimi"});
    request["launchSession"] =
        json!({"version":1,"id":"kimi-3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73","resume":true});
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request.clone())
        .await
        .unwrap();
    save_roster_terminal(
        &fixture,
        &json!({"id":id,"cwd":fixture.root,"title":"Kimi Code","launch":request["launch"],"launchSession":request["launchSession"]}),
    );
    let fixture = fixture
        .reopen_with_provider_capture(catalog, Some(lookup))
        .await;
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .unwrap();
    let specs = mutex_lock(&fixture.backend.specs).clone();
    assert_eq!(specs.len(), 2);
    let ResolvedTerminalLaunch::Program { arguments, .. } = &specs[1].launch else {
        panic!("program")
    };
    assert_eq!(
        arguments,
        &[
            OsString::from("--session"),
            OsString::from("session_3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73")
        ]
    );
    assert!(mutex_lock(&fixture.backend.process(1).input).is_empty());
    fixture.shutdown().await;
}

fn executable(directory: &TempDir, name: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fake executable");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
    path
}

#[test]
fn legacy_claude_binding_uses_picker_only_when_the_exact_uuid_is_absent() {
    const SAVED: &str = "11111111-1111-4111-8111-111111111111";

    let projects = TempDir::new().expect("Claude projects root");
    let mut wire = create_payload(uuid::Uuid::new_v4(), Path::new("/tmp"));
    wire["launch"] = json!({"version": 1, "profileId": "claude"});
    wire["launchSession"] = json!({"version": 1, "id": SAVED, "resume": true});
    let request: CreateTerminalRequest = serde_json::from_value(wire).expect("request");

    let recovered = recover_legacy_claude_binding(request.clone(), Some(projects.path()))
        .expect("missing session becomes picker");
    assert_eq!(recovered.launch_session.expect("picker binding").id, None);

    let project = projects.path().join("-tmp-worktree");
    fs::create_dir(&project).expect("project directory");
    fs::write(project.join(format!("{SAVED}.jsonl")), b"fixture\n").expect("saved transcript");
    assert_eq!(
        recover_legacy_claude_binding(request, Some(projects.path()))
            .expect("saved session remains exact")
            .launch_session
            .expect("saved binding")
            .id
            .as_deref(),
        Some(SAVED)
    );

    let unsafe_root = projects.path().join("not-a-directory");
    fs::write(&unsafe_root, b"unsafe root").expect("unsafe projects root");
    let mut wire = create_payload(uuid::Uuid::new_v4(), Path::new("/tmp"));
    wire["launch"] = json!({"version": 1, "profileId": "claude"});
    wire["launchSession"] = json!({"version": 1, "id": SAVED, "resume": true});
    let request = serde_json::from_value(wire).expect("request");
    assert_eq!(
        recover_legacy_claude_binding(request, Some(&unsafe_root)),
        Err(TerminalError::ProviderSessionLookupFailed(
            TerminalLaunchProfileId::Claude
        ))
    );
}

#[tokio::test]
async fn begin_authority_transaction_rejects_a_receipt_advance_during_verification() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created: TerminalSummary = serde_json::from_value(
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.create_terminal,
                create_payload(wire_id, &fixture.root),
            )
            .await
            .expect("create visible terminal"),
    )
    .expect("created terminal summary");
    let prior = fixture
        .desktop
        .live_terminal_reopen_authority(id)
        .expect("authority store")
        .expect("initial authority");

    // Model the exact verification race: the caller still holds A while a
    // provider receipt advances the durable slot to B before begin.
    let advanced_session = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("22222222-2222-4222-8222-222222222222".to_owned()),
        resume: true,
    };
    let prior_generation = created.generation;
    let mut advanced = created;
    advanced.launch_session = Some(advanced_session.clone());
    fixture
        .desktop
        .record_terminal_reopen_authority(&advanced)
        .expect("advance authority with provider receipt");

    let error = fixture
        .dispatcher
        .begin_reopen_authority_commit(
            id,
            Some(prior),
            ReopenAuthorityTransactionKind::Restart { prior_generation },
        )
        .await
        .expect_err("stale expected authority must not start a transaction");
    assert_eq!(error.code, "terminal-authority-changed");
    assert!(
        !fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id)
    );
    assert_eq!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("authority remains available")
            .expect("advanced authority remains")
            .terminal
            .launch_session,
        Some(advanced_session)
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn restart_settles_pending_final_capture_before_authority_transaction() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "capturing-claude-executable");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable,
        vec![OsString::from(format!(
            "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .expect("Claude launch program")
    .with_provider_session_capture()
    .expect("capturing Claude launch program")])
    .expect("launch catalog");
    let lookup = Arc::new(RestartCaptureLookup {
        fail_end_once: AtomicBool::new(true),
        ended: Mutex::new(Vec::new()),
    });
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = lookup.clone();
    let fixture =
        Fixture::new_with_launches_and_provider_sessions(catalog, Some(provider_sessions));
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let mut create = create_payload(wire_id, &fixture.root);
    create["launch"] = json!({"version": 1, "profileId": "claude"});
    create["launchSession"] = json!({
        "version": 1,
        "id": "11111111-1111-4111-8111-111111111111",
        "resume": true
    });
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create)
        .await
        .expect("create capturing Claude terminal");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect("close first generation");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    assert!(
        mutex_lock(&lookup.ended).is_empty(),
        "the planned first retirement failure must leave capture pending"
    );

    let restarted = fixture
        .dispatcher
        .dispatch(
            CHANNELS.restart_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect("host preflight settles capture without transaction retry loop");
    assert_eq!(restarted["generation"], 2);
    assert_eq!(mutex_lock(&lookup.ended).len(), 1);
    assert_eq!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("authority store")
            .expect("restarted terminal authority")
            .terminal
            .launch_session
            .and_then(|session| session.id),
        Some("44444444-4444-4444-8444-444444444444".to_owned())
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn host_authoritative_provider_binding_keeps_cold_create_retry_idempotent() {
    const ROSTER_SESSION: &str = "11111111-1111-4111-8111-111111111111";
    const AUTHORITY_SESSION: &str = "22222222-2222-4222-8222-222222222222";

    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "fixed-claude-executable");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable,
        Vec::new(),
        Vec::new(),
    )
    .expect("Claude launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let root = fixture.root.to_string_lossy().into_owned();
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved",
                    "name": "Saved",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": wire_id,
                        "title": "Build",
                        "cwd": root,
                        "launch": {"version": 1, "profileId": "claude"},
                        "launchSession": {
                            "version": 1,
                            "id": ROSTER_SESSION,
                            "resume": true
                        }
                    }]
                }]
            }),
        )
        .expect("save renderer roster");
    let authority = TerminalReopenAuthority {
        root: root.clone(),
        managed_worktree_key: None,
        terminal: TerminalWorkspaceTerminal {
            id,
            title: "Build".to_owned(),
            cwd: root,
            session_key: None,
            launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            launch_session: Some(TerminalLaunchSessionV1 {
                version: 1,
                id: Some(AUTHORITY_SESSION.to_owned()),
                resume: true,
            }),
        },
    };
    assert_eq!(
        fixture
            .desktop
            .compare_exchange_terminal_reopen_authority(
                id,
                &TerminalReopenAuthoritySlot::Absent,
                &TerminalReopenAuthoritySlot::Authority(authority),
            )
            .expect("install host authority"),
        TerminalReopenAuthorityCompareExchange::Exchanged
    );

    let mut request = create_payload(wire_id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    request["launchSession"] = json!({
        "version": 1,
        "id": ROSTER_SESSION,
        "resume": true
    });
    let first = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request.clone())
        .await
        .expect("first host-authorized cold create");
    assert_eq!(first["launchSession"]["id"], AUTHORITY_SESSION);

    let replay = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("lost-response retry with stale renderer binding");
    assert_eq!(replay, first);
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_capture_retirement_keeps_hidden_create_fenced_until_cleanup() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "capturing-claude-executable");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable,
        vec![OsString::from(format!(
            "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .expect("Claude launch program")
    .with_provider_session_capture()
    .expect("capturing Claude launch program")])
    .expect("launch catalog");
    let lookup = Arc::new(GatedCaptureRetirementLookup::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = lookup.clone();
    let fixture =
        Fixture::new_with_launches_and_provider_sessions(catalog, Some(provider_sessions));
    fixture
        .releases
        .unavailable_on_verification_call
        .store(2, Ordering::Release);
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let mut request = create_payload(wire_id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});

    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request.clone())
        .await
        .expect_err("post-spawn verification must roll back");
    assert_eq!(error.code, "terminal-authority-changed");
    assert!(lookup.end_calls.load(Ordering::Acquire) > 0);
    assert!(
        fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id),
        "failed receipt retirement must retain rollback ownership"
    );
    assert!(
        fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .any(|terminal| terminal.id == id),
        "the hidden entry must remain generation-fenced until capture cleanup succeeds"
    );
    let retry = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request.clone())
        .await
        .expect_err("hidden entry cannot be returned as an exact replay");
    assert_eq!(retry.code, "terminal-authority-busy");

    lookup.allow_end.store(true, Ordering::Release);
    eventually("background rollback cleanup", || {
        let transaction_present = fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id);
        let entry_present = fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .any(|terminal| terminal.id == id);
        !transaction_present && !entry_present
    })
    .await;
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("same stable ID can be retried after safe abandonment");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_dismiss_restores_exact_authority_before_retry() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "capturing-claude-executable");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable,
        vec![OsString::from(format!(
            "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .expect("Claude launch program")
    .with_provider_session_capture()
    .expect("capturing Claude launch program")])
    .expect("launch catalog");
    let lookup = Arc::new(RestartCaptureLookup::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = lookup.clone();
    let fixture =
        Fixture::new_with_launches_and_provider_sessions(catalog, Some(provider_sessions));
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let mut request = create_payload(wire_id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("create capturing Claude terminal");
    lookup.fail_end_once.store(true, Ordering::Release);
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect("close terminal");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    let prior = fixture
        .desktop
        .terminal_reopen_authority_slot(id)
        .expect("capture exact prior authority");

    lookup.fail_end_once.store(true, Ordering::Release);
    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
        .await
        .expect_err("planned capture retirement failure");
    assert_eq!(error.code, "terminal-session-capture-unavailable");
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(id)
            .expect("authority after failed dismissal"),
        prior
    );
    assert!(
        fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .any(|terminal| terminal.id == id)
    );
    assert!(
        !fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id)
    );

    let dismissed = fixture
        .dispatcher
        .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
        .await
        .expect("retry dismissal");
    assert_eq!(dismissed["removed"], true);
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(id)
            .expect("authority after dismissal"),
        TerminalReopenAuthoritySlot::Absent
    );
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    fixture.shutdown().await;
}

async fn stopped_workspace_terminal() -> (Fixture, Arc<RestartCaptureLookup>, uuid::Uuid) {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable(&launch_directory, "capturing-claude-executable"),
        vec![OsString::from(format!(
            "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .expect("Claude launch program")
    .with_provider_session_capture()
    .expect("capturing Claude launch program")])
    .expect("launch catalog");
    let lookup = Arc::new(RestartCaptureLookup::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = lookup.clone();
    let fixture =
        Fixture::new_with_launches_and_provider_sessions(catalog, Some(provider_sessions));
    let wire_id = uuid::Uuid::new_v4();
    let mut request = create_payload(wire_id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("create terminal");
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces": [{
                "id": "deleted", "name": "Deleted workspace",
                "createdAt": "2026-09-21T00:00:00Z",
                "updatedAt": "2026-09-21T00:00:00Z",
                "terminals": [{"id": wire_id, "title": "Claude", "cwd": fixture.root}]
            }]}),
        )
        .expect("save workspace");
    // Retain capture cleanup for dismissal instead of completing it at exit.
    lookup.fail_end_once.store(true, Ordering::Release);
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": 1}),
        )
        .await
        .expect("close terminal");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    (fixture, lookup, wire_id)
}

#[derive(Clone, Copy)]
enum WorkspaceDismissRace {
    Delete,
    DeleteWithCleanupFailure,
    ReplaceAuthority,
}

async fn dismiss_during_workspace_change(race: WorkspaceDismissRace) {
    let (fixture, lookup, wire_id) = stopped_workspace_terminal().await;
    let id = TerminalId::new(wire_id);
    let fail_capture_cleanup = matches!(race, WorkspaceDismissRace::DeleteWithCleanupFailure);
    lookup
        .fail_end_once
        .store(fail_capture_cleanup, Ordering::Release);
    let prior = fixture.desktop.terminal_reopen_authority_slot(id).unwrap();
    assert_ne!(prior, TerminalReopenAuthoritySlot::Absent);

    // The renderer sends both requests. Hold the dismissal after it captures
    // authority, then let the workspace save revoke that exact authority.
    let permits = Arc::clone(&fixture.dispatcher.blocking_gate)
        .acquire_many_owned(TERMINAL_BLOCKING_CONCURRENCY_PERMITS)
        .await
        .expect("hold dismissal worker");
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let dismiss = tokio::spawn(async move {
        dispatcher
            .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
            .await
    });
    eventually("dismissal captured authority", || {
        mutex_lock(&fixture.dispatcher.lifecycle_sink.state)
            .transactions
            .contains_key(&id)
    })
    .await;
    let expected = if matches!(race, WorkspaceDismissRace::ReplaceAuthority) {
        let TerminalReopenAuthoritySlot::Authority(mut replacement) = prior.clone() else {
            panic!("created terminal must have authority");
        };
        replacement.terminal.title = "Newer host-owned identity".to_owned();
        let replacement = TerminalReopenAuthoritySlot::Authority(replacement);
        assert_eq!(
            fixture
                .desktop
                .compare_exchange_terminal_reopen_authority(id, &prior, &replacement)
                .unwrap(),
            TerminalReopenAuthorityCompareExchange::Exchanged
        );
        replacement
    } else {
        fixture
            .desktop
            .dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces": []}))
            .expect("delete saved workspace");
        TerminalReopenAuthoritySlot::Absent
    };
    drop(permits);
    let outcome = dismiss.await.expect("dismissal task");
    if matches!(race, WorkspaceDismissRace::ReplaceAuthority) {
        let error = outcome.expect_err("changed authority must still refuse dismissal");
        assert_eq!(error.code, "terminal-authority-changed");
        assert!(error.replay_safe);
        assert_eq!(fixture.dispatcher.service.snapshot().terminals.len(), 1);
    } else if fail_capture_cleanup {
        assert_eq!(
            outcome.unwrap_err().code,
            "terminal-session-capture-unavailable"
        );
        assert_eq!(fixture.dispatcher.service.snapshot().terminals.len(), 1);
    } else {
        assert_eq!(
            outcome.expect("dismiss after workspace deletion")["removed"],
            true
        );
        assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    }
    assert_eq!(
        fixture.desktop.terminal_reopen_authority_slot(id).unwrap(),
        expected,
        "dismissal must preserve the workspace writer's authority change"
    );
    assert!(
        mutex_lock(&fixture.dispatcher.lifecycle_sink.state)
            .transactions
            .is_empty()
    );
    if fail_capture_cleanup {
        let retried = fixture
            .dispatcher
            .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
            .await
            .expect("explicit retry after capture cleanup failure");
        assert_eq!(retried["removed"], true);
    }
    fixture.shutdown().await;
}

#[tokio::test]
async fn workspace_delete_can_revoke_authority_before_terminal_dismissal() {
    dismiss_during_workspace_change(WorkspaceDismissRace::Delete).await;
}

#[tokio::test]
async fn workspace_delete_stays_revoked_after_dismissal_cleanup_failure() {
    dismiss_during_workspace_change(WorkspaceDismissRace::DeleteWithCleanupFailure).await;
}

#[tokio::test]
async fn workspace_change_cannot_dismiss_a_newer_terminal_authority() {
    dismiss_during_workspace_change(WorkspaceDismissRace::ReplaceAuthority).await;
}

#[tokio::test]
async fn close_fences_a_create_paused_in_workspace_verification() {
    let fixture = Fixture::new();
    fixture
        .releases
        .block_on_verification_call
        .store(1, Ordering::Release);
    let wire_id = uuid::Uuid::new_v4();
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let request = create_payload(wire_id, &fixture.root);
    let create =
        tokio::spawn(async move { dispatcher.dispatch(CHANNELS.create_terminal, request).await });
    eventually("workspace verifier entered", || {
        fixture
            .releases
            .verification_blocked
            .load(Ordering::Acquire)
    })
    .await;

    let dispatcher = Arc::clone(&fixture.dispatcher);
    let close = tokio::spawn(async move { dispatcher.close().await });
    eventually("shutdown fence installed", || {
        fixture.dispatcher.closed.load(Ordering::Acquire)
    })
    .await;
    fixture
        .releases
        .release_verification
        .store(true, Ordering::Release);
    fixture.releases.verification_changed.notify_waiters();

    let error = create
        .await
        .expect("create task")
        .expect_err("delayed create must observe shutdown");
    assert_eq!(error.code, "terminal-host-closed");
    close.await.expect("close task");
    assert!(mutex_lock(&fixture.backend.processes).is_empty());
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(TerminalId::new(wire_id))
            .expect("authority after fenced create"),
        TerminalReopenAuthoritySlot::Absent
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_retries_a_transient_process_termination_failure() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let process = fixture.backend.process(0);
    process.terminate_failures.store(1, Ordering::Release);

    tokio::time::timeout(Duration::from_secs(2), fixture.dispatcher.close())
        .await
        .expect("shutdown must retry rather than hang");
    assert!(*mutex_lock(&process.stopped));
    assert!(process.terminate_calls.load(Ordering::Acquire) >= 2);
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_recovers_a_create_cancelled_after_spawn() {
    let fixture = Fixture::new();
    fixture
        .releases
        .block_on_verification_call
        .store(2, Ordering::Release);
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let request = create_payload(wire_id, &fixture.root);
    let create =
        tokio::spawn(async move { dispatcher.dispatch(CHANNELS.create_terminal, request).await });
    eventually("post-spawn verification entered", || {
        fixture
            .releases
            .verification_blocked
            .load(Ordering::Acquire)
    })
    .await;
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    assert!(
        fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id)
    );

    create.abort();
    assert!(
        create
            .await
            .expect_err("create task cancelled")
            .is_cancelled()
    );
    tokio::time::timeout(Duration::from_secs(2), fixture.dispatcher.close())
        .await
        .expect("shutdown actively recovers the orphaned transaction");

    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(*mutex_lock(&fixture.backend.process(0).stopped));
    assert!(
        !fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id)
    );
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(id)
            .expect("authority after cancelled create"),
        TerminalReopenAuthoritySlot::Absent
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_recovers_a_restart_cancelled_after_spawn() {
    let fixture = Fixture::new();
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created: TerminalSummary = serde_json::from_value(
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.create_terminal,
                create_payload(wire_id, &fixture.root),
            )
            .await
            .expect("create managed terminal"),
    )
    .expect("created terminal summary");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": created.generation}),
        )
        .await
        .expect("settle first generation");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    let prior = fixture
        .desktop
        .terminal_reopen_authority_slot(id)
        .expect("authority before restart");
    let block_on = fixture.releases.verification_calls.load(Ordering::Acquire) + 2;
    fixture
        .releases
        .block_on_verification_call
        .store(block_on, Ordering::Release);
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let restart = tokio::spawn(async move {
        dispatcher
            .dispatch(
                CHANNELS.restart_terminal,
                serde_json::to_value(TerminalGenerationRequest {
                    id,
                    generation: created.generation,
                })
                .expect("restart payload"),
            )
            .await
    });
    eventually("post-restart verification entered", || {
        fixture
            .releases
            .verification_blocked
            .load(Ordering::Acquire)
    })
    .await;
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 2);

    restart.abort();
    assert!(
        restart
            .await
            .expect_err("restart task cancelled")
            .is_cancelled()
    );
    tokio::time::timeout(Duration::from_secs(2), fixture.dispatcher.close())
        .await
        .expect("shutdown actively recovers the orphaned restart");
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(id)
            .expect("authority after cancelled restart"),
        prior
    );
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(
        fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .is_empty()
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_recovers_a_dismiss_cancelled_before_its_worker() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let prior = fixture
        .desktop
        .terminal_reopen_authority_slot(id)
        .expect("authority before dismissal");
    let permits = Arc::clone(&fixture.dispatcher.blocking_gate)
        .acquire_many_owned(TERMINAL_BLOCKING_CONCURRENCY_PERMITS)
        .await
        .expect("hold every blocking permit");
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let dismiss = tokio::spawn(async move {
        dispatcher
            .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
            .await
    });
    eventually("dismissal transaction installed", || {
        fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .contains_key(&id)
    })
    .await;
    dismiss.abort();
    assert!(
        dismiss
            .await
            .expect_err("dismiss task cancelled")
            .is_cancelled()
    );
    drop(permits);

    tokio::time::timeout(Duration::from_secs(2), fixture.dispatcher.close())
        .await
        .expect("shutdown rolls back the ownerless dismissal");
    assert_eq!(
        fixture
            .desktop
            .terminal_reopen_authority_slot(id)
            .expect("authority after cancelled dismissal"),
        prior
    );
    assert!(
        fixture
            .dispatcher
            .lifecycle_sink
            .state
            .lock()
            .expect("lifecycle state")
            .transactions
            .is_empty()
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_unblocks_an_output_pump_behind_a_full_queue() {
    let fixture = Fixture::new();
    let (electron_input, host_input) = tokio::io::duplex(1024);
    let frame_bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    let plane = TerminalDataPlane::start(
        host_input,
        PendingWriter,
        TerminalDataTransportConfig {
            queue_limits: TerminalQueueLimits {
                total_bytes: frame_bytes,
                per_terminal_bytes: frame_bytes,
            },
            ..TerminalDataTransportConfig::default()
        },
    );
    let sender = plane.outbound.clone();
    let (push_sender, _pushes) = mpsc::channel(4);
    let dispatcher = Arc::new(
        TerminalHostDispatcher::with_backend_launches_and_claude_store(
            Arc::new(FakeRuntime::default()),
            Arc::clone(&fixture.desktop),
            fixture.releases.clone(),
            push_sender,
            Some(sender.clone()),
            TerminalServiceConfig::default(),
            fixture.backend.clone(),
            TerminalLaunchCatalog::shell_only(),
            None,
            None,
        )
        .expect("terminal dispatcher"),
    );
    let wire_id = uuid::Uuid::new_v4();
    let frame = |sequence| {
        TerminalDataFrame::new(
            wire_id,
            TerminalGeneration::initial().0,
            sequence,
            TerminalDataDirection::HostToElectron,
            TerminalDataKind::Data,
            vec![b'x'],
        )
        .expect("valid output frame")
    };
    let first_frame = frame(0);
    let second_frame = frame(1);
    let third_frame = frame(2);
    sender
        .enqueue_output(first_frame)
        .expect("first output enters queue");
    let second_sender = sender.clone();
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            second_sender.enqueue_output_waiting(second_frame, Duration::from_millis(5))
        }),
    )
    .await
    .expect("writer takes the first frame")
    .expect("second enqueue worker")
    .expect("second frame fills the queue");

    let blocked = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let blocked_for_pump = Arc::clone(&blocked);
    let finished_for_pump = Arc::clone(&finished);
    let pump_sender = sender;
    let pump = std::thread::spawn(move || {
        blocked_for_pump.store(true, Ordering::Release);
        let result = pump_sender.enqueue_output_waiting(third_frame, Duration::from_millis(5));
        assert!(matches!(result, Err(TerminalDataSendError::Closed)));
        finished_for_pump.store(true, Ordering::Release);
    });
    mutex_lock(&dispatcher.output_pumps).insert(
        (TerminalId::new(wire_id), TerminalGeneration::initial()),
        TerminalOutputPump {
            task: pump,
            admission: None,
        },
    );
    eventually("third frame reaches queue backpressure", || {
        blocked.load(Ordering::Acquire)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!finished.load(Ordering::Acquire));

    tokio::time::timeout(Duration::from_secs(2), dispatcher.close())
        .await
        .expect("shutdown closes the queue before awaiting output pumps");
    assert!(finished.load(Ordering::Acquire));

    drop(electron_input);
    drop(plane);
    fixture.shutdown().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn pending_spawn_is_hidden_and_commits_the_latest_buffered_summary() {
    let mut fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    fixture
        .dispatcher
        .begin_reopen_authority_commit(id, None, ReopenAuthorityTransactionKind::Create)
        .await
        .expect("begin authority transaction");
    let roots = ApprovedTerminalRoots::new([fixture.root.clone()]).expect("approved root");
    let request: CreateTerminalRequest =
        serde_json::from_value(create_payload(wire_id, &fixture.root)).expect("create request");
    let created = fixture
        .dispatcher
        .service
        .create(request, &roots)
        .expect("create under transaction");
    let renamed = fixture
        .dispatcher
        .service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: "Latest receipt title".to_owned(),
        })
        .expect("advance summary while commit is pending");

    for kind in [TerminalDataKind::Data, TerminalDataKind::Eof] {
        fixture.dispatcher.route_input_frame(
            TerminalDataFrame::new(
                wire_id,
                created.generation.0,
                0,
                TerminalDataDirection::ElectronToHost,
                kind,
                if kind == TerminalDataKind::Data {
                    b"must-not-run\n".to_vec()
                } else {
                    Vec::new()
                },
            )
            .expect("uncommitted input frame"),
        );
    }
    assert!(
        !mutex_lock(&fixture.dispatcher.lifecycle_sink.input_lanes).contains_key(&id),
        "hidden generations must not gain an input lane"
    );
    assert!(mutex_lock(&fixture.backend.process(0).input).is_empty());
    assert!(!*mutex_lock(&fixture.backend.process(0).stopped));

    let hidden = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, Value::Null)
        .await
        .expect("filtered snapshot");
    assert_eq!(hidden["terminals"], json!([]));
    assert!(matches!(
        fixture.pushes.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let retry_error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect_err("an in-flight authority transaction is not an exact replay");
    assert_eq!(retry_error.code, "terminal-authority-busy");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    let dismiss_error = fixture
        .dispatcher
        .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
        .await
        .expect_err("rollback-owned terminal cannot be dismissed externally");
    assert_eq!(dismiss_error.code, "terminal-authority-busy");

    let other_wire_id = uuid::Uuid::new_v4();
    let other = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(other_wire_id, &fixture.root),
        )
        .await
        .expect("unrelated terminal create");
    let unrelated = fixture.pushes.recv().await.expect("unrelated event");
    assert_eq!(unrelated.payload["revision"], 1);
    assert_eq!(unrelated.payload["terminal"]["id"], other["id"]);
    let independently_visible = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, Value::Null)
        .await
        .expect("independent snapshot");
    assert_eq!(
        independently_visible["terminals"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(independently_visible["terminals"][0]["id"], other["id"]);

    let committed = fixture
        .dispatcher
        .commit_reopen_authority(&created, None)
        .await
        .expect("commit latest summary");
    assert_eq!(committed.title, renamed.title);
    let authority = fixture
        .desktop
        .live_terminal_reopen_authority(id)
        .expect("authority store")
        .expect("committed authority");
    assert_eq!(authority.terminal.title, renamed.title);

    let committed_event = fixture.pushes.recv().await.expect("committed event");
    assert_eq!(committed_event.payload["revision"], 2);
    assert_eq!(committed_event.payload["terminal"]["title"], renamed.title);
    assert!(matches!(
        fixture.pushes.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let visible = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, Value::Null)
        .await
        .expect("committed snapshot");
    assert_eq!(visible["revision"], 2);
    assert!(visible["terminals"].as_array().is_some_and(|terminals| {
        terminals
            .iter()
            .any(|terminal| terminal["id"] == json!(id) && terminal["title"] == renamed.title)
    }));
    fixture.shutdown().await;
}

#[tokio::test]
async fn exact_create_replay_bypasses_mutable_workspace_verification() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let payload = create_payload(wire_id, &fixture.root);
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, payload.clone())
        .await
        .expect("initial create");
    let authority = fixture
        .desktop
        .live_terminal_reopen_authority(TerminalId::new(wire_id))
        .expect("authority store");
    let verification_calls = fixture.releases.verification_calls.load(Ordering::Acquire);
    fixture
        .releases
        .verification_unavailable
        .store(true, Ordering::Release);

    let replayed = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, payload)
        .await
        .expect("exact replay despite unavailable workspace verifier");

    assert_eq!(replayed, created);
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    assert!(!*mutex_lock(&fixture.backend.process(0).stopped));
    assert_eq!(
        fixture.releases.verification_calls.load(Ordering::Acquire),
        verification_calls
    );
    assert_eq!(
        fixture
            .desktop
            .live_terminal_reopen_authority(TerminalId::new(wire_id))
            .expect("authority store after replay"),
        authority
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_new_spawn_rolls_back_without_leaving_reopen_authority() {
    let mut fixture = Fixture::new();
    fixture
        .releases
        .unavailable_on_verification_call
        .store(2, Ordering::Release);
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect_err("post-spawn verification must fail");
    assert_eq!(error.code, "terminal-authority-changed");
    assert!(!error.replay_safe);
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("authority store")
            .is_none()
    );
    assert!(*mutex_lock(&fixture.backend.process(0).stopped));
    assert!(matches!(
        fixture.pushes.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let mut byte = [0_u8; 1];
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            fixture.electron_output.read(&mut byte),
        )
        .await
        .is_err(),
        "an uncommitted generation must not expose terminal bytes",
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_saved_reopen_preserves_prior_authority_and_retries_in_place() {
    let mut fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("seed terminal authority");
    fixture.backend.process(0).stop();
    let exited = wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved",
                    "name": "Saved",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": wire_id,
                        "title": exited["terminals"][0]["title"],
                        "cwd": fixture.root
                    }]
                }]
            }),
        )
        .expect("save terminal roster");
    fixture = fixture.reopen_without_workspace_grant().await;
    let TerminalReopenAuthorityState::Authority(prior) =
        fixture.desktop.terminal_reopen_authority_state(id)
    else {
        panic!("saved authority");
    };
    let unavailable_call = fixture.releases.verification_calls.load(Ordering::Acquire) + 2;
    fixture
        .releases
        .unavailable_on_verification_call
        .store(unavailable_call, Ordering::Release);

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect_err("post-spawn verifier outage");
    assert_eq!(error.code, "terminal-authority-changed");
    assert_eq!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("authority store"),
        Some(prior.clone())
    );
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(*mutex_lock(&fixture.backend.process(1).stopped));

    fixture
        .releases
        .unavailable_on_verification_call
        .store(0, Ordering::Release);
    let retried = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("same-host cold reopen retry");
    assert_eq!(retried["id"], json!(wire_id));
    let TerminalReopenAuthorityState::Authority(retried_authority) =
        fixture.desktop.terminal_reopen_authority_state(id)
    else {
        panic!("retried authority");
    };
    assert_eq!(retried_authority.root, prior.root);
    fixture.shutdown().await;
}

async fn saved_root_fixture(managed: bool) -> Fixture {
    let fixture = Fixture::new();
    if managed {
        mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    }
    let saved_id = uuid::Uuid::new_v4();
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(saved_id, &fixture.root),
        )
        .await
        .expect("seed approved terminal");
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces": [{
                "id": "saved", "name": "Saved",
                "createdAt": "2026-09-08T00:00:00Z",
                "updatedAt": "2026-09-08T00:00:00Z",
                "terminals": [{"id": saved_id, "title": "Terminal", "cwd": fixture.root}]
            }]}),
        )
        .expect("save workspace");
    fixture.reopen_without_workspace_grant().await
}

#[tokio::test]
async fn fresh_terminal_uses_saved_host_root_after_restart_without_picker_grant() {
    let fixture = saved_root_fixture(false).await;
    assert!(fixture.desktop.workspace_roots().is_empty());
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());

    let fresh_id = uuid::Uuid::new_v4();
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(fresh_id, &fixture.root),
        )
        .await
        .expect("fresh terminal in previously approved saved workspace");
    assert_eq!(created["id"], json!(fresh_id));
    assert_eq!(created["root"], json!(fixture.root));
    assert!(
        fixture.desktop.workspace_roots().is_empty(),
        "terminal proof does not grant editor access"
    );

    let nested = fixture.root.join("unapproved-child");
    fs::create_dir(&nested).expect("nested directory");
    let denied = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &nested),
        )
        .await
        .expect_err("saved proof grants only its exact root");
    assert_eq!(denied.code, "terminal-cwd-not-approved");

    fixture
        .desktop
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces": []}))
        .expect("remove saved proof membership");
    let denied = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect_err("removing the saved workspace revokes its root proof");
    assert_eq!(denied.code, "terminal-cwd-not-approved");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 2);
    fixture.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn fresh_terminal_at_a_moved_saved_root_starts_where_the_link_points() {
    let fixture = saved_root_fixture(false).await;
    // The project moves and leaves a symlink at its saved path, as a checkout
    // relocated out of a synced folder does. The renderer keeps naming the
    // old path.
    let moved = fixture.temporary.path().join("moved-workspace");
    fs::rename(&fixture.root, &moved).expect("move workspace");
    std::os::unix::fs::symlink(&moved, &fixture.root).expect("link the old path");
    let moved = fs::canonicalize(&moved).expect("canonical moved workspace");

    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect("fresh terminal through the old path");
    assert_eq!(created["root"], json!(moved));
    assert_eq!(created["cwd"], json!(moved));

    // A link into the workspace is still not an exact root.
    let nested = moved.join("child");
    fs::create_dir(&nested).expect("nested directory");
    let into_child = fixture.temporary.path().join("child-link");
    std::os::unix::fs::symlink(&nested, &into_child).expect("link a subfolder");
    let denied = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &into_child),
        )
        .await
        .expect_err("a link to a subfolder names no approved root");
    assert_eq!(denied.code, "terminal-cwd-changed");
    fixture.shutdown().await;
}

#[tokio::test]
async fn fresh_terminal_cannot_use_a_renderer_only_saved_root() {
    let fixture = saved_root_fixture(false).await;
    let forged = fixture.temporary.path().join("forged-root");
    fs::create_dir(&forged).expect("unapproved root");
    let forged = fs::canonicalize(forged).expect("canonical forged root");
    let mut saved = fixture
        .desktop
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!({}))
        .expect("saved workspace");
    saved[0]["terminals"][0]["cwd"] = json!(forged);
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces": saved}),
        )
        .expect("renderer can remember a folder but cannot grant it");
    for root in [&forged, &fixture.root] {
        let error = fixture
            .dispatcher
            .dispatch(
                CHANNELS.create_terminal,
                create_payload(uuid::Uuid::new_v4(), root),
            )
            .await
            .expect_err("changed saved identity is not authority for either root");
        assert_eq!(error.code, "terminal-cwd-not-approved");
    }
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn fresh_terminal_saved_root_preserves_managed_generation_verification() {
    let fixture = saved_root_fixture(true).await;
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect("fresh terminal in verified managed root");
    let id: TerminalId = serde_json::from_value(created["id"].clone()).expect("terminal id");
    assert_eq!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("host store")
            .expect("fresh proof")
            .managed_worktree_key
            .as_deref(),
        Some("managed#test")
    );
    *mutex_lock(&fixture.releases.managed_key) = Some("managed#replacement".to_owned());
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect_err("replacement worktree cannot reuse the saved generation");
    assert_eq!(error.code, "terminal-reopen-authority-stale");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn fresh_terminal_rolls_back_when_saved_root_proof_is_removed_during_spawn() {
    let fixture = saved_root_fixture(false).await;
    let post_spawn_call = fixture.releases.verification_calls.load(Ordering::Acquire) + 2;
    fixture
        .releases
        .block_on_verification_call
        .store(post_spawn_call, Ordering::Release);
    let id = uuid::Uuid::new_v4();
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let payload = create_payload(id, &fixture.root);
    let create =
        tokio::spawn(async move { dispatcher.dispatch(CHANNELS.create_terminal, payload).await });
    eventually("post-spawn verification paused", || {
        fixture
            .releases
            .verification_blocked
            .load(Ordering::Acquire)
    })
    .await;
    fixture
        .desktop
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces": []}))
        .expect("remove saved workspace during verification");
    fixture
        .releases
        .release_verification
        .store(true, Ordering::Release);
    fixture.releases.verification_changed.notify_waiters();
    let error = create
        .await
        .expect("create task")
        .expect_err("withdrawn authority");
    assert_eq!(error.code, "terminal-authority-changed");
    assert!(!error.replay_safe);
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(*mutex_lock(&fixture.backend.process(1).stopped));
    assert!(
        fixture
            .desktop
            .live_terminal_reopen_authority(TerminalId::new(id))
            .expect("authority store")
            .is_none()
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn cold_default_root_starts_without_renderer_grant_but_children_stay_refused() {
    let fixture = Fixture::new();
    assert!(
        !fixture
            .desktop
            .workspace_roots()
            .contains(&fixture.default_root)
    );

    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.default_root),
        )
        .await
        .expect("cold default terminal");
    assert_eq!(
        created["cwd"],
        fixture.default_root.to_string_lossy().as_ref()
    );

    let nested = fixture.default_root.join("remembered-child");
    std::fs::create_dir(&nested).expect("remembered child");
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &nested),
        )
        .await
        .expect_err("unapproved remembered child");
    assert_eq!(error.code, "terminal-cwd-not-approved");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn cold_saved_terminal_reopens_inside_its_managed_worktree_and_retains_that_root() {
    let mut fixture = Fixture::new();
    let terminal_id = uuid::Uuid::new_v4();
    let nested = fixture.root.join("src/deeper");
    std::fs::create_dir_all(&nested).expect("saved descendant cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical saved descendant");
    fixture.desktop.revoke_workspace_root(&fixture.root);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved-worktree",
                    "name": "Saved worktree",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": terminal_id,
                        "title": "Claude",
                        "cwd": nested,
                        "sessionKey": "managed#test"
                    }]
                }]
            }),
        )
        .expect("save terminal workspace");
    fs::remove_file(
        fixture
            .temporary
            .path()
            .join(".terminal-workspaces.json.host-authority"),
    )
    .expect("model an upgrade from a build without the authority sidecar");
    fixture = fixture.reopen_without_workspace_grant().await;

    let unknown = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &nested),
        )
        .await
        .expect_err("a renderer-only terminal id grants no retained root");
    assert_eq!(unknown.code, "terminal-cwd-not-approved");

    let mut reopen = create_payload(terminal_id, &nested);
    reopen["sessionKey"] = json!("managed#test");
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen)
        .await
        .expect("cold saved terminal in managed worktree");
    assert_eq!(created["cwd"], nested.to_string_lossy().as_ref());
    assert_eq!(created["root"], fixture.root.to_string_lossy().as_ref());
    assert!(fixture.desktop.workspace_is_open(&fixture.root));

    let releases = Arc::clone(&fixture.releases);
    let expected_release = fixture.root.to_string_lossy().into_owned();
    fixture.shutdown().await;
    assert_eq!(
        mutex_lock(&releases.workspaces).as_slice(),
        &[expected_release],
        "shutdown must schedule the common cleanup owner, whose saved-roster reservation retains this root"
    );
}

#[tokio::test]
async fn legacy_worktree_migration_survives_a_transient_verifier_failure() {
    let mut fixture = Fixture::new();
    let terminal_id = uuid::Uuid::new_v4();
    fixture.desktop.revoke_workspace_root(&fixture.root);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved-worktree",
                    "name": "Saved worktree",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": terminal_id,
                        "title": "Claude",
                        "cwd": fixture.root,
                        "sessionKey": "managed#test"
                    }]
                }]
            }),
        )
        .expect("save terminal workspace");
    fs::remove_file(
        fixture
            .temporary
            .path()
            .join(".terminal-workspaces.json.host-authority"),
    )
    .expect("model a pre-authority build");
    fixture = fixture.reopen_without_workspace_grant().await;
    fixture
        .releases
        .verification_unavailable
        .store(true, Ordering::Release);

    let mut reopen = create_payload(terminal_id, &fixture.root);
    reopen["sessionKey"] = json!("managed#test");
    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen.clone())
        .await
        .expect_err("unavailable migration verifier");
    assert_eq!(error.code, "terminal-authority-unavailable");
    assert!(mutex_lock(&fixture.backend.processes).is_empty());

    fixture
        .releases
        .verification_unavailable
        .store(false, Ordering::Release);
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen)
        .await
        .expect("retry keeps and resolves the migration candidate");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn legacy_gemini_authority_reopens_as_fresh_antigravity() {
    const LEGACY_SESSION: &str = "11111111-1111-4111-8111-111111111111";

    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "fixed-antigravity-executable");
    let executable = std::fs::canonicalize(executable).expect("canonical Antigravity executable");
    let arguments = vec![OsString::from("--fixed-antigravity-argument")];
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Gemini,
        &executable,
        arguments.clone(),
        Vec::new(),
    )
    .expect("fixed Antigravity launch program")])
    .expect("launch catalog");
    let mut fixture = Fixture::new();
    let terminal_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(terminal_id);
    fixture.desktop.revoke_workspace_root(&fixture.root);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved-antigravity",
                    "name": "Saved Antigravity",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": terminal_id,
                        "title": "Gemini",
                        "cwd": fixture.root,
                        "sessionKey": "managed#test",
                        "launch": {"version": 1, "profileId": "gemini"},
                        "launchSession": {
                            "version": 1,
                            "id": LEGACY_SESSION,
                            "resume": true
                        }
                    }]
                }]
            }),
        )
        .expect("save legacy Gemini terminal workspace");
    fs::remove_file(
        fixture
            .temporary
            .path()
            .join(".terminal-workspaces.json.host-authority"),
    )
    .expect("model a pre-authority Gemini build");
    fixture = fixture
        .reopen_without_workspace_grant_with_launches(catalog)
        .await;
    assert!(matches!(
        fixture.desktop.terminal_reopen_authority_state(id),
        TerminalReopenAuthorityState::PendingMigration(_)
    ));

    let mut reopen = create_payload(terminal_id, &fixture.root);
    reopen["title"] = json!("Antigravity");
    reopen["sessionKey"] = json!("managed#test");
    reopen["launch"] = json!({"version": 1, "profileId": "gemini"});
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen)
        .await
        .expect("legacy Gemini pane launches fresh Antigravity");

    assert_eq!(created["title"], "Antigravity");
    assert!(created["launchSession"].is_null());
    assert_eq!(
        mutex_lock(&fixture.backend.specs)[0].launch,
        ResolvedTerminalLaunch::Program {
            executable,
            arguments,
            environment: Vec::new(),
        }
    );
    assert!(
        fixture
            .desktop
            .live_terminal_reopen_authority(id)
            .expect("authority store")
            .expect("migrated Antigravity authority")
            .terminal
            .launch_session
            .is_none(),
        "the successful cold reopen must rewrite the host sidecar without Gemini's binding"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn reused_worktree_path_with_a_new_ledger_key_cannot_replay_saved_authority() {
    let mut fixture = Fixture::new();
    let terminal_id = uuid::Uuid::new_v4();
    fixture.desktop.revoke_workspace_root(&fixture.root);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    *mutex_lock(&fixture.releases.managed_key) = Some("managed#old".to_owned());
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "saved-worktree",
                    "name": "Saved worktree",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": terminal_id,
                        "title": "Claude",
                        "cwd": fixture.root,
                        "sessionKey": "managed#old"
                    }]
                }]
            }),
        )
        .expect("save terminal workspace");
    fs::remove_file(
        fixture
            .temporary
            .path()
            .join(".terminal-workspaces.json.host-authority"),
    )
    .expect("model a pre-authority build");
    fixture = fixture.reopen_without_workspace_grant().await;
    let mut reopen = create_payload(terminal_id, &fixture.root);
    reopen["sessionKey"] = json!("managed#old");
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen.clone())
        .await
        .expect("initial migration with the matching key");
    fixture = fixture.reopen_without_workspace_grant().await;
    *mutex_lock(&fixture.releases.managed_key) = Some("managed#replacement".to_owned());
    let before = mutex_lock(&fixture.backend.processes).len();

    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen)
        .await
        .expect_err("reused worktree generation");
    assert_eq!(error.code, "terminal-reopen-authority-stale");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), before);
    fixture.shutdown().await;
}

#[tokio::test]
async fn renderer_added_saved_id_after_startup_cannot_mint_worktree_authority() {
    let fixture = Fixture::new();
    let terminal_id = uuid::Uuid::new_v4();
    fixture.desktop.revoke_workspace_root(&fixture.root);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "late-worktree",
                    "name": "Late worktree",
                    "createdAt": "2026-09-02T00:00:00Z",
                    "updatedAt": "2026-09-02T00:00:00Z",
                    "terminals": [{
                        "id": terminal_id,
                        "title": "Claude",
                        "cwd": fixture.root,
                        "sessionKey": "managed#test"
                    }]
                }]
            }),
        )
        .expect("renderer roster save");
    let mut reopen = create_payload(terminal_id, &fixture.root);
    reopen["sessionKey"] = json!("managed#test");

    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, reopen)
        .await
        .expect_err("late renderer ID has no migration capability");
    assert_eq!(error.code, "terminal-reopen-authority-unproven");
    assert!(!matches!(
        fixture
            .desktop
            .terminal_reopen_authority_state(TerminalId::new(terminal_id)),
        TerminalReopenAuthorityState::Authority(_)
    ));
    assert!(mutex_lock(&fixture.backend.processes).is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_still_schedules_release_for_an_unsaved_managed_worktree() {
    let fixture = Fixture::new();
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect("unsaved managed-worktree terminal");

    let releases = Arc::clone(&fixture.releases);
    let expected = fixture.root.to_string_lossy().into_owned();
    fixture.dispatcher.close().await;
    assert_eq!(
        mutex_lock(&releases.workspaces).as_slice(),
        &[expected.clone()],
        "explicit terminal shutdown must drop leases before the worktree release queue drains"
    );
    fixture.shutdown().await;
    assert_eq!(mutex_lock(&releases.workspaces).as_slice(), &[expected]);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn launch_profiles_are_host_projected_and_agent_creation_uses_fixed_process_material() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let executable = executable(&launch_directory, "fixed-claude-executable");
    let executable = std::fs::canonicalize(executable).expect("canonical executable");
    let arguments = vec![OsString::from("--fixed-host-argument")];
    let environment = vec![(
        OsString::from("INTERFACE_FIXED_LAUNCH"),
        OsString::from("enabled"),
    )];
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &executable,
        arguments.clone(),
        environment.clone(),
    )
    .expect("fixed launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    let profiles = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminal_launch_profiles, Value::Null)
        .await
        .expect("launch profiles");
    assert_eq!(
        profiles
            .as_array()
            .expect("launch profile array")
            .iter()
            .map(|profile| profile["id"].as_str().expect("profile id"))
            .collect::<Vec<_>>(),
        vec![
            "shell", "claude", "codex", "grok", "gemini", "omp", "opencode", "muse", "kimi"
        ]
    );
    assert_eq!(
        profiles[0],
        json!({"id": "shell", "label": "Terminal", "available": true})
    );
    assert_eq!(
        profiles[1],
        json!({
            "id": "claude",
            "engine": "claude",
            "label": "Claude Code",
            "available": true
        })
    );
    assert_eq!(profiles[2]["available"], false);
    assert!(profiles[2]["unavailableReason"].is_string());
    assert_eq!(profiles[4]["label"], "Antigravity");
    let public_wire = profiles.to_string();
    assert!(!public_wire.contains(&executable.to_string_lossy().into_owned()));
    assert!(!public_wire.contains("--fixed-host-argument"));
    assert!(!public_wire.contains("INTERFACE_FIXED_LAUNCH"));

    let id = uuid::Uuid::new_v4();
    let mut request = create_payload(id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("agent terminal");
    assert_eq!(
        created["launch"],
        json!({"version": 1, "profileId": "claude"})
    );
    assert_eq!(
        mutex_lock(&fixture.backend.specs)[0].launch,
        ResolvedTerminalLaunch::Program {
            executable,
            arguments,
            environment,
        }
    );

    let process_count = mutex_lock(&fixture.backend.processes).len();
    for (launch, expected_code) in [
        (
            json!({"version": 1, "profileId": "gemini"}),
            "terminal-launch-profile-unavailable",
        ),
        (
            json!({"version": 2, "profileId": "claude"}),
            INVALID_PAYLOAD_CODE,
        ),
        (
            json!({
                "version": 1,
                "profileId": "claude",
                "executable": "/renderer/controlled"
            }),
            INVALID_PAYLOAD_CODE,
        ),
    ] {
        let mut request = create_payload(uuid::Uuid::new_v4(), &fixture.root);
        request["launch"] = launch;
        let error = fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, request)
            .await
            .expect_err("unsafe or unavailable launch refused");
        assert_eq!(error.code, expected_code);
        assert!(error.replay_safe);
    }
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), process_count);

    fixture.shutdown().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn pass_off_create_follows_its_source_and_refuses_an_unbindable_target() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let claude_executable = executable(&launch_directory, "fixed-claude-executable");
    let claude_executable =
        std::fs::canonicalize(claude_executable).expect("canonical Claude executable");
    let grok_executable = executable(&launch_directory, "fixed-grok-executable");
    let grok_executable =
        std::fs::canonicalize(grok_executable).expect("canonical Grok executable");
    let catalog = TerminalLaunchCatalog::new([
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Claude,
            &claude_executable,
            Vec::new(),
            Vec::new(),
        )
        .expect("fixed Claude launch program"),
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Grok,
            &grok_executable,
            Vec::new(),
            Vec::new(),
        )
        .expect("fixed Grok launch program"),
    ])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    mutex_lock(&fixture.releases.managed_roots).push(fixture.root.clone());

    let source_id = uuid::Uuid::new_v4();
    let mut request = create_payload(source_id, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    let source = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("source agent terminal");

    // A pass-off pane starts where its source agent works, not where the
    // request pointed: this one asks for the default root and still lands
    // in the source's approved workspace.
    let pass_off_id = uuid::Uuid::new_v4();
    let target_session_id = uuid::Uuid::new_v4();
    let mut request = create_payload(pass_off_id, &fixture.default_root);
    request["launch"] = json!({"version": 1, "profileId": "grok"});
    request["launchSession"] = json!({
        "version": 1,
        "id": target_session_id,
        "resume": false
    });
    request["handoff"] = json!({
        "version": 1,
        "sourceTerminalId": source_id,
        "transcript": "step 1 done",
        "fromTitle": "Build"
    });
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("pass-off terminal");
    assert_eq!(
        created["launch"],
        json!({"version": 1, "profileId": "grok"})
    );
    assert_eq!(
        created["launchSession"],
        json!({"version": 1, "id": target_session_id, "resume": false})
    );
    assert_eq!(created["cwd"], source["cwd"]);
    assert_eq!(created["root"], source["root"]);
    assert_ne!(
        created["cwd"],
        json!(fixture.default_root.to_string_lossy().as_ref())
    );
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        ..
    } = mutex_lock(&fixture.backend.specs)[1].launch.clone()
    else {
        panic!("a pass-off pane spawns a launch program");
    };
    assert_eq!(executable, grok_executable);
    assert_eq!(
        arguments.first(),
        Some(&OsString::from(format!("--session-id={target_session_id}")))
    );
    let worktree_argument = OsString::from("--worktree");
    assert!(
        !arguments
            .iter()
            .any(|argument| argument == &worktree_argument),
        "Interface terminal launches must not request a Grok worktree: {arguments:?}"
    );
    let prompt = arguments.last().expect("trailing pass-off prompt").clone();
    assert!(
        prompt
            .to_string_lossy()
            .contains(&format!("{pass_off_id}.md")),
        "the pass-off prompt names its own transcript file: {prompt:?}"
    );

    let process_count = mutex_lock(&fixture.backend.processes).len();
    for (launch, handoff, expected_code) in [
        (
            Value::Null,
            json!({
                "version": 1,
                "sourceTerminalId": source_id,
                "transcript": "step 1 done"
            }),
            "terminal-handoff-unsupported-profile",
        ),
        (
            json!({"version": 1, "profileId": "claude"}),
            json!({
                "version": 1,
                "sourceTerminalId": source_id,
                "transcript": "step 1 done",
                "sourceCwd": "/renderer/controlled"
            }),
            INVALID_PAYLOAD_CODE,
        ),
    ] {
        let mut request = create_payload(uuid::Uuid::new_v4(), &fixture.default_root);
        if !launch.is_null() {
            request["launch"] = launch;
        }
        request["handoff"] = handoff;
        let error = fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, request)
            .await
            .expect_err("unbindable or malformed pass-off refused");
        assert_eq!(error.code, expected_code);
        assert!(error.replay_safe);
    }
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), process_count);

    // If the source root stops being an approved pass-off destination,
    // core falls back to the requested default root. The host must reject
    // that post-spawn root mismatch and fully dismiss the uncommitted PTY
    // instead of returning a keyless managed terminal.
    fixture.desktop.revoke_workspace_root(&fixture.root);
    let mismatched_id = uuid::Uuid::new_v4();
    let mut request = create_payload(mismatched_id, &fixture.default_root);
    request["launch"] = json!({"version": 1, "profileId": "grok"});
    request["handoff"] = json!({
        "version": 1,
        "sourceTerminalId": source_id,
        "transcript": "step 2 pending"
    });
    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect_err("changed handoff root");
    assert_eq!(error.code, "terminal-authority-changed");
    assert!(
        !fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .any(|terminal| terminal.id == TerminalId::new(mismatched_id))
    );
    assert!(
        !mutex_lock(&fixture.dispatcher.pending_managed_reopen_proofs)
            .contains_key(&TerminalId::new(mismatched_id))
    );
    fixture.shutdown().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn control_binary_lifecycle_and_workspace_lease_are_one_host_owned_flow() {
    let mut fixture = Fixture::new();
    let id = uuid::Uuid::new_v4();
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create_payload(id, &fixture.root))
        .await
        .expect("create integrated terminal");
    assert_eq!(created["id"], json!(id));
    assert_eq!(created["generation"], 1);
    assert_eq!(created["state"], "running");
    for active in [false, true] {
        let activity = fixture
            .dispatcher
            .dispatch(
                CHANNELS.set_terminal_workspace_activity,
                json!({"version": 1, "id": id, "generation": 1, "active": active}),
            )
            .await
            .unwrap();
        assert_eq!(activity["id"], created["id"]);
        assert_eq!(activity["generation"], 1);
    }
    let stale = fixture
        .dispatcher
        .dispatch(
            CHANNELS.set_terminal_workspace_activity,
            json!({"version": 1, "id": id, "generation": 2, "active": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "terminal-stale-generation");
    assert!(fixture.desktop.workspace_is_open(&fixture.root));

    let lifecycle = fixture.pushes.recv().await.expect("create lifecycle push");
    assert_eq!(lifecycle.operation, CHANNELS.terminal_lifecycle);
    assert_eq!(lifecycle.payload["kind"], "upsert");
    assert_eq!(lifecycle.payload["terminal"]["state"], "running");

    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    let output = read_data_frame(&mut fixture.electron_output, &mut decoder).await;
    assert_eq!(output.terminal_id, id);
    assert_eq!(output.generation, 1);
    assert_eq!(output.sequence, 0);
    assert_eq!(output.payload, b"ready\r\n");

    let input = TerminalDataFrame::new(
        id,
        1,
        0,
        TerminalDataDirection::ElectronToHost,
        TerminalDataKind::Data,
        b"printf hello\n".to_vec(),
    )
    .expect("input frame")
    .encode()
    .expect("encoded input");
    fixture
        .electron_input
        .write_all(&input)
        .await
        .expect("send terminal input");
    eventually("terminal input delivery", || {
        mutex_lock(&fixture.backend.process(0).input).as_slice() == b"printf hello\n"
    })
    .await;

    fixture
        .dispatcher
        .dispatch(
            CHANNELS.resize_terminal,
            json!({
                "id": id,
                "generation": 1,
                "size": {"columns": 120, "rows": 40}
            }),
        )
        .await
        .expect("resize terminal");
    assert_eq!(
        mutex_lock(&fixture.backend.process(0).sizes).as_slice(),
        &[TerminalSize {
            columns: 120,
            rows: 40
        }]
    );

    fixture
        .dispatcher
        .dispatch(CHANNELS.close_terminal, json!({"id": id, "generation": 1}))
        .await
        .expect("close terminal");
    let exited = wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    assert_eq!(exited["terminals"][0]["exit"]["code"], 0);
    eventually("terminal workspace lease release", || {
        !fixture.desktop.workspace_is_open(&fixture.root)
            && !mutex_lock(&fixture.releases.workspaces).is_empty()
    })
    .await;
    assert_eq!(
        mutex_lock(&fixture.releases.workspaces).as_slice(),
        &[fixture.root.to_string_lossy().into_owned()]
    );
    let eof = read_data_frame(&mut fixture.electron_output, &mut decoder).await;
    assert_eq!(eof.kind, TerminalDataKind::Eof);
    assert_eq!(eof.sequence, 1);
    eventually("completed output pump pruning", || {
        fixture
            .dispatcher
            .prune_output_pumps()
            .expect("prune completed output pumps");
        mutex_lock(&fixture.dispatcher.output_pumps).is_empty()
    })
    .await;
    // The exited pane is still shown, so read_pane still sees its output
    // after its pump is gone.
    let pane = TerminalId::new(id);
    assert!(
        fixture
            .dispatcher
            .read_tail(pane, TerminalGeneration(1), 10)
            .is_some_and(|tail| tail.contains("ready")),
        "an exited, visible pane keeps its readable tail"
    );

    let restarted = fixture
        .dispatcher
        .dispatch(
            CHANNELS.restart_terminal,
            json!({"id": id, "generation": 1}),
        )
        .await
        .expect("restart terminal");
    assert_eq!(restarted["generation"], 2);
    assert!(fixture.desktop.workspace_is_open(&fixture.root));
    let stale = fixture
        .dispatcher
        .dispatch(
            CHANNELS.rename_terminal,
            json!({"id": id, "generation": 1, "title": "stale"}),
        )
        .await
        .expect_err("stale generation refused");
    assert_eq!(stale.code, "terminal-stale-generation");

    fixture.shutdown().await;
}

#[tokio::test]
async fn a_parked_terminal_write_does_not_stall_another_terminal_input() {
    let mut fixture = Fixture::new();
    let parked = uuid::Uuid::new_v4();
    let flowing = uuid::Uuid::new_v4();
    for id in [parked, flowing] {
        fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, create_payload(id, &fixture.root))
            .await
            .expect("create integrated terminal");
    }
    let parked_process = fixture.backend.process(0);
    let flowing_process = fixture.backend.process(1);
    parked_process.block_input();

    for (id, bytes) in [
        (parked, b"stuck".as_slice()),
        (flowing, b"flows".as_slice()),
    ] {
        let input = TerminalDataFrame::new(
            id,
            1,
            0,
            TerminalDataDirection::ElectronToHost,
            TerminalDataKind::Data,
            bytes.to_vec(),
        )
        .expect("input frame")
        .encode()
        .expect("encoded input");
        fixture
            .electron_input
            .write_all(&input)
            .await
            .expect("send terminal input");
    }

    eventually(
        "second terminal input must not queue behind a parked write",
        || mutex_lock(&flowing_process.input).as_slice() == b"flows",
    )
    .await;
    assert!(mutex_lock(&parked_process.input).is_empty());

    parked_process.unblock_input();
    eventually("released terminal input delivery", || {
        mutex_lock(&parked_process.input).as_slice() == b"stuck"
    })
    .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn input_pipe_eof_fences_creation_and_terminates_active_processes() {
    let mut fixture = Fixture::new();
    let id = uuid::Uuid::new_v4();
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create_payload(id, &fixture.root))
        .await
        .expect("create integrated terminal");
    let process = fixture.backend.process(0);

    fixture
        .electron_input
        .shutdown()
        .await
        .expect("close Electron input pipe");
    eventually("transport EOF terminal teardown", || {
        *mutex_lock(&process.stopped)
            && !fixture
                .dispatcher
                .transport_available
                .load(Ordering::Acquire)
    })
    .await;

    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(uuid::Uuid::new_v4(), &fixture.root),
        )
        .await
        .expect_err("failed transport must fence new terminals");
    assert_eq!(error.code, "terminal-data-unavailable");
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn unavailable_data_plane_and_runtime_authority_fail_before_spawning() {
    let temporary = TempDir::new().expect("temporary terminal host");
    let home = temporary.path().join("home");
    let root = temporary.path().join("workspace");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default directory");
    std::fs::create_dir(&root).expect("workspace");
    let desktop = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: temporary.path().join("appearance.json"),
                acp_profiles_file: temporary.path().join("acp-profiles.json"),
                terminal_workspaces_file: temporary.path().join("terminal-workspaces.json"),
                home_directory: home,
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("desktop service"),
    );
    desktop
        .grant_workspace_root(&root)
        .expect("explicit root grant");
    let backend = Arc::new(FakeBackend::default());
    let releases = Arc::new(ReleaseRecorder::default());
    let (pushes, _receiver) = mpsc::channel(4);
    let unavailable = TerminalHostDispatcher::with_backend_launches_and_claude_store(
        Arc::new(FakeRuntime::default()),
        Arc::clone(&desktop),
        releases.clone(),
        pushes.clone(),
        None,
        TerminalServiceConfig::default(),
        backend.clone(),
        TerminalLaunchCatalog::shell_only(),
        None,
        None,
    )
    .expect("disabled terminal dispatcher");
    let id = uuid::Uuid::new_v4();
    let error = unavailable
        .dispatch(CHANNELS.create_terminal, create_payload(id, &root))
        .await
        .expect_err("missing data plane refused");
    assert_eq!(error.code, "terminal-data-unavailable");
    assert!(mutex_lock(&backend.processes).is_empty());

    let (electron_input, host_input) = tokio::io::duplex(1024);
    let (host_output, _electron_output) = tokio::io::duplex(1024);
    let plane = TerminalDataPlane::start(
        host_input,
        host_output,
        TerminalDataTransportConfig::default(),
    );
    drop(electron_input);
    let authority_error = TerminalHostDispatcher::with_backend_launches_and_claude_store(
        Arc::new(FailingRuntime),
        desktop,
        releases,
        pushes,
        Some(plane.outbound.clone()),
        TerminalServiceConfig::default(),
        backend.clone(),
        TerminalLaunchCatalog::shell_only(),
        None,
        None,
    )
    .expect("terminal dispatcher")
    .dispatch(CHANNELS.create_terminal, create_payload(id, &root))
    .await
    .expect_err("runtime authority failure closes creation");
    assert_eq!(authority_error.code, "terminal-authority-unavailable");
    assert!(mutex_lock(&backend.processes).is_empty());
    plane.shutdown().await;
}

#[tokio::test]
async fn strict_control_payloads_and_owner_table_are_stable() {
    assert_eq!(TERMINAL_OPERATIONS.len(), 14);
    let fixture = Fixture::new();
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"extra": true}),
        )
        .await
        .expect_err("non-empty launch profile list request refused");
    assert_eq!(error.code, INVALID_PAYLOAD_CODE);
    let error = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, json!({"extra": true}))
        .await
        .expect_err("non-empty list request refused");
    assert_eq!(error.code, INVALID_PAYLOAD_CODE);
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            json!({
                "id": uuid::Uuid::new_v4(),
                "cwd": fixture.root,
                "extra": true
            }),
        )
        .await
        .expect_err("unknown create field refused");
    assert_eq!(error.code, INVALID_PAYLOAD_CODE);
    fixture.shutdown().await;
}

#[tokio::test]
async fn retired_settings_channel_answers_unavailable_and_never_writes() {
    let directory = TempDir::new().unwrap();
    let program = executable(&directory, "codex-control-probe");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        &program,
        Vec::new(),
        Vec::new(),
    )
    .unwrap()])
    .unwrap();
    let fixture = Fixture::new_with_launches(catalog);
    let id = uuid::Uuid::new_v4();
    let mut create = create_payload(id, &fixture.root);
    create["launch"] = json!({"version":1,"profileId":"codex"});
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create)
        .await
        .unwrap();
    let request = json!({"version":1,"id":id,"generation":1,
        "screen":{"lines":["› ","gpt-6-astra high · /repo"],"cursorRow":0,
            "cursorColumn":2,"atBottom":true,"bracketedPaste":true,"ghostText":false},
        "selection":{"kind":"effort","value":"xhi"}});
    let mut stale = request.clone();
    stale["generation"] = json!(2);
    assert!(
        fixture
            .dispatcher
            .dispatch(CHANNELS.advance_terminal_controls, stale)
            .await
            .is_err()
    );
    assert!(mutex_lock(&fixture.backend.process(0).input).is_empty());
    let result = fixture
        .dispatcher
        .dispatch(CHANNELS.advance_terminal_controls, request)
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"version":1,"model":null,"effort":null,"ready":false,"pending":false,
            "error":"Terminal settings controls are no longer available."})
    );
    assert!(mutex_lock(&fixture.backend.process(0).input).is_empty());
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.dispatcher.close().await;
}

#[path = "pane_report_tests.rs"]
mod pane_report_tests;

/// The admission mirror always equals the lifecycle state it summarizes.
fn assert_admission_mirror(dispatcher: &TerminalHostDispatcher) {
    let lifecycle = dispatcher.lifecycle_sink.state.lock().expect("lifecycle");
    let derived: BTreeMap<_, _> = lifecycle
        .visible_terminals
        .iter()
        .filter(|(id, _)| !lifecycle.transactions.contains_key(id))
        .map(|(id, terminal)| (*id, terminal.generation))
        .collect();
    assert_eq!(
        *dispatcher
            .lifecycle_sink
            .admission
            .lock()
            .expect("admission"),
        derived
    );
}

fn input_frame(id: TerminalId, generation: TerminalGeneration) -> TerminalDataFrame {
    TerminalDataFrame::new(
        id.as_uuid(),
        generation.0,
        0,
        TerminalDataDirection::ElectronToHost,
        TerminalDataKind::Data,
        b"x".to_vec(),
    )
    .expect("input frame")
}

#[tokio::test]
async fn keystrokes_are_admitted_without_waiting_for_a_publish_in_progress() {
    let fixture = Fixture::new();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let wire_id = uuid::Uuid::new_v4();
        let created = fixture
            .dispatcher
            .dispatch(
                CHANNELS.create_terminal,
                create_payload(wire_id, &fixture.root),
            )
            .await
            .expect("create terminal");
        let generation = TerminalGeneration(created["generation"].as_u64().expect("generation"));
        ids.push((TerminalId::new(wire_id), generation));
    }
    assert_admission_mirror(&fixture.dispatcher);
    let (first, first_generation) = ids[0];
    let (second, second_generation) = ids[1];

    // A restart transaction on one pane refuses its input until it ends,
    // for its current generation and any other.
    let authority = fixture
        .desktop
        .live_terminal_reopen_authority(first)
        .expect("authority store")
        .expect("live restart authority");
    fixture
        .dispatcher
        .begin_reopen_authority_commit(
            first,
            Some(authority),
            ReopenAuthorityTransactionKind::Restart {
                prior_generation: first_generation,
            },
        )
        .await
        .expect("begin restart transaction");
    assert_admission_mirror(&fixture.dispatcher);
    for generation in [first_generation, first_generation.next()] {
        assert_eq!(
            fixture
                .dispatcher
                .enqueue_input_frame(input_frame(first, generation))
                .expect_err("input waits for the commit")
                .code,
            "terminal-not-visible"
        );
    }

    // Another pane types while a publish holds the lifecycle lock across a
    // slow sidecar write.
    let held = fixture
        .dispatcher
        .lifecycle_sink
        .state
        .lock()
        .expect("lifecycle");
    let (sent, typed) = std::sync::mpsc::channel();
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _runtime = runtime.enter();
        let _ = sent.send(dispatcher.enqueue_input_frame(input_frame(second, second_generation)));
    });
    let admitted = typed.recv_timeout(Duration::from_secs(5));
    drop(held);
    admitted
        .expect("input admission never waits on the lifecycle lock")
        .expect("the other pane's input is admitted");

    fixture
        .dispatcher
        .finish_reopen_authority_commit(first)
        .await;
    assert_admission_mirror(&fixture.dispatcher);
    fixture
        .dispatcher
        .enqueue_input_frame(input_frame(first, first_generation))
        .expect("input resumes after the transaction ends");
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_dismissed_terminal_releases_its_input_lane_and_phone_replay() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let generation = TerminalGeneration(created["generation"].as_u64().expect("generation"));
    let key = (wire_id, generation.0);
    fixture
        .dispatcher
        .write_input(id, generation, b"typed\n".to_vec())
        .expect("type into the pane");
    assert!(mutex_lock(&fixture.dispatcher.lifecycle_sink.input_lanes).contains_key(&id));

    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect("close terminal");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    // An exited pane is still visible: the phone can still replay it.
    assert!(fixture.dispatcher.remote_hub.replay.read(key, 0).is_some());
    assert!(mutex_lock(&fixture.dispatcher.lifecycle_sink.input_lanes).contains_key(&id));

    fixture
        .dispatcher
        .dispatch(CHANNELS.dismiss_terminal, json!({"id": wire_id}))
        .await
        .expect("dismiss terminal");
    assert!(
        !mutex_lock(&fixture.dispatcher.lifecycle_sink.input_lanes).contains_key(&id),
        "a dismissed terminal must not keep an input lane thread"
    );
    // The Removed delivery (or the pump, once it has forwarded EOF) releases
    // the stream; no later request has to prune it.
    wait_for_replay(&fixture.dispatcher, key, false).await;
    fixture.shutdown().await;
}

/// Waits until the phone replay holds `key`, or until it no longer does.
async fn wait_for_replay(dispatcher: &TerminalHostDispatcher, key: (uuid::Uuid, u64), held: bool) {
    eventually("phone replay retention", || {
        dispatcher.remote_hub.replay.read(key, 0).is_some() == held
    })
    .await;
}

#[tokio::test]
async fn a_restart_retires_the_prior_generation_phone_replay() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let first = (wire_id, created["generation"].as_u64().expect("generation"));
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": wire_id, "generation": first.1}),
        )
        .await
        .expect("close terminal");
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    assert!(
        fixture
            .dispatcher
            .remote_hub
            .replay
            .read(first, 0)
            .is_some(),
        "an exited pane is still visible, so the phone can replay it"
    );

    let restarted = fixture
        .dispatcher
        .dispatch(
            CHANNELS.restart_terminal,
            json!({"id": wire_id, "generation": first.1}),
        )
        .await
        .expect("restart terminal");
    let second = (
        wire_id,
        restarted["generation"]
            .as_u64()
            .expect("restarted generation"),
    );
    assert_ne!(first, second);
    assert!(
        fixture
            .dispatcher
            .remote_hub
            .replay
            .read(second, 0)
            .is_some()
    );
    wait_for_replay(&fixture.dispatcher, first, false).await;
    fixture.shutdown().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn terminal_requests_do_not_wait_for_a_publish_stalled_in_its_durable_write() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let generation = TerminalGeneration(created["generation"].as_u64().expect("generation"));

    // The next publish stalls inside its durable sidecar write, holding the
    // lifecycle lock, until it is released (or ten seconds pass, so a
    // regression fails the elapsed-time assertion instead of hanging).
    let (entered, reached) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let armed = AtomicBool::new(true);
    *mutex_lock(&fixture.dispatcher.lifecycle_sink.durable_write_hook) =
        Some(Arc::new(move || {
            if armed.swap(false, Ordering::AcqRel) {
                let _ = entered.send(());
                let _ = mutex_lock(&released).recv_timeout(Duration::from_secs(10));
            }
        }));
    // TerminalService publishes on its caller's plain thread.
    let service = fixture.dispatcher.service.clone();
    let stalled = std::thread::spawn(move || {
        service.resize(ResizeTerminalRequest {
            id,
            generation,
            size: TerminalSize {
                columns: 100,
                rows: 30,
            },
        })
    });
    reached
        .recv_timeout(Duration::from_secs(5))
        .expect("the publish reached its durable write");

    // This runtime has one worker. A request that took the lifecycle lock
    // on it would park it until the write finished, stalling the rest too.
    let started = std::time::Instant::now();
    let dispatcher = Arc::clone(&fixture.dispatcher);
    let queued_resize = tokio::spawn(async move {
        dispatcher
            .dispatch(
                CHANNELS.resize_terminal,
                json!({
                    "id": wire_id,
                    "generation": generation.0,
                    "size": {"columns": 110, "rows": 32}
                }),
            )
            .await
    });
    tokio::task::yield_now().await;
    let profiles = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminal_launch_profiles, Value::Null)
        .await
        .expect("launch profiles");
    assert!(
        profiles
            .as_array()
            .is_some_and(|profiles| !profiles.is_empty())
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a terminal request waited for a publish's durable write"
    );
    assert!(
        !queued_resize.is_finished(),
        "a resize that publishes waits for the stalled publish off the async workers"
    );

    release.send(()).expect("release the durable write");
    stalled
        .join()
        .expect("stalled resize thread")
        .expect("stalled resize");
    queued_resize
        .await
        .expect("queued resize task")
        .expect("queued resize");
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_dismissal_rollback_retry_stops_once_rollback_no_longer_owns_the_transaction() {
    let fixture = Fixture::new();
    let sink = Arc::clone(&fixture.dispatcher.lifecycle_sink);

    // Retired elsewhere: nothing is left to roll back.
    tokio::time::timeout(
        Duration::from_secs(5),
        finish_dismiss_authority_rollback(Arc::clone(&sink), TerminalId::new(uuid::Uuid::new_v4())),
    )
    .await
    .expect("a missing transaction ends the retry");

    // Owned by a create, not by rollback: the retry leaves it alone.
    let id = TerminalId::new(uuid::Uuid::new_v4());
    fixture
        .dispatcher
        .begin_reopen_authority_commit(id, None, ReopenAuthorityTransactionKind::Create)
        .await
        .expect("begin a create transaction");
    tokio::time::timeout(
        Duration::from_secs(5),
        finish_dismiss_authority_rollback(Arc::clone(&sink), id),
    )
    .await
    .expect("a transaction rollback does not own ends the retry");
    assert!(
        mutex_lock(&sink.state).transactions.contains_key(&id),
        "the create transaction is untouched"
    );
    fixture.dispatcher.finish_reopen_authority_commit(id).await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn an_output_chunk_larger_than_one_frame_is_split_not_dropped() {
    let backend = Arc::new(FakeBackend::default());
    let printed = vec![b'x'; TERMINAL_DATA_MAX_PAYLOAD_BYTES + 4_096];
    *mutex_lock(&backend.output) = Some(printed.clone());
    let mut fixture = Fixture::new_with_parts(
        TerminalLaunchCatalog::shell_only(),
        None,
        TerminalServiceConfig {
            output_read_chunk_bytes: printed.len(),
            ..TerminalServiceConfig::default()
        },
        backend,
    );
    let wire_id = uuid::Uuid::new_v4();
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    let mut received = Vec::new();
    let mut sequences = Vec::new();
    let mut bytes = vec![0_u8; 64 * 1024];
    tokio::time::timeout(Duration::from_secs(5), async {
        while received.len() < printed.len() {
            let count = fixture
                .electron_output
                .read(&mut bytes)
                .await
                .expect("terminal output read");
            assert_ne!(count, 0, "terminal output pipe ended");
            for frame in decoder
                .push(&bytes[..count])
                .expect("terminal output frame")
            {
                if frame.kind == TerminalDataKind::Data {
                    assert!(frame.payload.len() <= TERMINAL_DATA_MAX_PAYLOAD_BYTES);
                    sequences.push(frame.sequence);
                    received.extend_from_slice(&frame.payload);
                }
            }
        }
    })
    .await
    .expect("every printed byte reaches Electron");
    assert_eq!(received, printed);
    assert_eq!(sequences, [0, 1]);
    fixture.shutdown().await;
}

/// Holds a live output owner for one generation so the next attempt to claim
/// it fails like a pump that could not start.
fn occupy_output(
    dispatcher: &TerminalHostDispatcher,
    id: TerminalId,
    generation: TerminalGeneration,
) -> std::sync::mpsc::Sender<()> {
    let (release, released) = std::sync::mpsc::channel::<()>();
    let task = std::thread::spawn(move || {
        let _ = released.recv();
    });
    mutex_lock(&dispatcher.output_pumps).insert(
        (id, generation),
        TerminalOutputPump {
            task,
            admission: None,
        },
    );
    release
}

#[tokio::test]
async fn create_and_restart_close_a_spawn_whose_output_cannot_be_claimed() {
    let fixture = Fixture::new();
    // Create: the new generation is dismissed and nothing becomes visible.
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let blocker = occupy_output(&fixture.dispatcher, id, TerminalGeneration::initial());
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect_err("the output owner is taken");
    assert_eq!(error.code, "terminal-output-owner-conflict");
    assert!(!error.replay_safe);
    assert!(fixture.dispatcher.service.snapshot().terminals.is_empty());
    assert!(*mutex_lock(&fixture.backend.process(0).stopped));
    drop(blocker);

    // Restart: the failed generation is closed and the pane stays visible.
    let wire_id = uuid::Uuid::new_v4();
    let id = TerminalId::new(wire_id);
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    fixture.backend.process(1).stop();
    wait_for_state(&fixture.dispatcher, TerminalState::Exited).await;
    let blocker = occupy_output(&fixture.dispatcher, id, TerminalGeneration(2));
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.restart_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect_err("the restarted generation's output owner is taken");
    assert_eq!(error.code, "terminal-output-owner-conflict");
    assert!(!error.replay_safe);
    assert!(*mutex_lock(&fixture.backend.process(2).stopped));
    let snapshot = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, Value::Null)
        .await
        .expect("terminal snapshot");
    assert_eq!(snapshot["terminals"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["terminals"][0]["id"], json!(wire_id));
    drop(blocker);
    fixture.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn background_rollback_retries_back_off_to_a_cap_and_never_stop() {
    let mut retry = CleanupRetry::default();
    let mut delays = Vec::new();
    for _ in 0..9 {
        let started = tokio::time::Instant::now();
        retry.wait().await;
        delays.push(started.elapsed().as_millis());
    }
    assert_eq!(delays, [100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000]);
    assert_eq!(
        blocking_step(|| Err::<(), _>("store busy")).await,
        Err("store busy".to_owned())
    );
    assert_eq!(blocking_step(|| Ok::<_, String>(1)).await, Ok(()));
}

/// The exact refusal a dispatch returned: code, message, retryable and
/// replay-safe, as the renderer receives them.
fn refusal(error: &DispatchError) -> (&str, &str, bool, bool) {
    (
        error.code.as_str(),
        error.message.as_str(),
        error.retryable,
        error.replay_safe,
    )
}

fn save_roster_terminal(fixture: &Fixture, terminal: &Value) {
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({
                "workspaces": [{
                    "id": "pinned",
                    "name": "Pinned",
                    "createdAt": "2026-09-27T00:00:00Z",
                    "updatedAt": "2026-09-27T00:00:00Z",
                    "terminals": [terminal]
                }]
            }),
        )
        .expect("renderer roster save");
}

#[tokio::test]
async fn create_refuses_an_unproven_or_unreadable_saved_authority_exactly() {
    let fixture = Fixture::new();
    let unproven = uuid::Uuid::new_v4();
    save_roster_terminal(
        &fixture,
        &json!({"id": unproven, "title": "Build", "cwd": fixture.root}),
    );
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(unproven, &fixture.root),
        )
        .await
        .expect_err("a saved ID added after startup has no proof");
    assert_eq!(
        refusal(&error),
        (
            "terminal-reopen-authority-unproven",
            "Interface could not prove that saved terminal's restart authority.",
            false,
            true,
        )
    );

    std::fs::write(
        fixture
            .temporary
            .path()
            .join(".terminal-workspaces.json.host-authority"),
        b"not the authority sidecar",
    )
    .expect("damage the authority sidecar");
    let fixture = fixture.reopen_without_workspace_grant().await;
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(unproven, &fixture.root),
        )
        .await
        .expect_err("an unreadable sidecar proves nothing");
    assert_eq!(
        refusal(&error),
        (
            "terminal-authority-unavailable",
            "Interface could not read that saved terminal's restart proof. Try again.",
            true,
            true,
        )
    );
    assert!(mutex_lock(&fixture.backend.processes).is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn restart_refuses_a_saved_entry_that_no_longer_matches_its_proof_exactly() {
    let fixture = Fixture::new();
    let wire_id = uuid::Uuid::new_v4();
    let created = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(wire_id, &fixture.root),
        )
        .await
        .expect("create terminal");
    save_roster_terminal(
        &fixture,
        &json!({
            "id": wire_id,
            "title": "Build",
            "cwd": fixture.root,
            "sessionKey": "another#session"
        }),
    );
    let error = fixture
        .dispatcher
        .dispatch(
            CHANNELS.restart_terminal,
            json!({"id": wire_id, "generation": created["generation"]}),
        )
        .await
        .expect_err("the saved entry names another session");
    assert_eq!(
        refusal(&error),
        (
            "terminal-reopen-authority-unproven",
            "Interface could not prove that terminal's restart authority.",
            false,
            true,
        )
    );
    assert_eq!(mutex_lock(&fixture.backend.processes).len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn an_operation_outside_the_terminal_table_is_refused_exactly() {
    let fixture = Fixture::new();
    let error = fixture
        .dispatcher
        .dispatch("iface:notATerminalOperation", Value::Null)
        .await
        .expect_err("unowned operation");
    assert_eq!(
        refusal(&error),
        (
            OPERATION_NOT_MIGRATED_CODE,
            "That operation is not owned by the integrated terminal service.",
            false,
            true,
        )
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn settings_terminal_tools_show_each_found_cli_version_and_an_install_hint_for_the_rest() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let claude = launch_directory.path().join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] || exit 7\nprintf '\\n  2.1.300 (Claude Code)\\nsecond line\\n'\n",
    )
    .expect("fake claude");
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &claude,
        Vec::new(),
        Vec::new(),
    )
    .expect("fixed launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    // The launcher's ordinary listing never waits on a CLI.
    let plain = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminal_launch_profiles, Value::Null)
        .await
        .expect("launch profiles");
    assert!(plain[1].get("version").is_none());
    let detailed = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"details": true}),
        )
        .await
        .expect("detailed launch profiles");
    assert_eq!(detailed[1]["id"], "claude");
    assert_eq!(detailed[1]["version"], "2.1.300 (Claude Code)");
    assert!(detailed[1].get("installHint").is_none());
    // A missing CLI says how to install it; Codex has one standard command.
    assert_eq!(detailed[2]["id"], "codex");
    assert_eq!(detailed[2]["available"], false);
    assert_eq!(
        detailed[2]["installHint"],
        "Install it with `npm install -g @openai/codex`, then press Refresh."
    );
    assert_eq!(
        detailed[5]["installHint"],
        "Install OMP so `omp` runs in your login shell, then press Refresh."
    );
    assert!(
        detailed[0].get("installHint").is_none(),
        "the shell is always there"
    );
    assert!(
        !detailed
            .to_string()
            .contains(&launch_directory.path().to_string_lossy().into_owned()),
        "a path never leaves the host"
    );
    let refused = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"details": "yes"}),
        )
        .await
        .expect_err("a malformed listing is refused");
    assert!(refused.replay_safe);
}

/// A fake `claude` that answers the provider canary: `--version` with
/// `version`, `--help` with every flag Interface depends on but `drop`, and
/// any argv ending in `--version` as accepted.
fn canary_claude(path: &Path, version: &str, drop: &str) {
    use interface_terminal::provider_canary::{CanaryProbe, expectation};
    let help = expectation(TerminalLaunchProfileId::Claude)
        .expect("Claude expectations")
        .probes
        .iter()
        .filter_map(|probe| match probe {
            CanaryProbe::Help { args, names } if *args == ["--help"] => Some(*names),
            _ => None,
        })
        .flatten()
        .filter(|name| **name != drop)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  --help) echo \"Options: {help}\" ;;\n  'auth --help') echo 'Commands: status login' ;;\n  *--version) echo '{version} (Claude Code)' ;;\n  *) exit 3 ;;\nesac\n"
        ),
    )
    .expect("fake claude");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[tokio::test]
async fn settings_terminal_tools_say_what_changed_in_a_cli_since_the_last_provider_check() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let claude = launch_directory.path().join("claude");
    canary_claude(&claude, "2.1.300", "");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &claude,
        Vec::new(),
        Vec::new(),
    )
    .expect("fixed launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    let store = fixture.temporary.path().join("provider-canary.v1.json");
    let watch = crate::provider_canary::ProviderCanaryWatch::open_file(&store).expect("store");
    // The background check waits longer than this test runs.
    fixture
        .dispatcher
        .start_provider_canary(watch, Duration::from_secs(3_600));
    let details = || async {
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.list_terminal_launch_profiles,
                json!({"details": true}),
            )
            .await
            .expect("detailed launch profiles")[1]
            .clone()
    };
    // First sight is checked in the background; the listing does not wait,
    // and it all matches: nothing to say.
    let first = details().await;
    assert_eq!(first["version"], "2.1.300 (Claude Code)");
    assert!(first.get("changedSinceLastCheck").is_none(), "{first}");
    eventually("the first check is kept", || {
        std::fs::read_to_string(&store).is_ok_and(|saved| saved.contains("2.1.300 (Claude Code)"))
    })
    .await;
    assert!(details().await.get("changedSinceLastCheck").is_none());
    // The launcher's ordinary listing never runs the check.
    let plain = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminal_launch_profiles, Value::Null)
        .await
        .expect("launch profiles");
    assert!(plain[1].get("changedSinceLastCheck").is_none());

    // An update that keeps everything Interface depends on.
    canary_claude(&claude, "2.1.301", "");
    let updated = details().await;
    let change = &updated["changedSinceLastCheck"];
    assert_eq!(change["version"], 1);
    assert_eq!(change["from"], "2.1.300");
    assert_eq!(change["to"], "2.1.301");
    assert_eq!(change["findings"], json!([]));
    assert!(
        change["checkedAt"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );

    // An update that drops a flag Interface passes.
    canary_claude(&claude, "2.1.302-next", "--session-id");
    let drifted = details().await;
    assert_eq!(drifted["changedSinceLastCheck"]["from"], "2.1.301");
    assert_eq!(drifted["changedSinceLastCheck"]["to"], "2.1.302");
    assert_eq!(
        drifted["changedSinceLastCheck"]["findings"],
        json!(["`claude --help` no longer lists --session-id."])
    );
    // Kept on disk, and not checked again while the version stays the same.
    let again = details().await;
    assert_eq!(
        again["changedSinceLastCheck"],
        drifted["changedSinceLastCheck"]
    );
    // The next update puts the flag back: what is shown is the new change.
    canary_claude(&claude, "2.1.303", "");
    let fixed = details().await;
    assert_eq!(fixed["changedSinceLastCheck"]["from"], "2.1.302");
    assert_eq!(fixed["changedSinceLastCheck"]["findings"], json!([]));
    let saved = std::fs::read_to_string(&store).expect("the check is kept");
    assert!(saved.contains("2.1.303 (Claude Code)"));
    assert!(!saved.contains(&launch_directory.path().to_string_lossy().into_owned()));
    fixture.shutdown().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One switch, followed from start through on, off and taken.
async fn settings_switches_grok_onto_interfaces_tools_only_when_asked_and_never_at_start() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    // A stand-in Grok whose user config is one state file: `mcp list --json`
    // prints it, `mcp add` writes Interface's entry, `mcp remove` deletes it.
    let state = launch_directory.path().join("grok-mcp.json");
    let ours = json!([{
        "command": "/bin/sh", "args": interface_terminal::pane_tools::PANE_TOOLS_ARGS,
        "enabled": true, "name": "iface", "scope": "user"
    }]);
    let grok = launch_directory.path().join("grok");
    std::fs::write(
        &grok,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n'mcp list') [ -f '{state}' ] && cat '{state}' || printf '[]\\n' ;;\n\
             'mcp add') printf '%s\\n' '{ours}' > '{state}' ;;\n'mcp remove') rm -f '{state}' ;;\nesac\n",
            state = state.display(),
        ),
    )
    .expect("fake grok");
    std::fs::set_permissions(&grok, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Grok,
        &grok,
        Vec::new(),
        Vec::new(),
    )
    .expect("fixed launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    let grok_row = |profiles: &Value| {
        profiles
            .as_array()
            .expect("profiles")
            .iter()
            .find(|profile| profile["id"] == "grok")
            .cloned()
            .expect("grok row")
    };

    // Opening the host and listing tools write nothing.
    let plain = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminal_launch_profiles, Value::Null)
        .await
        .expect("launch profiles");
    assert!(grok_row(&plain).get("paneTools").is_none());
    let detailed = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"details": true}),
        )
        .await
        .expect("detailed launch profiles");
    assert_eq!(grok_row(&detailed)["paneTools"], "off");
    assert!(!state.exists(), "nothing is written until the owner asks");
    // A missing Antigravity has nothing to switch.
    assert!(
        detailed
            .as_array()
            .unwrap()
            .iter()
            .all(|profile| profile["id"] == "grok" || profile.get("paneTools").is_none())
    );

    let on = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"details": true, "paneTools": {"profileId": "grok", "enabled": true}}),
        )
        .await
        .expect("switch on");
    assert_eq!(grok_row(&on)["paneTools"], "on");
    assert!(state.exists());

    let off = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"paneTools": {"profileId": "grok", "enabled": false}}),
        )
        .await
        .expect("switch off");
    assert_eq!(grok_row(&off)["paneTools"], "off");
    assert!(!state.exists());

    // The owner's own `iface` server is shown as taken and left alone.
    std::fs::write(
        &state,
        r#"[{"command":"/opt/mine","args":[],"enabled":true,"name":"iface","scope":"user"}]"#,
    )
    .unwrap();
    let taken = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"paneTools": {"profileId": "grok", "enabled": true}}),
        )
        .await
        .expect_err("an owner's own entry is never replaced");
    assert_eq!(taken.code, "terminal-pane-tools-taken");
    assert!(taken.replay_safe);
    assert!(
        std::fs::read_to_string(&state)
            .unwrap()
            .contains("/opt/mine")
    );
    let refused = fixture
        .dispatcher
        .dispatch(
            CHANNELS.list_terminal_launch_profiles,
            json!({"paneTools": {"profileId": "claude", "enabled": true}}),
        )
        .await
        .expect_err("only Grok and Antigravity keep the entry");
    assert!(refused.replay_safe);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One agent's clean exit, then one that was asked to close.
async fn an_agent_that_exits_by_itself_is_handed_a_shell_by_rust_whatever_the_window_shows() {
    let launch_directory = TempDir::new().expect("temporary launch directory");
    let claude = executable(&launch_directory, "claude");
    let claude = std::fs::canonicalize(claude).expect("canonical executable");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &claude,
        Vec::new(),
        Vec::new(),
    )
    .expect("fixed launch program")])
    .expect("launch catalog");
    let fixture = Fixture::new_with_launches(catalog);
    fixture
        .dispatcher
        .start_shell_handoff()
        .expect("shell handoff worker");
    assert!(
        fixture.dispatcher.start_shell_handoff().is_err(),
        "one worker per host"
    );
    let terminals = || fixture.dispatcher.service.snapshot().terminals;
    let agent = uuid::Uuid::new_v4();
    let agent_id = TerminalId::new(agent);
    let mut request = create_payload(agent, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("agent terminal");

    // The agent ends by itself — no renderer looks, none needs to.
    fixture.backend.process(0).stop();
    eventually("the agent's pane is handed a shell", || {
        terminals()
            .iter()
            .any(|terminal| terminal.replaces == Some(agent_id))
    })
    .await;
    let shell = terminals()
        .into_iter()
        .find(|terminal| terminal.replaces == Some(agent_id))
        .expect("shell");
    assert_eq!(
        shell.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Shell)
    );
    assert_eq!(Path::new(&shell.cwd), fixture.root.as_path());
    assert_eq!(shell.root, shell.cwd);
    assert_eq!(shell.state, TerminalState::Running);
    assert_eq!(
        (shell.size.columns, shell.size.rows),
        (90, 28),
        "the shell keeps the agent's size"
    );
    assert!(shell.launch_session.is_none() && shell.passed_off_from.is_none());
    eventually("the agent the shell replaced is dismissed", || {
        terminals().iter().all(|terminal| terminal.id != agent_id)
    })
    .await;
    assert_eq!(mutex_lock(&fixture.backend.specs).len(), 2);
    // The wire carries which agent the shell replaced.
    let listed = fixture
        .dispatcher
        .dispatch(CHANNELS.list_terminals, Value::Null)
        .await
        .expect("terminals");
    let listed_shell = listed["terminals"]
        .as_array()
        .expect("terminal list")
        .iter()
        .find(|terminal| terminal["id"] == json!(shell.id))
        .cloned()
        .expect("the shell is listed");
    assert_eq!(listed_shell["replaces"], json!(agent_id));

    // An agent someone asked to close keeps its tab and gets no shell.
    let closed = uuid::Uuid::new_v4();
    let closed_id = TerminalId::new(closed);
    let mut request = create_payload(closed, &fixture.root);
    request["launch"] = json!({"version": 1, "profileId": "claude"});
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, request)
        .await
        .expect("second agent");
    fixture
        .dispatcher
        .dispatch(
            CHANNELS.close_terminal,
            json!({"id": closed, "generation": created["generation"]}),
        )
        .await
        .expect("close the agent");
    eventually("the closed agent exits", || {
        terminals()
            .iter()
            .any(|terminal| terminal.id == closed_id && terminal.state == TerminalState::Exited)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        terminals()
            .iter()
            .all(|terminal| terminal.replaces != Some(closed_id)),
        "a requested close is not the agent finishing by itself"
    );
    assert!(terminals().iter().any(|terminal| terminal.id == closed_id));
    assert_eq!(mutex_lock(&fixture.backend.specs).len(), 3);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_to_pane_rejects_an_exited_pane() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4();
    let terminal_id = TerminalId::new(id);
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create_payload(id, &fixture.root))
        .await
        .expect("create terminal");

    let send = || async {
        let request: crate::voice::tools::SendToPane = serde_json::from_value(json!({
            "pane_id": id.to_string(),
            "text": "report back",
        }))
        .expect("send_to_pane payload");
        crate::voice::tools::send_to_pane(&fixture.dispatcher, request).await
    };

    assert!(send().await.is_ok());

    fixture.backend.process(0).stop();
    eventually("the pane exits", || {
        fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .iter()
            .any(|terminal| terminal.id == terminal_id && terminal.state == TerminalState::Exited)
    })
    .await;

    assert_eq!(
        send().await.err().as_deref(),
        Some("Pane is not running; input was not sent.")
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn preview_output_reads_agent_session_artifacts_without_granting_directory_listing() {
    let fixture = Fixture::new();
    let home = fixture.temporary.path().join("home");
    let omp_session = home.join(".omp/agent/sessions/session-1");
    let agy_brain = home.join(".gemini/antigravity-cli/brain/conv-1");
    fs::create_dir_all(&omp_session).expect("omp session dir");
    fs::create_dir_all(&agy_brain).expect("agy brain dir");
    let omp_session = fs::canonicalize(&omp_session).expect("canonical omp session");
    let agy_brain = fs::canonicalize(&agy_brain).expect("canonical agy brain");
    fs::write(omp_session.join("summary.md"), b"# OMP Report\n").expect("omp artifact");
    fs::write(agy_brain.join("walkthrough.md"), b"# Walkthrough\n").expect("agy artifact");

    for (dir, file, expected) in [
        (&omp_session, "summary.md", "# OMP Report\n"),
        (&agy_brain, "walkthrough.md", "# Walkthrough\n"),
    ] {
        let read = fixture
            .dispatcher
            .dispatch(
                CHANNELS.preview_output,
                json!({
                    "version": 1,
                    "action": "read",
                    "root": dir,
                    "path": file,
                }),
            )
            .await
            .expect("read agent session artifact");
        assert_eq!(read["snapshot"]["content"]["text"], expected);

        let list = fixture
            .dispatcher
            .dispatch(
                CHANNELS.preview_output,
                json!({
                    "version": 1,
                    "action": "list",
                    "root": dir,
                    "path": "",
                }),
            )
            .await;
        assert!(
            list.is_err(),
            "agent artifact root must not authorize directory listing"
        );
    }
    fixture.shutdown().await;
}

#[path = "terminal_restore_matrix_tests.rs"]
mod restore_matrix;
